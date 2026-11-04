mod devices;
mod services;
use crate::{
    cas::Cas,
    http::{self, Http, Response},
};
use anyhow::{Context, Result, bail, ensure};
pub use devices::{Device, DeviceList, resolve_targets};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub use services::{Service, resolve_service, service_number};
use std::{collections::HashMap, time::Duration};
/// Login preserves unrecognized service names exactly, as in the Go client.
pub fn login_service(service: &str) -> &str {
    match service {
        "campus" => "校园网",
        "unicom" => "中国联通",
        "telecom" => "中国电信",
        "mobile" => "中国移动",
        _ => service,
    }
}
pub const BASE: &str = "https://auth1.ysu.edu.cn";
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Status {
    pub online: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_ip: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_mac: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remaining_traffic: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "empty_object")]
    pub account: Value,
    #[serde(default, skip_serializing_if = "empty_object")]
    pub raw: Value,
}
fn empty_object(value: &Value) -> bool {
    value.is_null() || value.as_object().is_some_and(|v| v.is_empty())
}
impl Status {
    pub fn current(&self, service: &str) -> bool {
        self.online
            && !service.is_empty()
            && (login_service(&self.service) == login_service(service)
                || field(&self.raw, "service") == service)
    }
    pub fn summary(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("status is serializable");
        for field in ["message", "account", "raw"] {
            value.as_object_mut().unwrap().remove(field);
        }
        value
    }
}
pub fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}
pub struct Portal {
    pub http: Http,
    pub base: String,
}
impl Portal {
    pub fn new(timeout: Duration) -> Result<Self> {
        Self::with_base(timeout, BASE)
    }
    pub fn with_base(timeout: Duration, base: &str) -> Result<Self> {
        Ok(Self {
            http: Http::new(timeout)?,
            base: base.trim_end_matches('/').into(),
        })
    }
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    fn envelope(response: Response) -> Result<Value> {
        ensure!(response.status == 200, "Portal HTTP {}", response.status);
        let env: Value = serde_json::from_str(&response.body).context("Portal 未返回有效 JSON")?;
        ensure!(
            env["code"] == 200 && env.get("success").is_none_or(|v| v.is_null() || v == true),
            "Portal 拒绝请求: {}",
            field(&env, "message")
        );
        Ok(env)
    }
    fn post_envelope(&self, path: &str, body: Value) -> Result<Value> {
        Self::envelope(self.http.json(&self.endpoint(path), &body)?)
    }
    fn post(&self, path: &str, body: Value) -> Result<Value> {
        Ok(self.post_envelope(path, body)?["data"].take())
    }
    pub fn session(&self) -> Result<HashMap<String, String>> {
        let (url, _) = self
            .http
            .follow(&self.endpoint("/eportal/redirect.jsp?mode=history"))?;
        ensure!(
            url.contains("portal-main"),
            "Portal 未到达 portal-main 页面"
        );
        let values: HashMap<_, _> = Url::parse(&url)?.query_pairs().into_owned().collect();
        ensure!(
            values.get("sessionId").is_some_and(|s| !s.is_empty()),
            "Portal 缺少 sessionId"
        );
        Ok(values)
    }
    pub fn query_status(&self, session: &str) -> Result<Status> {
        let url = http::query(
            &self.endpoint("/eportal/adaptor/getOnlineUserInfo"),
            &[("sessionId", session), ("_", &http::millis().to_string())],
        )?;
        let mut env = Self::envelope(self.http.get(&url)?)?;
        let raw = env["data"]["portalOnlineUserInfo"].take();
        ensure!(raw.is_object(), "Portal 缺少 portalOnlineUserInfo");
        Ok(Status {
            online: field(&raw, "result") == "success" || !field(&raw, "userName").is_empty(),
            username: field(&raw, "userName").into(),
            service: {
                let s = field(&raw, "realServiceName");
                if s.is_empty() {
                    field(&raw, "service")
                } else {
                    s
                }
            }
            .into(),
            user_ip: field(&raw, "userIp").into(),
            user_mac: field(&raw, "userMac").into(),
            message: field(&raw, "message").into(),
            raw,
            ..Default::default()
        })
    }
    pub fn status(&self) -> Result<Status> {
        let info = self.session()?;
        let mut st = self.query_status(&info["sessionId"])?;
        if st.online {
            st.account = self.post(
                "/eportal/operator/getAccountInfo",
                json!({"sessionId":info["sessionId"]}),
            )?;
            ensure!(st.account.is_object(), "账户信息不是对象");
            st.name = field(&st.account, "name").into();
            if st.service == "校园网"
                && let Some(items) = st.account["accountInfo"].as_array()
            {
                st.remaining_traffic = items
                    .iter()
                    .find(|v| field(v, "title") == "剩余流量")
                    .map(|v| field(v, "content").to_owned())
                    .unwrap_or_default();
            }
        }
        Ok(st)
    }
    fn online_session(&self) -> Result<(String, Status)> {
        let info = self.session()?;
        let st = self.query_status(&info["sessionId"])?;
        ensure!(st.online, "当前设备离线，请先使用 login 登录");
        Ok((info["sessionId"].clone(), st))
    }
    pub fn logout(&self) -> Result<()> {
        let info = self.session()?;
        if self.query_status(&info["sessionId"])?.online {
            self.post(
                "/eportal/network/offline",
                json!({"sessionId":info["sessionId"]}),
            )?;
        }
        Ok(())
    }
    pub fn login(&self, cas: &Cas, service: &str) -> Result<Status> {
        let info = self.session()?;
        let session = &info["sessionId"];
        let st = self.query_status(session)?;
        if st.online {
            return Ok(st);
        }
        let mut url = Url::parse(&self.endpoint("/cas-sso/login"))?;
        {
            let mut q = url.query_pairs_mut();
            q.extend_pairs([
                ("flowSessionId", session.as_str()),
                ("preview", "false"),
                ("appType", "normal"),
                ("language", "zh-CN"),
            ]);
            q.append_pair("timer", &http::millis().to_string());
            for key in ["customPageId", "mode", "nasIp", "userIp", "ssid"] {
                q.append_pair(key, info.get(key).map(String::as_str).unwrap_or_default());
            }
        }
        self.http.follow(url.as_str())?;
        let r = self
            .http
            .get(&self.endpoint("/cas-sso/clientredirect?client_name=sidadapter"))?;
        let loc = r.location.context("委托认证缺少 CAS 跳转")?;
        ensure!(loc.contains("/authserver/login"), "委托认证未指向 CAS");
        let parsed = Url::parse(&loc)?;
        let service_url = parsed
            .query_pairs()
            .find(|(k, _)| k == "service")
            .context("CAS URL 缺少 service")?
            .1
            .into_owned();
        let ticket = cas.ticket(&service_url)?;
        self.http
            .follow(&http::query(&service_url, &[("ticket", &ticket)])?)?;
        let online = self.post("/eportal/network/userOnline", json!({"sessionId":session}))?;
        if online["online"] != true {
            self.post(
                "/eportal/network/serviceSelection",
                json!({"sessionId":session}),
            )?;
            let result = self.post(
                "/eportal/network/serviceLogin",
                json!({"sessionId":session,"service":login_service(service)}),
            )?;
            if field(&result, "authResult") == "fail" {
                bail!("准入失败: {}", field(&result, "authMessage"));
            }
            ensure!(
                matches!(field(&result, "authResult"), "" | "success"),
                "无法识别准入响应"
            );
            let check = self.post("/eportal/network/userOnline", json!({"sessionId":session}))?;
            ensure!(
                check["online"] == true,
                "认证后仍离线: {} {}",
                field(&check, "message"),
                field(&result, "authMessage")
            );
        }
        let status = self.query_status(session)?;
        ensure!(status.online, "准入后状态仍为离线");
        Ok(status)
    }
}
