mod period;
use anyhow::{Context, Result, ensure};
pub use period::NoAuthPeriod;
use serde::{Deserialize, Deserializer};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
pub struct Span(pub Duration);
impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        parse_duration(&String::deserialize(d)?)
            .map(Span)
            .map_err(serde::de::Error::custom)
    }
}
pub fn parse_duration(input: &str) -> Result<Duration> {
    if input == "0" {
        return Ok(Duration::ZERO);
    }
    let mut rest = input;
    let mut seconds = 0.0;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .context("时长缺少单位")?;
        ensure!(end > 0, "无效时长: {input}");
        let n: f64 = rest[..end].parse()?;
        rest = &rest[end..];
        let (unit, scale) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))
        .context("无效时长单位")?;
        seconds += n * scale;
        rest = &rest[unit.len()..];
    }
    ensure!(!input.is_empty(), "时长不能为空");
    Ok(Duration::try_from_secs_f64(seconds)?)
}
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub probe_urls: Vec<String>,
    pub probe_interval: Span,
    pub probe_confirm: usize,
    pub probe_confirm_gap: Span,
    pub probe_timeout: Span,
    pub nolink_interval: Span,
    pub backoff_initial: Span,
    pub backoff_max: Span,
    pub no_auth_period: NoAuthPeriod,
}
impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            probe_urls: vec![],
            probe_interval: Span(Duration::from_secs(60)),
            probe_confirm: 3,
            probe_confirm_gap: Span(Duration::from_secs(3)),
            probe_timeout: Span(Duration::from_secs(5)),
            nolink_interval: Span(Duration::from_secs(15)),
            backoff_initial: Span(Duration::from_secs(10)),
            backoff_max: Span(Duration::from_secs(600)),
            no_auth_period: NoAuthPeriod::default(),
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub username: String,
    pub password: String,
    pub service: String,
    pub credential_path: PathBuf,
    pub http_timeout: Span,
    pub daemon: DaemonConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            service: "campus".into(),
            credential_path: user_dir().join("cas.json"),
            http_timeout: Span(Duration::from_secs(30)),
            daemon: DaemonConfig::default(),
        }
    }
}
pub fn user_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/ysunethelper")
}
pub fn resolve(explicit: Option<&Path>, daemon: bool) -> PathBuf {
    if let Some(path) = explicit {
        return path.into();
    }
    let local = PathBuf::from("ysunethelper.json");
    if local.exists() {
        return local;
    }
    let user = user_dir().join("config.json");
    if user.exists() || !daemon {
        return user;
    }
    PathBuf::from("/etc/ysunethelper/config.json")
}
impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = serde_json::from_slice(
            &fs::read(path).with_context(|| format!("读取配置 {}", path.display()))?,
        )?;
        if config.service.is_empty() {
            config.service = "campus".into();
        }
        if config.credential_path.as_os_str().is_empty() {
            config.credential_path = user_dir().join("cas.json");
        }
        if config.http_timeout.0.is_zero() {
            config.http_timeout = Self::default().http_timeout;
        }
        let defaults = DaemonConfig::default();
        let d = &mut config.daemon;
        for (span, default) in [
            (&mut d.probe_interval, defaults.probe_interval),
            (&mut d.probe_confirm_gap, defaults.probe_confirm_gap),
            (&mut d.probe_timeout, defaults.probe_timeout),
            (&mut d.nolink_interval, defaults.nolink_interval),
            (&mut d.backoff_initial, defaults.backoff_initial),
            (&mut d.backoff_max, defaults.backoff_max),
        ] {
            if span.0.is_zero() {
                *span = default;
            }
        }
        if d.probe_confirm == 0 {
            d.probe_confirm = defaults.probe_confirm;
        }
        if d.no_auth_period.start.is_empty() {
            d.no_auth_period.start = "23:30".into();
        }
        if d.no_auth_period.end.is_empty() {
            d.no_auth_period.end = "05:30".into();
        }
        d.no_auth_period.validate()?;
        ensure!(
            d.backoff_initial.0 <= d.backoff_max.0,
            "backoff_initial 不能大于 backoff_max"
        );
        Ok(config)
    }
    pub fn validate_daemon(&self) -> Result<()> {
        ensure!(
            !self.username.trim().is_empty()
                && !self.password.is_empty()
                && !self.service.trim().is_empty(),
            "请填写 username/password，并确认 service"
        );
        self.daemon.no_auth_period.validate()
    }
}
pub const TEMPLATE: &str = include_str!("template.json");
/// Create a private file before writing secret material; never follow a preexisting symlink.
pub fn write_private(path: &Path, data: &[u8], replace: bool) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let temp = parent.join(format!(".ysunethelper-{:016x}.tmp", rand::random::<u64>()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let outcome = (|| -> Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        if replace {
            #[cfg(windows)]
            if path.exists() {
                fs::remove_file(path)?;
            }
            fs::rename(&temp, path)?;
        } else {
            fs::hard_link(&temp, path)?;
        }
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    outcome
}
