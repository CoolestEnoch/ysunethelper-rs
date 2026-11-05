# YSU-NetHelper Rust

*~~燕山大学~~*哈尔滨工业大学秦皇岛校区校园网认证助手的 Rust 实现。支持 CAS 登录、状态查询、登出、免密码切换运营商、在线设备管理，以及自动保持在线的前台守护进程。可复用 Go 版 JSON 配置和 CAS 凭据文件。

[上游](https://github.com/Youwenqwq/ysu-nethelper)

## 使用

```sh
ysunethelper status
ysunethelper status -v --test
ysunethelper status --json
ysunethelper login -u 学号 -p '密码' -s campus
ysunethelper logout
ysunethelper switch unicom
ysunethelper tui
ysunethelper devices --json
ysunethelper kick 2
ysunethelper --config /etc/ysunethelper/config.json daemon
ysunethelper version
```

兼容 Go 版 `-config`、`-test`、`-json`、`-verbose`、`-version`，以及登录参数 `-u` / `-username` / `--u` / `--username`（密码、运营商同样支持对应写法）。支持 `-u=账号`、`--json=false` 等等号赋值及 Go 布尔值形式；重复参数以最后一个值为准。`-v` 和 `--config` 可放在子命令前后。所有查询和变更命令（除菜单外）支持 `--json`；`status --json --test` 与 Go 版一致：先输出 JSON，再追加探针结果和 `status: ONLINE/OFFLINE (portal)` 文本。供 JSON 解析器读取时请单独使用 `status --json`。错误信息和登录提示写到 stderr。命令选项必须放在位置参数前，例如 `kick --json 1`；`kick 1 --json` 会将 `--json` 视为下线目标并报错，不会提交部分目标。

服务编号固定为 `0` 校园网 / `1` 中国电信 / `2` 中国移动 / `3` 中国联通；也接受 `campus`、`telecom`、`mobile`、`unicom` 和中文简称。切换使用在线会话，只提交一次请求并最多等待 20 秒验证目标服务；失败后请用 `status` 复查。切换不修改配置，daemon 重连仍使用配置中的服务。

`tui` 使用逐行菜单，兼容普通 TTY、SSH、串口和无鼠标终端；`r` 刷新、`q` 退出。`login` 缺少密码时在终端中隐藏输入，非交互环境需要配置或 `-u/-p`。命令行密码可能出现在进程列表中，长期使用建议通过私有配置文件提供。

`devices` 和 `kick` 不读取 CAS 凭据、不索取密码。设备编号从 1 开始，也可直接输入 UUID；所有目标会先校验并去重，再一次提交。成功表示下线请求被接受，可通过 `devices` 复查。

## 配置和部署

配置路径：显式 `--config` → 当前目录 `ysunethelper.json` → `~/.config/ysunethelper/config.json`。仅 daemon 额外查找 `/etc/ysunethelper/config.json`。Windows 用户目录使用 `USERPROFILE` 回退。

CLI 显式指定的配置文件不存在时，与 Go 版一样使用默认配置；已有文件格式损坏仍会报错。daemon 未找到配置会生成当前目录模板（显式路径则在该路径生成），然后退出，等待填写账号密码。模板见 [src/config/template.json](src/config/template.json)。已有配置不会被覆盖。Unix 配置和凭据文件以 `0600` 创建。CAS 默认缓存为 `~/.config/ysunethelper/cas.json`；切换账号时会丢弃其他账号的缓存。

配置支持 `username`、`password`、`service`、`credential_path`、`http_timeout` 及模板中的全部 `daemon` 参数。时长用 `60s`、`5m`、`1m30s` 等形式。`daemon.probe_urls` 可自定义 204 探针列表。

daemon 使用并行 HTTP 204 探针，不跟随重定向；连续确认断网后再查询门户，处理离线或认证假死，并使用指数退避。密码错误、验证码和 MFA 要求立即停止，退出码 `78`；无法识别的 CAS 登录错误连续三次也会停止。网关临时封禁或多次准入后依旧断网使用长退避。

`daemon.no_auth_period` 默认关闭。启用后按北京时间暂停检测、登出和认证，不主动下线。`weekdays` 指时段开始日（0 周日至 6 周六），默认周日至周四 23:30 至次日 05:30；含开始、不含结束。

Ctrl+C / SIGTERM 可打断用户名和密码输入、菜单、网络请求及 daemon 等待，以退出码 `130` 结束。取消密码输入会恢复终端回显；网络请求无需等待 `http_timeout` 到期。普通请求仍受配置的超时限制。收到取消后不再发起后续认证请求或保存新凭据。

HTTP 请求支持系统代理环境变量并正常校验 TLS 证书。路由器需要可信 CA 证书和正确的系统时间。

[deploy/](deploy/) 提供 systemd、OpenRC 和 OpenWrt procd 模板。二进制名称均为 `ysunethelper`。systemd 在退出码 78 时不会重启；procd 模板不自动 respawn，以免不可恢复的账密错误导致重复登录。修正配置后手动重启服务。

## 构建

```sh
cargo build --locked --release
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
# 可选：本地 HTTPS 模拟门户测试，需要 Python 3 和 openssl 命令
python3 tests/embedded_smoke.py target/release/ysunethelper
# CLI 兼容性回归；指定 --go 可与已有 Go 可执行文件逐项对照
python3 tests/compatibility.py --rust target/release/ysunethelper
./target/release/ysunethelper --help
```

Rust 2024 edition，最低版本 1.88。发行构建启用体积优化、LTO、单个 codegen unit、panic abort 和符号剥离。本机 Linux 默认内置 OpenSSL；要使用系统开发库构建，可运行 `OPENSSL_NO_VENDOR=1 cargo build --release`。版本默认 `0.1.0-dev`，发布时通过 `YSUNETHELPER_VERSION` 注入 tag。

| 平台 | 架构 | 链接方式 |
| --- | --- | --- |
| Windows | x86 / x86_64 / ARM64 | 静态 MSVC CRT，系统 DLL |
| macOS | x86_64 / ARM64 | 系统框架 |
| Linux | x86 / x86_64 / ARMv7 / ARM64 / RISC-V64 / MIPS little-endian soft-float | 静态 musl 和 OpenSSL |
| Android | ARMv7 / ARM64 / x86 / x86_64 | 静态原生命令行程序，API 23+ |
| Android（实验） | RISC-V64 | NDK r27 预览 ABI，API 35 |
| OpenWrt | MIPS32r2 / mipsel_24kc / MT7621 soft-float | 动态 musl 和 OpenSSL 3 |

Linux 构建使用 `cross` 和 [Cross.toml](Cross.toml)；MIPS 与 Android RISC-V 使用 nightly 的 `build-std`。Android 不是 APK，构建脚本在正式编译前验证 NDK 链接能力。

OpenWrt 动态版使用 [scripts/build-openwrt.sh](scripts/build-openwrt.sh) 中校验 SHA256 的 24.10.4 SDK，动态加载器为 `/lib/ld-musl-mipsel-sf.so.1`，依赖 `libc`、`libgcc`、`libopenssl3` 和 CA 证书。面向 MT7621；ImmortalWrt 25.12-SNAPSHOT 需存在相同加载器和 OpenSSL 3 SONAME。若系统缺少这些库，安装依赖带来的总空间可能超过节省的二进制空间。静态版不依赖系统 OpenSSL。

## 发布

上传仓库后，普通分支 push 和 pull request 会运行 `Checks`：在 Linux、Windows、macOS 上检查格式、Clippy 和 Rust 测试；Linux 额外运行本地 HTTPS 与 CLI 兼容性测试，并校验 workflow 语法及发布打包逻辑。

发布使用 [Static and MT7621 releases](.github/workflows/release.yml)。提交并推送代码后，再推送 `v*` tag 即可触发全部 17 个目标：

```sh
git tag v0.1.0
git push origin v0.1.0
```

也可在 GitHub 仓库的 Actions → Static and MT7621 releases → Run workflow 中填写已存在的 tag。手动运行同样检出该 tag 的代码。工作流使用仓库自带的 `GITHUB_TOKEN` 上传 Release，无需另配 PAT。

MT7621 对应的两个下载文件是：

- `ysunethelper-linux-mipsle-softfloat.tar.gz`：完全静态链接 musl 和 OpenSSL。
- `ysunethelper-openwrt-mipsel_24kc.tar.gz`：MT7621 动态版，使用系统 musl、OpenSSL 3 和 libgcc。

两种 MIPS 产物都会检查架构、软浮点 ABI、加载器及动态依赖，再用 QEMU 24Kc 执行版本号、HTTPS 模拟门户和 63 组 CLI 兼容性检查。静态 Linux x86_64 也会执行版本查询，确认容器中的程序带入了正确的 tag。

工作流先创建草稿 Release，各架构验证后直接上传压缩包及 JSON 清单，包内包含二进制、README、LICENSE 和配置模板。全部成功后校验 17 个目标的清单、tag 和文件哈希，生成 `SHA256SUMS` 并发布。不使用 Actions artifact 中转，也不要求 release-notes Markdown；说明在 Releases 页面手动填写，重跑保留现有说明。任一构建失败时新 Release 保持草稿，修复后可重跑失败任务。

工作流默认使用 `contents: read`，创建、上传和发布 Release 的任务单独申请 `contents: write`。同一 tag 的发布串行运行。nightly 只用于需要 `build-std` 的目标，其余目标使用 stable。

## 目录

- `src/cas/`：统一身份认证、加密、HTML 表单、会话缓存
- `src/portal/`：网络准入、状态、运营商切换、设备管理
- `src/config/`：配置解析、私有文件、禁认证时段
- `src/ui/`：终端输出和逐行交互菜单
- `src/http.rs`、`src/probe.rs`、`src/daemon.rs`：HTTP、探测和自动重连
- `tests/`：本地模拟服务与协议回归测试，不访问真实账号
- `scripts/`、`.github/workflows/`、`deploy/`：构建发布与服务部署

协议移植自同作者的 Go 项目 YSU-NetHelper；参考 [ysu-sdk](https://github.com/Youwenqwq/ysu-sdk) 和 [YSUNetLoginv2](https://github.com/KamijoToma/YSUNetLoginv2)。MIT License。
