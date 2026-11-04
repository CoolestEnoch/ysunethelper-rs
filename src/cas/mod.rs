pub mod crypto;
pub mod parser;
use crate::{
    Failure, config,
    http::{self, CookieEntry, Http},
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
pub const BASE: &str = "https://cer.ysu.edu.cn";
pub struct Cas {
    pub http: Http,
    pub base: String,
    pub username: String,
}
#[derive(Serialize, Deserialize)]
struct Credential {
    #[serde(default)]
    username: String,
    cookies: Vec<CookieEntry>,
}
impl Cas {
    pub fn new(timeout: Duration) -> Result<Self> {
        Self::with_base(timeout, BASE)
    }
    pub fn with_base(timeout: Duration, base: &str) -> Result<Self> {
        Ok(Self {
            http: Http::new(timeout)?,
            base: base.trim_end_matches('/').into(),
            username: String::new(),
        })
    }
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    pub fn load(&mut self, path: &Path) -> Result<()> {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut cred: Credential = serde_json::from_slice(&data).context("CAS 缓存格式错误")?;
        let host = Url::parse(&self.base)?
            .host_str()
            .context("无效 CAS 域名")?
            .to_owned();
        for cookie in &mut cred.cookies {
            if cookie.domain.is_empty() {
                cookie.domain = host.clone();
            }
            if cookie.path.is_empty() {
                cookie.path = "/".into();
            }
        }
        cred.cookies.retain(|e| {
            e.domain == host
                && (e.path == "/" || e.path.starts_with("/authserver"))
                && e.expires.is_none_or(|t| t > http::epoch())
        });
        self.http.cookies.borrow_mut().0 = cred.cookies;
        self.username = cred.username;
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let parsed = Url::parse(&self.base)?;
        let host = parsed.host_str().unwrap_or_default();
        let cookies = self
            .http
            .cookies
            .borrow()
            .0
            .iter()
            .filter(|c| {
                c.domain == host
                    && (c.path == "/" || c.path.starts_with("/authserver"))
                    && c.expires.is_none_or(|t| t > http::epoch())
            })
            .cloned()
            .collect();
        config::write_private(
            path,
            &serde_json::to_vec_pretty(&Credential {
                username: self.username.clone(),
                cookies,
            })?,
            true,
        )
    }
    pub fn drop_credential(&mut self) {
        self.username.clear();
        self.http.cookies.borrow_mut().0.clear();
    }
    pub fn authenticated(&self) -> Result<bool> {
        let r = self.http.get(&self.endpoint("/authserver/index.do"))?;
        if http::redirect(r.status) {
            return Ok(!r
                .location
                .context("CAS 重定向缺少 Location")?
                .contains("/authserver/login"));
        }
        ensure!(
            r.status == 200 || r.status == 401,
            "CAS 状态查询 HTTP {}",
            r.status
        );
        Ok(r.status == 200)
    }
    pub fn login(&mut self, username: &str, password: &str) -> Result<()> {
        let login = self.endpoint("/authserver/login");
        let service = self.endpoint("/personalInfo/personCenter/index.html");
        let page_url = http::query(&login, &[("service", &service)])?;
        let (_, page) = self.http.follow(&page_url)?;
        let fields = parser::fields(&page);
        let execution = fields
            .get("execution")
            .filter(|s| !s.is_empty())
            .context("CAS 页面缺少 execution")?;
        let salt = fields
            .get("pwdEncryptSalt")
            .context("CAS 页面缺少 pwdEncryptSalt")?;
        let encrypted = crypto::encrypt(password, salt)?;
        let post = http::query(&page_url, &[("_", &http::millis().to_string())])?;
        let r = self.http.send(
            &post,
            self.http
                .client
                .post(&post)
                .header("Origin", &self.base)
                .header("Referer", &page_url)
                .form(&[
                    ("username", username),
                    ("password", encrypted.as_str()),
                    ("captcha", ""),
                    ("_eventId", "submit"),
                    ("cllt", "userNameLogin"),
                    ("dllt", "generalLogin"),
                    ("lt", ""),
                    ("execution", execution),
                ]),
        )?;
        if http::redirect(r.status) {
            let location = r.location.context("CAS 登录重定向缺少 Location")?;
            if parser::challenge(&location) {
                return Err(Failure::Challenge("MFA".into()).into());
            }
            let target = Url::parse(&post)?.join(&location)?;
            if target.as_str().contains(&service)
                || target.query_pairs().any(|(k, _)| k == "ticket")
            {
                // TGC has already been installed. Landing on the service is best effort.
                let _ = self.http.follow(target.as_str());
            } else {
                let (url, body) = self.http.follow(target.as_str())?;
                if parser::challenge(&body) {
                    return Err(Failure::Challenge("MFA".into()).into());
                }
                ensure!(
                    url.contains(&service)
                        || Url::parse(&url)?.query_pairs().any(|(k, _)| k == "ticket"),
                    "无法识别 CAS 登录跳转结果"
                );
            }
        } else {
            ensure!(matches!(r.status, 200 | 401), "CAS 登录 HTTP {}", r.status);
            if ["IP freeze", "has been blocked", "IP被冻结"]
                .iter()
                .any(|k| r.body.contains(k))
            {
                return Err(Failure::Blocked.into());
            }
            let message = parser::error_message(&r.body);
            if parser::challenge(&r.body)
                || message.contains("验证码")
                || message.to_ascii_lowercase().contains("captcha")
            {
                return Err(Failure::Challenge(message).into());
            }
            return Err(Failure::Credentials(if message.is_empty() {
                "服务端未提供原因".into()
            } else {
                message
            })
            .into());
        }
        self.username = username.into();
        Ok(())
    }
    pub fn ticket(&self, service: &str) -> Result<String> {
        let url = http::query(&self.endpoint("/authserver/login"), &[("service", service)])?;
        let r = self.http.get(&url)?;
        ensure!(http::redirect(r.status), "CAS 出票未返回重定向");
        let location = r.location.context("CAS 出票缺少 Location")?;
        if location.contains("/authserver/login") {
            return Err(Failure::Unauthenticated.into());
        }
        let parsed = Url::parse(&url)?.join(&location)?;
        if let Some((_, ticket)) = parsed.query_pairs().find(|(k, _)| k == "ticket") {
            return Ok(ticket.into_owned());
        }
        bail!("CAS 回跳缺少 ticket")
    }
}
