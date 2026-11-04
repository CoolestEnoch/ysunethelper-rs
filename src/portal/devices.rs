use super::*;
use std::collections::HashSet;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub uuid: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ip: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mac: String,
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub online_duration: String,
    pub current: bool,
}
#[derive(Debug, Serialize)]
pub struct DeviceList {
    pub online: Vec<Device>,
    pub raw: Value,
}
fn uuid(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn mac(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect::<String>()
        .to_ascii_lowercase()
}
pub fn resolve_targets(targets: &[String], devices: &[Device]) -> Result<Vec<Device>> {
    ensure!(!targets.is_empty(), "下线目标不能为空");
    let mut selected = vec![];
    let mut seen = HashSet::new();
    for target in targets {
        let device = if uuid(target) {
            devices
                .iter()
                .find(|d| d.uuid.eq_ignore_ascii_case(target))
                .context("UUID 不在当前账号的在线设备中")?
        } else {
            let index = target
                .parse::<usize>()
                .context("目标必须为设备编号或 UUID")?;
            devices
                .get(index.checked_sub(1).context("设备编号从 1 开始")?)
                .context("设备编号超出范围")?
        };
        if seen.insert(device.uuid.to_ascii_lowercase()) {
            selected.push(device.clone());
        }
    }
    Ok(selected)
}
impl Portal {
    fn session_devices(&self, session: &str, status: &Status) -> Result<DeviceList> {
        let raw = self.post(
            "/eportal/adaptor/devices/findDevice",
            json!({"sessionId":session}),
        )?;
        let list = raw
            .get("onlineDevices")
            .context("设备响应缺少 onlineDevices")?;
        let empty = vec![];
        let items = if list.is_null() {
            &empty
        } else {
            list.as_array().context("onlineDevices 不是数组")?
        };
        let mut online = vec![];
        let mut seen = HashSet::new();
        for item in items {
            let id = field(item, "onlineUserUuid");
            ensure!(
                uuid(id) && seen.insert(id.to_ascii_lowercase()),
                "设备响应包含无效或重复 UUID"
            );
            let ip = field(item, "userIpv4");
            let address = field(item, "userMac");
            online.push(Device {
                uuid: id.into(),
                name: field(item, "deviceName").into(),
                ip: ip.into(),
                mac: address.into(),
                kind: field(item, "deviceType").into(),
                auth_type: field(item, "authType").into(),
                online_duration: field(item, "onlineDuration").into(),
                current: item["currentDevice"] == true
                    || !ip.is_empty() && ip == status.user_ip
                    || !address.is_empty() && mac(address) == mac(&status.user_mac),
            });
        }
        Ok(DeviceList { online, raw })
    }
    pub fn devices(&self) -> Result<DeviceList> {
        let (session, status) = self.online_session()?;
        self.session_devices(&session, &status)
    }
    pub fn kick(&self, targets: &[String]) -> Result<Vec<Device>> {
        ensure!(!targets.is_empty(), "下线目标不能为空");
        let (session, status) = self.online_session()?;
        let list = self.session_devices(&session, &status)?;
        let selected = resolve_targets(targets, &list.online)?;
        let ids: Vec<_> = selected.iter().map(|d| d.uuid.as_str()).collect();
        let env = self
            .post_envelope(
                "/eportal/adaptor/kick-offline/batch",
                json!({"sessionId":session,"onlineUserUuids":ids}),
            )
            .context("下线请求未确认，请用 devices 复查")?;
        ensure!(
            field(&env, "message").eq_ignore_ascii_case("ok"),
            "下线请求未被明确确认，请用 devices 复查"
        );
        Ok(selected)
    }
}
