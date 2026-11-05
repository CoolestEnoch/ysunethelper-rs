use serde_json::json;
use std::time::Duration;
use ysunethelper::{
    Failure,
    cas::{crypto, parser},
    config::{Config, NoAuthPeriod, parse_duration, write_private},
    daemon::{Action, RetryPolicy},
    http::{Cookies, path_match},
    portal::{Device, resolve_service, resolve_targets},
};
#[test]
fn cas_cipher_matches_go_fixture() {
    assert_eq!(
        crypto::encrypt_with(
            "Test@Password123",
            "Ab3dFghJkmNprstX",
            b"2345678ABCDEFGHJKMNPQRSTWXYZabcdefhijkmnprstwxyz2345678ABCDEFGAB",
            b"WXYZabcdefhijkmn"
        )
        .unwrap(),
        "+7RllxbRpbD7XwM7NmfkD+fe1CNHm9JxnGbIVl2fIbg0/94V1/hwJGAeytLK0toXlGDtAdEmVmXtSvxSH9OsSIHkD4xZx9q44vGGQdKLYC2F8gK08wvnOmZ1JcyncckK"
    );
    assert!(crypto::encrypt("pw", "bad").is_err());
    for key in [
        "0123456789abcdef",
        "0123456789abcdef01234567",
        "0123456789abcdef0123456789abcdef",
    ] {
        assert_ne!(
            crypto::encrypt("pw", key).unwrap(),
            crypto::encrypt("pw", key).unwrap()
        );
    }
}
#[test]
fn cas_forms_are_scoped_and_entities_decoded() {
    let html = r#"<form><input type='hidden' name='execution' value='wrong'></form><form><input name='cllt' value='userNameLogin'><input TYPE='hidden' name='execution' value='a&amp;b'><input type='hidden' id='pwdEncryptSalt' value='1234567890123456'></form>"#;
    let fields = parser::fields(html);
    assert_eq!(fields["execution"], "a&b");
    assert_eq!(fields["pwdEncryptSalt"], "1234567890123456");
    assert_eq!(
        parser::error_message("<span class='extra form-errorTip'>密码&amp;错误</span>"),
        "密码&错误"
    );
    assert!(parser::challenge("var needCaptcha = \"true\""));
    assert!(!parser::challenge("var needCaptcha = \"false\""));
}
#[test]
fn cookies_obey_domain_path_secure_expiry_and_order() {
    let mut jar = Cookies::default();
    let url = reqwest::Url::parse("https://example.test/authserver/login").unwrap();
    jar.absorb(&url, "SESSION=root; Path=/; Secure");
    jar.absorb(&url, "SESSION=auth; Path=/authserver; HttpOnly");
    jar.absorb(&url, "bad=secret; Domain=unrelated.test; Path=/");
    assert_eq!(jar.header(&url), "SESSION=auth; SESSION=root");
    assert_eq!(
        jar.header(&reqwest::Url::parse("http://example.test/authserver").unwrap()),
        "SESSION=auth"
    );
    assert_eq!(
        jar.header(&reqwest::Url::parse("https://sub.example.test/").unwrap()),
        ""
    );
    jar.absorb(&url, "SESSION=gone; Path=/authserver; Max-Age=0");
    assert_eq!(jar.header(&url), "SESSION=root");
    assert!(!path_match("/authserver-other", "/authserver"));
    assert!(path_match("/authserver/login", "/authserver"));
}
#[test]
fn beijing_cross_midnight_boundaries() {
    // 2024-01-04 (Thursday) 23:30 Beijing = 1704382200 UTC epoch seconds.
    let p = NoAuthPeriod {
        enabled: true,
        ..Default::default()
    };
    p.validate().unwrap();
    let start = 1704382200;
    let end = start + 6 * 3600;
    assert_eq!(p.next_window(start), Some((start, end)));
    assert_eq!(p.next_window(end - 1), Some((start, end)));
    assert!(p.next_window(end).unwrap().0 > end);
    assert!(p.next_window(start + 2 * 86400).unwrap().0 > start + 2 * 86400);
    assert!(
        NoAuthPeriod {
            enabled: true,
            weekdays: vec![],
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        NoAuthPeriod {
            enabled: true,
            start: "5:30".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}
#[test]
fn configuration_compatibility_and_private_write() {
    assert_eq!(
        parse_duration("1m30.5s").unwrap(),
        Duration::from_millis(90500)
    );
    assert!(parse_duration("-1s").is_err());
    assert!(parse_duration("NaNs").is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    write_private(&path,br#"{"username":"abc","password":"pw","http_timeout":"0","daemon":{"probe_interval":"0s"}}"#,false).unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.http_timeout.0, Duration::from_secs(30));
    assert_eq!(cfg.daemon.probe_interval.0, Duration::from_secs(60));
    cfg.validate_daemon().unwrap();
    assert!(write_private(&path, b"overwrite", false).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn invalid_device_prevents_entire_batch_and_duplicates_collapse() {
    let d:Device=serde_json::from_value(json!({"uuid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"router","ip":"","mac":"","type":"","auth_type":"","online_duration":"","current":true})).unwrap();
    assert!(resolve_targets(&["1".into(), "99".into()], std::slice::from_ref(&d)).is_err());
    assert!(resolve_targets(&["0".into()], std::slice::from_ref(&d)).is_err());
    assert_eq!(
        resolve_targets(&["1".into(), d.uuid.to_uppercase()], &[d])
            .unwrap()
            .len(),
        1
    );
    assert_eq!(resolve_service("3"), "中国联通");
    assert_eq!(resolve_service(" TELECOM "), "中国电信");
}
#[test]
fn hard_failures_and_login_circuit_breaker() {
    let mut retry = RetryPolicy::default();
    assert_eq!(
        retry.on_error(&Failure::Credentials("bad".into()).into()),
        Action::Stop
    );
    assert_eq!(
        retry.on_error(&Failure::Challenge("captcha".into()).into()),
        Action::Stop
    );
    assert_eq!(
        retry.on_error(&Failure::Blocked.into()),
        Action::LongBackoff
    );
    assert_eq!(
        retry.on_error(&Failure::Login(Failure::Network(anyhow::anyhow!("timeout")).into()).into()),
        Action::NoLink
    );
    assert_eq!(retry.login_failures, 0);
    for i in 1..=3 {
        assert_eq!(
            retry.on_error(&Failure::Login(anyhow::anyhow!("unknown rejection")).into()),
            if i == 3 {
                Action::Stop
            } else {
                Action::Backoff
            }
        );
    }
}

#[test]
fn runtime_shutdown_does_not_wait_for_blocked_dns_workers() {
    use std::{sync::mpsc, time::Instant};
    let runtime = ysunethelper::shutdown::Runtime::new().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    runtime
        .block_on(async {
            // reqwest's system resolver uses the same non-cancellable blocking pool.
            tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(3));
            });
        })
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    drop(runtime);
    let elapsed = start.elapsed();
    let _ = release_tx.send(());
    assert!(
        elapsed < Duration::from_millis(500),
        "runtime waited for a resolver worker: {elapsed:?}"
    );
}
