use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::{ffi::OsString, path::PathBuf};

pub const VERSION: &str = match option_env!("YSUNETHELPER_VERSION") {
    Some(v) => v,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const EXAMPLES: &str = "命令用法（兼容 Go 版）:
  ysunethelper [-config 路径] status [-v] [-test] [--json]
  ysunethelper [-config 路径] login [-u 账号] [-p 密码] [-s 运营商] [--json]
  ysunethelper [-config 路径] logout [--json]
  ysunethelper [-config 路径] switch [--json] <编号|名称|英文别名>
  ysunethelper [-config 路径] tui
  ysunethelper [-config 路径] devices [--json]
  ysunethelper [-config 路径] kick [--json] <序号|UUID>...
  ysunethelper [-config 路径] [-v] daemon
  ysunethelper version

--json: status、login、logout、switch、devices、kick 均支持 JSON 输出。
登录示例: ysunethelper login -u 202400114514 -p 'your-password' -s unicom --json
运营商: campus=校园网 / telecom=中国电信 / mobile=中国移动 / unicom=中国联通。
login 的 -s 仅决定本次离线准入；已经在线时不会切换，请使用 switch。
兼容单/双横线参数、=赋值和布尔值（例如 -json=false）；重复参数采用最后一个值。
命令选项请放在位置参数之前。使用 ysunethelper <命令> -h 查看详细帮助。";

#[derive(Debug, Parser)]
#[command(name="ysunethelper", version=VERSION, disable_version_flag=true,
    about="燕山大学校园网认证助手", after_help=EXAMPLES)]
pub struct Cli {
    /// 配置文件路径；兼容 -config，daemon 额外查找 /etc 下的配置
    #[arg(long, global = true, allow_hyphen_values = true, value_name = "路径")]
    pub config: Option<PathBuf>,
    /// 完整状态；daemon 中启用详细日志
    #[arg(short='v',long,global=true,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
    pub verbose: bool,
    /// 显示版本号并退出（也可运行 version）
    #[arg(short='V',long="version",action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
    pub show_version: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 查询在线状态；支持 -v、-test、--json
    Status {
        /// 以 JSON 输出状态；与 -v 组合时包含完整原始字段
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        json: bool,
        /// 额外检测 Internet；在状态后追加探测结果和在线状态文本
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        test: bool,
    },
    /// 登录：-u 账号 -p 密码 -s 运营商；支持 --json
    #[command(
        after_help = "示例: ysunethelper login -u 202400114514 -p 'your-password' -s unicom --json\n-s 支持 campus/unicom/telecom/mobile 或服务全名。参数不写入配置文件。\n已经在线时不会切换运营商，请使用 switch。"
    )]
    Login {
        /// 统一身份认证账号（兼容 -username、--u、--username）
        #[arg(short = 'u', long, allow_hyphen_values = true, value_name = "账号")]
        username: Option<String>,
        /// 统一身份认证密码（兼容 -password、--p、--password）
        #[arg(short = 'p', long, allow_hyphen_values = true, value_name = "密码")]
        password: Option<String>,
        /// campus/unicom/telecom/mobile 或服务全名（兼容 -service、--s）
        #[arg(short = 's', long, allow_hyphen_values = true, value_name = "运营商")]
        service: Option<String>,
        /// 以 JSON 输出登录结果
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        json: bool,
    },
    /// 当前设备下线；支持 --json
    Logout {
        /// 以 JSON 输出登出结果
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        json: bool,
    },
    /// 免密码切换运营商；支持 --json（0 校园网 / 1 电信 / 2 移动 / 3 联通）
    Switch {
        /// 以 JSON 输出切换结果
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        json: bool,
        #[arg(value_parser=nonblank)]
        service: String,
    },
    /// 兼容串口和 TTY 的运营商菜单
    Tui,
    /// 免密码查询在线设备；支持 --json
    Devices {
        /// 以 JSON 输出在线设备和原始数据
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        json: bool,
    },
    /// 下线设备（序号从 1 开始，或 UUID）；支持 --json
    Kick {
        /// 以 JSON 输出已提交下线请求的设备
        #[arg(long,action=ArgAction::Set,num_args=0..=1,require_equals=true,default_missing_value="true",default_value="false",value_parser=go_bool)]
        json: bool,
        #[arg(required=true,num_args=1..)]
        targets: Vec<String>,
    },
    /// 前台自动保持在线；使用 -v 输出详细日志
    Daemon,
    /// 显示版本号
    Version,
}
fn nonblank(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("运营商不能为空".into())
    } else {
        Ok(value.trim().into())
    }
}
fn go_bool(value: &str) -> Result<bool, String> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err("布尔值必须为 true/false（也兼容 Go 的 1/0、t/f）".into()),
    }
}
/// Normalize Go flag spellings without touching option values. Like flag.FlagSet,
/// parsing stops at the first positional argument, so `kick 1 --json` must never
/// turn into a destructive request for just device 1.
fn normalize(args: impl IntoIterator<Item = OsString>) -> (Vec<OsString>, bool) {
    let mut out = Vec::new();
    let mut args = args.into_iter();
    out.push(args.next().unwrap_or_else(|| "ysunethelper".into()));
    let mut needs_value = false;
    let mut command = false;
    let mut options = true;
    let mut global_verbose = false;
    let mut show_version = false;
    for arg in args {
        if needs_value {
            out.push(arg);
            needs_value = false;
            continue;
        }
        if arg == "--" && options {
            options = false;
            if command {
                out.push(arg);
            }
            continue;
        }
        let text = arg.to_str().unwrap_or("");
        if options && text.starts_with('-') && text != "-" {
            let (key, assigned) = text
                .trim_start_matches('-')
                .split_once('=')
                .map_or((text.trim_start_matches('-'), None), |(k, v)| (k, Some(v)));
            let canonical = match key {
                "config" => Some("config"),
                "u" | "username" => Some("username"),
                "p" | "password" => Some("password"),
                "s" | "service" => Some("service"),
                "v" | "verbose" => Some("verbose"),
                "json" => Some("json"),
                "test" => Some("test"),
                "version" | "V" => Some("version"),
                "help" | "h" => Some("help"),
                _ => None,
            };
            if let Some(name) = canonical {
                if !command && name == "verbose" {
                    global_verbose = assigned
                        .map(go_bool)
                        .transpose()
                        .ok()
                        .flatten()
                        .unwrap_or(true);
                }
                if !command && name == "version" {
                    show_version = assigned
                        .map(go_bool)
                        .transpose()
                        .ok()
                        .flatten()
                        .unwrap_or(true);
                }
                needs_value = assigned.is_none()
                    && matches!(name, "config" | "username" | "password" | "service");
                out.push(if let Some(value) = assigned {
                    format!("--{name}={value}").into()
                } else {
                    format!("--{name}").into()
                });
            } else {
                out.push(arg);
            }
        } else if !command {
            if show_version {
                break;
            }
            command = true;
            options = true;
            out.push(arg);
        } else {
            if options {
                out.push("--".into());
                options = false;
            }
            out.push(arg);
        }
    }
    (out, global_verbose)
}
pub fn try_parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli, clap::Error> {
    let (args, global_verbose) = normalize(args);
    let mut command = Cli::command().args_override_self(true);
    command = command.mut_subcommands(|sub| sub.args_override_self(true));
    let matches = command.try_get_matches_from(args)?;
    let mut cli = Cli::from_arg_matches(&matches)?;
    cli.verbose |= global_verbose;
    Ok(cli)
}
pub fn parse() -> Cli {
    try_parse(std::env::args_os()).unwrap_or_else(|e| e.exit())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Cli {
        try_parse(
            std::iter::once("ysunethelper")
                .chain(args.iter().copied())
                .map(OsString::from),
        )
        .unwrap()
    }
    #[test]
    fn every_go_login_spelling_keeps_credentials_and_service() {
        for flags in [
            ["-u", "-p", "-s"],
            ["--u", "--p", "--s"],
            ["-username", "-password", "-service"],
            ["--username", "--password", "--service"],
        ] {
            for service in [
                "campus",
                "unicom",
                "telecom",
                "mobile",
                "校园网",
                "中国联通",
            ] {
                let cli = parse(&[
                    "login", flags[0], "student", flags[1], "-json", flags[2], service, "--json",
                ]);
                let Some(Command::Login {
                    username,
                    password,
                    service: got,
                    json,
                }) = cli.command
                else {
                    panic!()
                };
                assert_eq!(username.as_deref(), Some("student"));
                assert_eq!(password.as_deref(), Some("-json"));
                assert_eq!(got.as_deref(), Some(service));
                assert!(json);
            }
        }
    }
    #[test]
    fn assignments_boolean_values_and_repeated_flags() {
        let cli = parse(&[
            "-config=a.json",
            "--config=b.json",
            "login",
            "-u=old",
            "--username=new",
            "-p=-secret",
            "-s=unicom",
            "-json=true",
            "--json=false",
        ]);
        assert_eq!(cli.config, Some("b.json".into()));
        let Some(Command::Login {
            username,
            password,
            service,
            json,
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(username.as_deref(), Some("new"));
        assert_eq!(password.as_deref(), Some("-secret"));
        assert_eq!(service.as_deref(), Some("unicom"));
        assert!(!json);
        for value in [
            "1", "t", "T", "TRUE", "true", "True", "0", "f", "F", "FALSE", "false", "False",
        ] {
            let cli = parse(&[
                "status",
                &format!("--json={value}"),
                &format!("-test={value}"),
            ]);
            assert!(
                matches!(cli.command,Some(Command::Status{json,test}) if json==go_bool(value).unwrap() && test==json)
            );
        }
        assert!(parse(&["-v", "status", "-v=false"]).verbose);
        assert!(!parse(&["-v", "-v=false", "status", "-v=false"]).verbose);
        assert!(!parse(&["-version=false", "status"]).show_version);
        assert!(parse(&["-version", "ignored-command", "--unknown"]).show_version);
    }
    #[test]
    fn positional_arguments_stop_option_parsing() {
        let cli = parse(&["kick", "--json", "1", "--json=false"]);
        assert!(
            matches!(cli.command,Some(Command::Kick{json:true,targets}) if targets==["1","--json=false"])
        );
        let cli = parse(&["--", "switch", "--json", "3"]);
        assert!(matches!(cli.command,Some(Command::Switch{json:true,service}) if service=="3"));
        let cli = parse(&["kick", "--", "--json"]);
        assert!(
            matches!(cli.command,Some(Command::Kick{json:false,targets}) if targets==["--json"])
        );
    }
}
