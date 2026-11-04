use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use tiny_http::{Header, Response, Server, StatusCode};
use ysunethelper::{cas::Cas, http::Http, portal::Portal, probe::Prober};
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}
impl Reply {
    fn json(data: Value) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: json!({"code":200,"message":"ok","data":data}).to_string(),
        }
    }
    fn redirect(url: String) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".into(), url)],
            body: String::new(),
        }
    }
    fn html(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
        }
    }
}
struct Mock {
    base: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Mock {
    fn new(handler: impl Fn(&str, &str, &str) -> Reply + Send + 'static) -> Self {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let join = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                let Some(mut req) = server.recv_timeout(Duration::from_millis(20)).unwrap() else {
                    continue;
                };
                let url = req.url().to_string();
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body).unwrap();
                let cookie = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Cookie"))
                    .map(|h| h.value.as_str())
                    .unwrap_or("");
                let reply = handler(&url, &body, cookie);
                let mut response =
                    Response::from_string(reply.body).with_status_code(StatusCode(reply.status));
                for (key, value) in reply.headers {
                    response.add_header(Header::from_bytes(key, value).unwrap());
                }
                let _ = req.respond(response);
            }
        });
        Self {
            base,
            stop,
            thread: Some(join),
        }
    }
    fn portal(&self) -> Portal {
        Portal::with_base(Duration::from_secs(2), &self.base).unwrap()
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn status(service: &str) -> Value {
    json!({"portalOnlineUserInfo":{"result":"success","userName":"student","realServiceName":service,"userIp":"10.0.0.2","userMac":"aa:bb:cc:dd:ee:ff"}})
}
fn entry(url: &str) -> Option<Reply> {
    if url.starts_with("/eportal/redirect.jsp") {
        Some(Reply::redirect(
            "/portal-main?sessionId=real-session".into(),
        ))
    } else if url.starts_with("/portal-main") {
        Some(Reply::html("portal"))
    } else {
        None
    }
}
#[test]
fn status_uses_real_session_and_preserves_traffic_units() {
    let mock = Mock::new(|url, body, _| {
        if let Some(r) = entry(url) {
            return r;
        }
        if url.starts_with("/eportal/adaptor/getOnlineUserInfo") {
            assert!(url.contains("sessionId=real-session"));
            return Reply::json(status("校园网"));
        }
        assert_eq!(
            serde_json::from_str::<Value>(body).unwrap()["sessionId"],
            "real-session"
        );
        Reply::json(json!({"name":"张三","accountInfo":[{"title":"剩余流量","content":"28.1GB"}]}))
    });
    let st = mock.portal().status().unwrap();
    assert!(st.online);
    assert_eq!(st.name, "张三");
    assert_eq!(st.remaining_traffic, "28.1GB");
    assert!(st.summary().get("raw").is_none());
}
#[test]
fn switch_submits_once_and_waits_through_old_and_offline_states() {
    let changes = Arc::new(Mutex::new((0, 0)));
    let state = changes.clone();
    let mock = Mock::new(move |url, body, _| {
        if let Some(r) = entry(url) {
            return r;
        }
        let mut s = state.lock().unwrap();
        if url.starts_with("/eportal/adaptor/getOnlineUserInfo") {
            if s.0 == 0 {
                return Reply::json(status("校园网"));
            }
            s.1 += 1;
            return Reply::json(match s.1 {
                1 => status("校园网"),
                2 => json!({"portalOnlineUserInfo":{"result":"fail"}}),
                _ => status("中国联通"),
            });
        }
        let b: Value = serde_json::from_str(body).unwrap();
        if url.ends_with("serviceSelection") {
            assert_eq!(b["selectType"], 1);
            return Reply::json(json!([{"key":"中国联通","value":"unicom-internal"}]));
        }
        assert!(url.ends_with("setService"));
        assert_eq!(b["serviceName"], "unicom-internal");
        assert_eq!(b["rememberService"], false);
        assert!(b["browserFgr"].is_null());
        s.0 += 1;
        Reply::json(json!({"switchResult":"success"}))
    });
    let st = mock
        .portal()
        .switch_with_timeout("3", Duration::from_secs(2), Duration::from_millis(1))
        .unwrap();
    assert_eq!(st.service, "中国联通");
    assert_eq!(*changes.lock().unwrap(), (1, 3));
}
#[test]
fn kick_validates_every_target_before_mutation_and_reuses_session() {
    let submissions = Arc::new(Mutex::new(vec![]));
    let state = submissions.clone();
    let mock = Mock::new(move |url, body, _| {
        if let Some(r) = entry(url) {
            return r;
        }
        if url.starts_with("/eportal/adaptor/getOnlineUserInfo") {
            return Reply::json(status("校园网"));
        }
        let b: Value = serde_json::from_str(body).unwrap();
        assert_eq!(b["sessionId"], "real-session");
        if url.ends_with("findDevice") {
            return Reply::json(
                json!({"onlineDevices":[{"onlineUserUuid":"abcdef1234567890abcdef1234567890","deviceName":"router","userIpv4":"10.0.0.2"}]}),
            );
        }
        assert!(url.ends_with("/batch"));
        state.lock().unwrap().push(b);
        Reply::json(Value::Null)
    });
    let portal = mock.portal();
    assert!(portal.kick(&["1".into(), "99".into()]).is_err());
    assert!(submissions.lock().unwrap().is_empty());
    let selected = portal
        .kick(&["1".into(), "ABCDEF1234567890ABCDEF1234567890".into()])
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert!(selected[0].current);
    assert_eq!(
        submissions.lock().unwrap()[0]["onlineUserUuids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn offline_operations_never_post() {
    let mock = Mock::new(|url, body, _| {
        assert!(body.is_empty());
        if let Some(r) = entry(url) {
            return r;
        }
        Reply::json(json!({"portalOnlineUserInfo":{"result":"fail"}}))
    });
    let portal = mock.portal();
    portal.logout().unwrap();
    assert!(portal.devices().is_err());
    assert!(portal.switch("unicom").is_err());
    assert!(portal.kick(&["1".into()]).is_err());
}
#[test]
fn http_redirects_and_cookies_are_manual_and_probe_rejects_redirects() {
    let mock = Mock::new(|url, _, cookie| match url {
        "/start" => Reply {
            headers: vec![
                ("Location".into(), "/js".into()),
                ("Set-Cookie".into(), "SESSION=secret; Path=/".into()),
            ],
            ..Reply::redirect("/js".into())
        },
        "/js" => {
            assert_eq!(cookie, "SESSION=secret");
            Reply::html("<script>location.href='/done?space=a b'</script>")
        }
        "/204" => Reply {
            status: 204,
            ..Reply::html("")
        },
        _ => Reply::html("done"),
    });
    let http = Http::new(Duration::from_secs(2)).unwrap();
    assert_eq!(
        http.get(&format!("{}/start", mock.base)).unwrap().status,
        302
    );
    let (url, body) = http.follow(&format!("{}/start", mock.base)).unwrap();
    assert!(url.ends_with("space=a%20b"));
    assert_eq!(body, "done");
    let prober = Prober::new(
        &[format!("{}/start", mock.base), format!("{}/204", mock.base)],
        Duration::from_secs(1),
    )
    .unwrap();
    let results = prober.check();
    assert!(!results[0].ok);
    assert!(results[1].ok);
    assert!(prober.online());
}
#[test]
fn cas_login_persists_account_and_compatible_cookie_metadata() {
    let mock = Mock::new(|url, body, cookie| {
        if url.starts_with("/authserver/index.do") {
            return if cookie.contains("TGC=valid") {
                Reply::html("ok")
            } else {
                Reply::redirect("/authserver/login".into())
            };
        }
        if body.is_empty() {
            if url.starts_with("/personalInfo/") {
                return Reply::html("ok");
            }
            return Reply::html(
                r#"<form><input name='cllt' value='userNameLogin'><input type='hidden' name='execution' value='e1s1'><input type='hidden' id='pwdEncryptSalt' value='1234567890123456'></form>"#,
            );
        }
        assert!(body.contains("username=student"));
        assert!(body.contains("execution=e1s1"));
        assert!(!body.contains("password=plainpassword"));
        Reply {
            status: 302,
            headers: vec![
                (
                    "Location".into(),
                    "/personalInfo/personCenter/index.html?ticket=ST-1".into(),
                ),
                (
                    "Set-Cookie".into(),
                    "TGC=valid; Path=/authserver; HttpOnly".into(),
                ),
            ],
            body: String::new(),
        }
    });
    let mut cas = Cas::with_base(Duration::from_secs(2), &mock.base).unwrap();
    assert!(!cas.authenticated().unwrap());
    cas.login("student", "plainpassword").unwrap();
    assert!(cas.authenticated().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cas.json");
    cas.save(&path).unwrap();
    let mut restored = Cas::with_base(Duration::from_secs(2), &mock.base).unwrap();
    restored.load(&path).unwrap();
    assert_eq!(restored.username, "student");
    assert!(restored.authenticated().unwrap());
    restored.drop_credential();
    assert!(!restored.authenticated().unwrap());
}
#[test]
fn portal_login_completes_delegation_and_checks_real_admission() {
    let cas_mock = Mock::new(|url, _, _| {
        assert!(url.starts_with("/authserver/login?service="));
        Reply::redirect("http://service.test/callback?ticket=ST-abc%2B123".into())
    });
    let cas = Cas::with_base(Duration::from_secs(2), &cas_mock.base).unwrap();
    let portal_base = Arc::new(Mutex::new(String::new()));
    let base = portal_base.clone();
    let admitted = Arc::new(Mutex::new(false));
    let flag = admitted.clone();
    let cas_base = cas_mock.base.clone();
    let mock = Mock::new(move |url, body, _| {
        if let Some(r) = entry(url) {
            return r;
        }
        if url.starts_with("/eportal/adaptor/getOnlineUserInfo") {
            return Reply::json(if *flag.lock().unwrap() {
                status("校园网")
            } else {
                json!({"portalOnlineUserInfo":{"result":"fail"}})
            });
        }
        if url.starts_with("/cas-sso/login") {
            assert!(url.contains("flowSessionId=real-session"));
            return Reply::html("login");
        }
        if url.starts_with("/cas-sso/clientredirect") {
            let mut location =
                reqwest::Url::parse(&format!("{cas_base}/authserver/login")).unwrap();
            location
                .query_pairs_mut()
                .append_pair("service", &format!("{}/callback", base.lock().unwrap()));
            return Reply::redirect(location.to_string());
        }
        if url.starts_with("/callback") {
            assert!(url.contains("ticket=ST-abc%2B123"));
            return Reply::html("ok");
        }
        if url.ends_with("userOnline") {
            return Reply::json(json!({"online":*flag.lock().unwrap()}));
        }
        if url.ends_with("serviceSelection") {
            return Reply::json(json!([]));
        }
        assert!(url.ends_with("serviceLogin"));
        let b: Value = serde_json::from_str(body).unwrap();
        assert_eq!(b["service"], "校园网");
        *flag.lock().unwrap() = true;
        Reply::json(json!({"authResult":"success"}))
    });
    *portal_base.lock().unwrap() = mock.base.clone();
    assert!(mock.portal().login(&cas, "campus").unwrap().online);
    assert!(*admitted.lock().unwrap());
}

#[test]
fn cas_classifies_hard_rejections_without_retrying_credentials() {
    for (body, expected) in [
        ("<span id='showErrorTip'>密码错误</span>", "credentials"),
        ("var needCaptcha = 'true'", "challenge"),
        ("reAuthCheck", "challenge"),
        ("IP被冻结", "blocked"),
    ] {
        let body = body.to_string();
        let mock = Mock::new(move |_, form, _| {
            if form.is_empty() {
                return Reply::html(
                    r#"<form><input name='cllt' value='userNameLogin'><input type='hidden' name='execution' value='e1s1'><input type='hidden' id='pwdEncryptSalt' value='1234567890123456'></form>"#,
                );
            }
            Reply {
                status: 401,
                ..Reply::html(&body)
            }
        });
        let mut cas = Cas::with_base(Duration::from_secs(1), &mock.base).unwrap();
        let error = cas.login("user", "password").unwrap_err();
        let actual = match error.downcast_ref::<ysunethelper::Failure>().unwrap() {
            ysunethelper::Failure::Credentials(_) => "credentials",
            ysunethelper::Failure::Challenge(_) => "challenge",
            ysunethelper::Failure::Blocked => "blocked",
            _ => panic!("unexpected error"),
        };
        assert_eq!(actual, expected);
    }
}
#[test]
fn successful_envelope_does_not_hide_negative_business_acknowledgement() {
    let mock = Mock::new(|url, body, _| {
        if let Some(r) = entry(url) {
            return r;
        }
        if url.starts_with("/eportal/adaptor/getOnlineUserInfo") {
            return Reply::json(status("校园网"));
        }
        if url.ends_with("findDevice") {
            return Reply::json(
                json!({"onlineDevices":[{"onlineUserUuid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}),
            );
        }
        assert!(!body.is_empty());
        Reply {
            status: 200,
            headers: vec![],
            body: json!({"code":200,"message":"not accepted","data":null}).to_string(),
        }
    });
    assert!(mock.portal().kick(&["1".into()]).is_err());
}
#[test]
fn switch_timeout_does_not_resubmit() {
    let submissions = Arc::new(Mutex::new(0));
    let count = submissions.clone();
    let mock = Mock::new(move |url, _, _| {
        if let Some(r) = entry(url) {
            return r;
        }
        if url.starts_with("/eportal/adaptor/getOnlineUserInfo") {
            return Reply::json(status("校园网"));
        }
        if url.ends_with("serviceSelection") {
            return Reply::json(json!([{"value":"中国联通"}]));
        }
        assert!(url.ends_with("setService"));
        *count.lock().unwrap() += 1;
        Reply::json(json!({"switchResult":"success"}))
    });
    let error = mock
        .portal()
        .switch_with_timeout(
            "unicom",
            Duration::from_millis(50),
            Duration::from_millis(5),
        )
        .unwrap_err();
    assert!(error.to_string().contains("已接受切换"));
    assert_eq!(*submissions.lock().unwrap(), 1);
}
#[test]
fn credential_restore_filters_unrelated_hosts_paths_and_expired_cookies() {
    let mock = Mock::new(|_, _, cookie| {
        assert_eq!(cookie, "TGC=valid");
        Reply::html("ok")
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cas.json");
    std::fs::write(&path,json!({"username":"student","cookies":[
        {"name":"TGC","value":"valid","domain":"","path":"/authserver","secure":false,"expires":null},
        {"name":"foreign","value":"x","domain":"foreign.test","path":"/"},
        {"name":"service","value":"x","domain":"127.0.0.1","path":"/personalInfo"},
        {"name":"expired","value":"x","domain":"127.0.0.1","path":"/","expires":1}
    ]}).to_string()).unwrap();
    let mut cas = Cas::with_base(Duration::from_secs(1), &mock.base).unwrap();
    cas.load(&path).unwrap();
    assert!(cas.authenticated().unwrap());
    assert_eq!(cas.username, "student");
}
