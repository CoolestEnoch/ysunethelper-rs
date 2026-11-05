use crate::{
    Failure, cas::Cas, config::Config, http::epoch, is_network, portal::Portal, probe::Prober,
    shutdown,
};
use anyhow::Result;
use std::time::Duration;

pub fn ensure_cas(cas: &mut Cas, cfg: &Config) -> Result<()> {
    let mut authenticated = cas.authenticated()?;
    if authenticated && !cfg.username.is_empty() && cas.username != cfg.username {
        cas.drop_credential();
        authenticated = false;
    }
    if !authenticated {
        cas.login(&cfg.username, &cfg.password)
            .map_err(Failure::Login)?;
        cas.save(&cfg.credential_path)?;
    }
    Ok(())
}
#[derive(Default)]
pub struct RetryPolicy {
    pub login_failures: usize,
    pub post_auth_failures: usize,
}
#[derive(Debug, PartialEq)]
pub enum Action {
    Stop,
    NoLink,
    LongBackoff,
    Backoff,
}
impl RetryPolicy {
    pub fn on_error(&mut self, error: &anyhow::Error) -> Action {
        let kinds: Vec<_> = error
            .chain()
            .filter_map(|e| e.downcast_ref::<Failure>())
            .collect();
        if kinds.iter().any(|e| {
            matches!(
                e,
                Failure::Credentials(_) | Failure::Challenge(_) | Failure::Hard(_)
            )
        }) {
            return Action::Stop;
        }
        if kinds.iter().any(|e| matches!(e, Failure::Blocked)) {
            return Action::LongBackoff;
        }
        if is_network(error) {
            return Action::NoLink;
        }
        if kinds.iter().any(|e| matches!(e, Failure::Login(_))) {
            self.login_failures += 1;
            if self.login_failures >= 3 {
                return Action::Stop;
            }
        }
        Action::Backoff
    }
}
pub fn run(cfg: &Config, verbose: bool) -> Result<()> {
    let mut cas = Cas::new(cfg.http_timeout.0)?;
    if let Err(e) = cas.load(&cfg.credential_path) {
        eprintln!("加载 CAS 缓存失败，将重新登录: {e}");
    }
    let portal = Portal::new(cfg.http_timeout.0)?;
    let probe = Prober::new(&cfg.daemon.probe_urls, cfg.daemon.probe_timeout.0)?;
    let mut retry = RetryPolicy::default();
    let mut backoff = cfg.daemon.backoff_initial.0;
    let mut state = "INIT";
    while !shutdown::stopped() {
        if paused(cfg) {
            state = "INIT";
            backoff = cfg.daemon.backoff_initial.0;
            retry.post_auth_failures = 0;
            continue;
        }
        if verbose {
            eprintln!("state={state} probe");
        }
        if probe.online() {
            state = "ONLINE";
            backoff = cfg.daemon.backoff_initial.0;
            retry.post_auth_failures = 0;
            sleep(cfg, cfg.daemon.probe_interval.0);
            continue;
        }
        if shutdown::stopped() {
            break;
        }
        if state != "INIT" {
            let mut confirmed = true;
            for _ in 1..cfg.daemon.probe_confirm {
                sleep(cfg, cfg.daemon.probe_confirm_gap.0);
                if shutdown::stopped() || in_period(cfg) || probe.online() {
                    confirmed = false;
                    break;
                }
            }
            if !confirmed {
                continue;
            }
        }
        if paused(cfg) || shutdown::stopped() {
            continue;
        }
        let status = match portal.status() {
            Ok(st) => st,
            Err(e) => {
                if shutdown::stopped() {
                    break;
                }
                eprintln!("状态查询失败: {e}");
                if is_network(&e) {
                    state = "NO_LINK";
                    sleep(cfg, cfg.daemon.nolink_interval.0);
                } else {
                    state = "BACKOFF";
                    sleep_backoff(cfg, &mut backoff);
                }
                continue;
            }
        };
        if paused(cfg) || shutdown::stopped() {
            continue;
        }
        state = "OFFLINE";
        if status.online {
            eprintln!("门户在线但 Internet 不通，重新认证");
            if let Err(e) = portal.logout() {
                eprintln!("下线失败: {e}");
            }
        }
        if paused(cfg) || shutdown::stopped() {
            continue;
        }
        let result = ensure_cas(&mut cas, cfg).and_then(|_| {
            // Credential exchange can cross a scheduled boundary: check again before admission.
            if in_period(cfg) || shutdown::stopped() {
                anyhow::bail!("暂停准入");
            }
            portal.login(&cas, &cfg.service)
        });
        if let Err(e) = result {
            if shutdown::stopped() {
                break;
            }
            eprintln!("认证失败: {e:#}");
            match retry.on_error(&e) {
                Action::Stop => return Err(Failure::Hard(e.to_string()).into()),
                Action::NoLink => {
                    state = "NO_LINK";
                    sleep(cfg, cfg.daemon.nolink_interval.0);
                }
                Action::LongBackoff => {
                    state = "BACKOFF";
                    sleep(cfg, cfg.daemon.backoff_max.0);
                }
                Action::Backoff => {
                    state = "BACKOFF";
                    sleep_backoff(cfg, &mut backoff);
                }
            }
            continue;
        }
        retry.login_failures = 0;
        if paused(cfg) || shutdown::stopped() {
            continue;
        }
        if probe.online() {
            state = "ONLINE";
            backoff = cfg.daemon.backoff_initial.0;
            retry.post_auth_failures = 0;
            eprintln!("认证成功，Internet 已恢复");
            sleep(cfg, cfg.daemon.probe_interval.0);
        } else {
            retry.post_auth_failures += 1;
            state = "BACKOFF";
            if retry.post_auth_failures >= 2 {
                retry.post_auth_failures = 0;
                eprintln!("认证后 Internet 仍不通，进入长退避");
                sleep(cfg, cfg.daemon.backoff_max.0);
            } else {
                sleep_backoff(cfg, &mut backoff);
            }
        }
    }
    Ok(())
}
fn in_period(cfg: &Config) -> bool {
    cfg.daemon
        .no_auth_period
        .next_window(epoch())
        .is_some_and(|(start, _)| start <= epoch())
}
fn paused(cfg: &Config) -> bool {
    let now = epoch();
    if let Some((start, end)) = cfg.daemon.no_auth_period.next_window(now)
        && start <= now
    {
        eprintln!("AUTH_PAUSED: 禁认证时段，暂停探测和认证");
        shutdown::wait(Duration::from_secs((end - now) as u64));
        return true;
    }
    false
}
fn sleep(cfg: &Config, duration: Duration) {
    let jitter = 0.8 + rand::random::<f64>() * 0.4;
    let mut delay = duration.mul_f64(jitter);
    if let Some((start, _)) = cfg.daemon.no_auth_period.next_window(epoch()) {
        delay = delay.min(Duration::from_secs((start - epoch()).max(0) as u64));
    }
    shutdown::wait(delay);
}
fn sleep_backoff(cfg: &Config, backoff: &mut Duration) {
    sleep(cfg, *backoff);
    *backoff = backoff.saturating_mul(2).min(cfg.daemon.backoff_max.0);
}
