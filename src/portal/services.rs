use super::*;
use crate::{is_network, shutdown};
use std::time::Instant;
const NAMES: [&str; 4] = ["校园网", "中国电信", "中国移动", "中国联通"];
pub fn resolve_service(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "校园" | "campus" => NAMES[0],
        "1" | "电信" | "telecom" => NAMES[1],
        "2" | "移动" | "mobile" => NAMES[2],
        "3" | "联通" | "unicom" => NAMES[3],
        _ => value.trim(),
    }
    .to_owned()
}
pub fn service_number(value: &str) -> Option<usize> {
    let value = resolve_service(value);
    NAMES.iter().position(|s| *s == value)
}
#[derive(Debug, Deserialize)]
pub struct Service {
    #[serde(default)]
    pub key: String,
    pub value: String,
    pub order: Option<i64>,
}
impl Portal {
    fn session_services(&self, session: &str) -> Result<Vec<Service>> {
        let mut list: Vec<Service> = serde_json::from_value(self.post(
            "/eportal/network/serviceSelection",
            json!({"sessionId":session,"selectType":1}),
        )?)?;
        ensure!(!list.is_empty(), "当前会话没有可切换的服务");
        for service in &mut list {
            ensure!(!service.value.trim().is_empty(), "服务列表包含空 value");
            if service.key.is_empty() {
                service.key = service.value.clone();
            }
        }
        list.sort_by_key(|s| s.order.unwrap_or(i64::MAX));
        Ok(list)
    }
    pub fn services(&self) -> Result<(Status, Vec<Service>)> {
        let (session, st) = self.online_session()?;
        Ok((st, self.session_services(&session)?))
    }
    pub fn switch(&self, target: &str) -> Result<Status> {
        self.switch_with_timeout(target, Duration::from_secs(20), Duration::from_secs(1))
    }
    pub fn switch_with_timeout(
        &self,
        target: &str,
        timeout: Duration,
        interval: Duration,
    ) -> Result<Status> {
        let name = resolve_service(target);
        ensure!(!name.is_empty(), "目标运营商不能为空");
        let (session, st) = self.online_session()?;
        if st.current(&name) {
            return Ok(st);
        }
        let services = self.session_services(&session)?;
        let target = services
            .iter()
            .find(|s| s.key == name || s.value == name)
            .context("当前会话未提供目标服务，请使用 tui 查看")?;
        if st.current(&target.value) {
            return Ok(st);
        }
        let result=self.post("/eportal/network/setService",json!({"sessionId":session,"serviceName":target.value,"rememberService":false,"browserFgr":null}))
            .context("切换请求未确认，请用 status 复查；请勿立即重复提交")?;
        ensure!(
            field(&result, "switchResult") == "success",
            "切换失败（{}）: {}；原连接可能已下线，请用 status 复查",
            field(&result, "failType"),
            field(&result, "message")
        );
        let deadline = Instant::now() + timeout;
        self.http.deadline.set(Some(deadline));
        let outcome = (|| {
            let mut detail = "尚未读到在线状态".to_string();
            while Instant::now() < deadline {
                shutdown::check()?;
                let status = self
                    .session()
                    .and_then(|s| self.query_status(&s["sessionId"]));
                match status {
                    Ok(st) if st.current(&target.value) || st.current(&target.key) => {
                        return Ok(st);
                    }
                    Ok(st) => {
                        detail = if st.online {
                            format!("当前仍为 {}", st.service)
                        } else {
                            "当前离线".into()
                        }
                    }
                    Err(e) => {
                        if !is_network(&e) && Instant::now() < deadline {
                            return Err(e.context("服务端已接受切换，但验证失败，请用 status 复查"));
                        }
                        detail = e.to_string();
                    }
                }
                shutdown::wait(interval.min(deadline.saturating_duration_since(Instant::now())));
            }
            bail!(
                "服务端已接受切换，等待 {} 上线超时（{}），请用 status 复查",
                target.key,
                detail
            )
        })();
        self.http.deadline.set(None);
        outcome
    }
}
