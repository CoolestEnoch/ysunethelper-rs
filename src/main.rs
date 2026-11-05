mod cli;
mod ui;
use anyhow::{Context, Result};
use clap::CommandFactory;
use cli::{Cli, Command, VERSION};
use serde_json::json;
use std::io::{self, IsTerminal};
use ysunethelper::{
    Failure,
    cas::Cas,
    config::{self, Config},
    daemon,
    portal::Portal,
    probe::Prober,
    shutdown,
};
fn main() {
    let cli = cli::parse();
    let result = shutdown::install().and_then(|_| run(cli));
    if shutdown::stopped() {
        std::process::exit(130);
    }
    if let Err(e) = result {
        eprintln!("ysunethelper: {e:#}");
        let hard = e
            .chain()
            .any(|e| matches!(e.downcast_ref::<Failure>(), Some(Failure::Hard(_))));
        std::process::exit(if hard { 78 } else { 1 });
    }
}
fn output(value: impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
fn run(cli: Cli) -> Result<()> {
    if cli.show_version {
        println!("ysunethelper {VERSION}");
        return Ok(());
    }
    let Some(command) = cli.command else {
        Cli::command().print_help()?;
        println!();
        let system = std::path::Path::new("/etc/ysunethelper/config.json");
        if system.exists() {
            match Config::load(system).and_then(|cfg| cfg.validate_daemon()) {
                Ok(()) => {}
                Err(e) => eprintln!("请检查系统配置 {}: {e}", system.display()),
            }
        }
        std::process::exit(2);
    };
    if matches!(command, Command::Version) {
        println!("ysunethelper {VERSION}");
        return Ok(());
    }
    let is_daemon = matches!(command, Command::Daemon);
    let explicit = cli.config.as_deref().filter(|p| !p.as_os_str().is_empty());
    let path = config::resolve(explicit, is_daemon);
    let mut cfg = match Config::load(&path) {
        Ok(c) => c,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            if is_daemon {
                let target = explicit
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| "ysunethelper.json".into());
                config::write_private(&target, config::TEMPLATE.as_bytes(), false)?;
                println!(
                    "已生成配置模板 {}，请填写 username/password 后重新运行 daemon。",
                    target.display()
                );
                return Ok(());
            }
            Config::default()
        }
        Err(e) if is_daemon => {
            return Err(Failure::Hard(format!("{}: {e:#}", path.display())).into());
        }
        Err(e) => return Err(e),
    };
    if is_daemon {
        cfg.validate_daemon()
            .map_err(|e| Failure::Hard(format!("{}: {e}", path.display())))?;
        return daemon::run(&cfg, cli.verbose);
    }
    let portal = Portal::new(cfg.http_timeout.0)?;
    match command {
        Command::Status { json, test } => {
            let st = portal.status()?;
            if json {
                output(if cli.verbose {
                    serde_json::to_value(&st)?
                } else {
                    st.summary()
                })?;
            } else {
                ui::status(&st, cli.verbose)?;
            }
            // Preserve the Go CLI's text suffix even when --json is also specified.
            if test {
                for r in Prober::new(&cfg.daemon.probe_urls, cfg.daemon.probe_timeout.0)?.check() {
                    shutdown::check()?;
                    println!(
                        "internet probe {} {} ({})",
                        if r.ok { "OK  " } else { "FAIL" },
                        r.url,
                        r.detail
                    );
                }
                shutdown::check()?;
                println!(
                    "status: {} (portal)",
                    if st.online { "ONLINE" } else { "OFFLINE" }
                );
            }
        }
        Command::Login {
            username,
            password,
            service,
            json,
        } => {
            if let Some(u) = username.filter(|s| !s.is_empty()) {
                cfg.username = u;
            }
            if let Some(p) = password.filter(|s| !s.is_empty()) {
                cfg.password = p;
            }
            if let Some(s) = service.filter(|s| !s.is_empty()) {
                cfg.service = s;
            }
            let mut cas = Cas::new(cfg.http_timeout.0)?;
            cas.load(&cfg.credential_path)?;
            let mut valid = cas.authenticated()?;
            if valid && !cfg.username.is_empty() && cas.username != cfg.username {
                cas.drop_credential();
                valid = false;
            }
            if !valid {
                if cfg.username.is_empty() || cfg.password.is_empty() {
                    anyhow::ensure!(
                        io::stdin().is_terminal(),
                        "缺少账号密码，请使用配置文件或 -u/-p（交互登录需要终端）"
                    );
                    let line = ui::input::line("用户名: ", false)?;
                    cfg.username = line.trim().into();
                    anyhow::ensure!(!cfg.username.is_empty(), "用户名不能为空");
                    cfg.password = ui::input::line("密码: ", true)?;
                    anyhow::ensure!(!cfg.password.is_empty(), "密码不能为空");
                }
                cas.login(&cfg.username, &cfg.password)?;
                cas.save(&cfg.credential_path)
                    .context("保存 CAS 凭据失败")?;
            }
            let st = portal.login(&cas, &cfg.service)?;
            if json {
                let mut value = st.summary();
                let object = value.as_object_mut().unwrap();
                object.remove("online");
                object.remove("remaining_traffic");
                value["ok"] = true.into();
                output(value)?;
            } else {
                println!(
                    "登录成功。用户: {} / 服务: {} / IP: {}",
                    st.username, st.service, st.user_ip
                );
            }
        }
        Command::Logout { json } => {
            portal.logout()?;
            if json {
                output(json!({"ok":true}))?;
            } else {
                println!("下线成功");
            }
        }
        Command::Switch { json, service } => {
            let st = portal.switch(&service)?;
            if json {
                let mut value = json!({"ok":true,"online":st.online,"service":st.service});
                if !st.username.is_empty() {
                    value["username"] = st.username.into();
                }
                if !st.user_ip.is_empty() {
                    value["user_ip"] = st.user_ip.into();
                }
                // Go switch emits one JSON object per line; preserve it for line readers.
                println!("{}", serde_json::to_string(&value)?);
            } else {
                println!("当前运营商: {} / IP: {}", st.service, st.user_ip);
            }
        }
        Command::Tui => ui::menu(&portal)?,
        Command::Devices { json } => {
            let list = portal.devices()?;
            if json {
                output(list)?;
            } else {
                ui::devices(&list);
            }
        }
        Command::Kick { json, targets } => {
            let kicked = portal.kick(&targets)?;
            if json {
                output(json!({"ok":true,"kicked":kicked}))?;
            } else {
                let descriptions = kicked
                    .iter()
                    .map(|d| {
                        format!(
                            "{}({})",
                            if d.name.is_empty() {
                                "未命名"
                            } else {
                                &d.name
                            },
                            d.uuid
                        )
                    })
                    .collect::<Vec<_>>();
                println!(
                    "下线请求已提交: {}（执行可能有延迟，可用 devices 复查）",
                    descriptions.join(", ")
                );
            }
        }
        Command::Daemon | Command::Version => unreachable!(),
    }
    Ok(())
}
