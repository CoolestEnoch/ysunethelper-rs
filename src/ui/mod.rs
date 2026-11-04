use anyhow::Result;
use std::io::{self, IsTerminal, Write};
use ysunethelper::{
    portal::{DeviceList, Portal, Status, resolve_service, service_number},
    shutdown,
};
pub fn status(st: &Status, verbose: bool) -> Result<()> {
    println!("在线状态: {}", if st.online { "在线" } else { "离线" });
    for (name, value) in [
        ("姓名", &st.name),
        ("学工号", &st.username),
        ("IP", &st.user_ip),
        ("MAC", &st.user_mac),
        ("运营商", &st.service),
        ("剩余流量", &st.remaining_traffic),
    ] {
        if !value.is_empty() {
            println!("{name}: {value}");
        }
    }
    if verbose {
        if !st.message.is_empty() {
            println!("服务端消息: {}", st.message);
        }
        for (label, value) in [("账户信息", &st.account), ("认证门户原始字段", &st.raw)]
        {
            if value.as_object().is_some_and(|v| !v.is_empty()) {
                println!("{label}: {}", serde_json::to_string_pretty(value)?);
            }
        }
    }
    Ok(())
}
pub fn devices(list: &DeviceList) {
    if list.online.is_empty() {
        println!("当前没有在线设备");
        return;
    }
    println!("在线设备 {} 台:", list.online.len());
    for (i, d) in list.online.iter().enumerate() {
        print!(
            "  [{}] {}{}",
            i + 1,
            if d.name.is_empty() {
                "(未命名)"
            } else {
                &d.name
            },
            if d.current { "（当前设备）" } else { "" }
        );
        for (key, value) in [
            ("ip", &d.ip),
            ("mac", &d.mac),
            ("type", &d.kind),
            ("在线时长", &d.online_duration),
        ] {
            if !value.is_empty() {
                print!("  {key}={value}");
            }
        }
        println!("\n      uuid={}", d.uuid);
    }
}
pub fn menu(portal: &Portal) -> Result<()> {
    anyhow::ensure!(
        io::stdin().is_terminal(),
        "stdin 不是终端；请使用 switch unicom 等显式命令"
    );
    println!(
        "YSU-NetHelper · 运营商切换\n使用当前在线会话，无需密码。切换可能短暂断网；不修改配置和 daemon 重连服务。"
    );
    // One reader thread lets signal handling interrupt a terminal read without raw mode.
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in io::stdin().lock().lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    while !shutdown::stopped() {
        let services = match portal.services() {
            Ok((st, services)) => {
                println!("\n当前运营商: {} / IP: {}", st.service, st.user_ip);
                for s in &services {
                    let number = service_number(&s.key)
                        .or_else(|| service_number(&s.value))
                        .map(|n| format!("{n}. "))
                        .unwrap_or_default();
                    println!(
                        "  {number}{}{}",
                        s.key,
                        if st.current(&s.value) || st.current(&s.key) {
                            "（当前）"
                        } else {
                            ""
                        }
                    );
                }
                services
            }
            Err(e) => {
                eprintln!("读取失败: {e}");
                vec![]
            }
        };
        loop {
            print!("输入编号、名称或英文别名，r 刷新，q 退出: ");
            io::stdout().flush()?;
            let line = loop {
                if shutdown::stopped() {
                    return Ok(());
                }
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(line) => break line?,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    _ => {}
                }
            };
            if line.trim().eq_ignore_ascii_case("q") {
                return Ok(());
            }
            if line.trim().eq_ignore_ascii_case("r") {
                break;
            }
            let name = resolve_service(&line);
            let Some(target) = services.iter().find(|s| s.key == name || s.value == name) else {
                println!("请选择列表中的运营商。");
                continue;
            };
            match portal.switch(&target.value) {
                Ok(st) => println!("当前运营商: {} / IP: {}", st.service, st.user_ip),
                Err(e) => eprintln!("切换失败: {e}"),
            };
            break;
        }
    }
    Ok(())
}
