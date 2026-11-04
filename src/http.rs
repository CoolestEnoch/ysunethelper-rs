use crate::{Failure, shutdown};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    Url,
    blocking::{Client, RequestBuilder},
    header::{ACCEPT, ACCEPT_LANGUAGE, COOKIE, HeaderMap, HeaderValue, SET_COOKIE},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    io::Read,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
#[derive(Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub expires: Option<i64>,
    #[serde(default)]
    pub host_only: bool,
}
fn root_path() -> String {
    "/".into()
}
#[derive(Default)]
pub struct Cookies(pub Vec<CookieEntry>);
impl Cookies {
    pub fn header(&mut self, url: &Url) -> String {
        self.0.retain(|e| e.expires.is_none_or(|t| t > epoch()));
        let host = url.host_str().unwrap_or_default();
        let mut matches: Vec<_> = self
            .0
            .iter()
            .filter(|e| {
                (host == e.domain || !e.host_only && host.ends_with(&format!(".{}", e.domain)))
                    && path_match(url.path(), &e.path)
                    && (!e.secure || url.scheme() == "https")
            })
            .collect();
        matches.sort_by_key(|e| std::cmp::Reverse(e.path.len()));
        matches
            .iter()
            .map(|e| format!("{}={}", e.name, e.value))
            .collect::<Vec<_>>()
            .join("; ")
    }
    pub fn absorb(&mut self, url: &Url, header: &str) {
        let Ok(c) = cookie::Cookie::parse(header) else {
            return;
        };
        let host = url.host_str().unwrap_or_default();
        let domain = c
            .domain()
            .unwrap_or(host)
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if host != domain && !host.ends_with(&format!(".{domain}")) {
            return;
        }
        let path = c
            .path()
            .filter(|p| p.starts_with('/'))
            .unwrap_or_else(|| {
                url.path()
                    .rsplit_once('/')
                    .map(|(p, _)| if p.is_empty() { "/" } else { p })
                    .unwrap_or("/")
            })
            .to_string();
        let expires = c
            .max_age()
            .map(|age| epoch().saturating_add(age.whole_seconds()))
            .or_else(|| c.expires_datetime().map(|d| d.unix_timestamp()));
        self.0
            .retain(|e| !(e.name == c.name() && e.domain == domain && e.path == path));
        if expires.is_some_and(|t| t <= epoch()) {
            return;
        }
        self.0.push(CookieEntry {
            name: c.name().into(),
            value: c.value().into(),
            domain,
            path,
            secure: c.secure().unwrap_or(false),
            expires,
            host_only: c.domain().is_none(),
        });
    }
}
pub fn path_match(request: &str, path: &str) -> bool {
    request == path
        || request
            .strip_prefix(path)
            .is_some_and(|suffix| path.ends_with('/') || suffix.starts_with('/'))
}
pub struct Response {
    pub status: u16,
    pub location: Option<String>,
    pub body: String,
}
pub struct Http {
    pub client: Client,
    pub cookies: RefCell<Cookies>,
    pub deadline: Cell<Option<Instant>>,
    timeout: Duration,
}
impl Http {
    pub fn new(timeout: Duration) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"));
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN,zh;q=0.9"));
        let client = Client::builder().redirect(Policy::none()).retry(reqwest::retry::never())
            .timeout(timeout).default_headers(headers)
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/58.0.3029.110 Safari/537.3").build()?;
        Ok(Self {
            client,
            cookies: RefCell::default(),
            deadline: Cell::new(None),
            timeout,
        })
    }
    pub fn send(&self, url: &str, mut builder: RequestBuilder) -> Result<Response> {
        shutdown::check()?;
        let parsed = Url::parse(url)?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https"),
            "不支持的 URL 协议"
        );
        let timeout = if let Some(deadline) = self.deadline.get() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "操作超时");
            remaining.min(self.timeout)
        } else {
            self.timeout
        };
        let cookie = self.cookies.borrow_mut().header(&parsed);
        if !cookie.is_empty() {
            let mut h = HeaderValue::from_str(&cookie)?;
            h.set_sensitive(true);
            builder = builder.header(COOKIE, h);
        }
        let response = builder
            .timeout(timeout)
            .send()
            .map_err(|e| Failure::Network(e.without_url().into()))?;
        for header in response.headers().get_all(SET_COOKIE) {
            if let Ok(value) = header.to_str() {
                self.cookies.borrow_mut().absorb(&parsed, value);
            }
        }
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        response
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| Failure::Network(e.into()))?;
        ensure!(bytes.len() <= 4 * 1024 * 1024, "响应超过 4 MiB");
        Ok(Response {
            status,
            location,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
    pub fn get(&self, url: &str) -> Result<Response> {
        self.send(url, self.client.get(url))
    }
    pub fn json(&self, url: &str, value: &serde_json::Value) -> Result<Response> {
        self.send(url, self.client.post(url).json(value))
    }
    pub fn follow(&self, url: &str) -> Result<(String, String)> {
        let mut current = Url::parse(url)?;
        for _ in 0..10 {
            let response = self.get(current.as_str())?;
            let next = if redirect(response.status) {
                Some(response.location.context("重定向缺少 Location")?)
            } else {
                js_redirect(&response.body)
            };
            if let Some(next) = next {
                current = current.join(&next.replace(' ', "%20"))?;
            } else {
                ensure!(
                    (200..300).contains(&response.status),
                    "HTTP {}",
                    response.status
                );
                return Ok((current.to_string(), response.body));
            }
        }
        bail!("重定向超过 10 次")
    }
}
pub fn redirect(code: u16) -> bool {
    matches!(code, 301 | 302 | 303 | 307 | 308)
}
pub fn js_redirect(body: &str) -> Option<String> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"location\.href\s*=\s*['"]([^'"]+)['"]"#).unwrap()
    });
    RE.captures(body).map(|c| c[1].to_string())
}
pub fn query(base: &str, args: &[(&str, &str)]) -> Result<String> {
    let mut url = Url::parse(base)?;
    url.query_pairs_mut().extend_pairs(args.iter().copied());
    Ok(url.to_string())
}
