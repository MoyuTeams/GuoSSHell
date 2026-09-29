# GuoSSHell

一个 **纯 SSH 客户端**，覆盖 iPhone / iPad、macOS、Windows、Linux 与 Android。
业务与终端语义跑在 **Rust**
（上游 [rsHell](https://github.com/hugefiver/rsHell) 的内核，直接作为固定提交的 git 依赖），
只有渲染与交互用 **Flutter** 重写。Dart 不写业务逻辑。

> **实现期的唯一参考是 [`PLAN.md`](PLAN.md)。** 动代码之前先读它。

## 目录

```
GuoSSHell/
├── PLAN.md              实现规划：铁律 / 架构 / 实测事实 / 性能预算 / M0–M6 / 复用清单 / 陷阱
├── docs/                调研报告、验收文档、followups/（「存在但不马上修」的问题）
├── lib/                 Flutter app（纯展示与交互，无终端状态）
│   ├── main.dart        入口：initializeRust + 许可登记 + MaterialApp
│   └── src/
│       ├── bindings/    rinf gen 生成的 Dart 绑定（**不手改**；rinf 默认不入库）
│       ├── catalog/     连接目录与编辑
│       ├── desktop/     Windows Fluent 工作区、自绘标题栏与窗口材质
│       ├── keys/        私钥、OpenPGP 卡、安全密钥
│       ├── settings/    设置与开源许可页
│       ├── terminal/    帧解码与帧驱动的终端适配器、窗格、键位条、交互对话框
│       └── workspace/   标签与分屏
├── .github/             检查、单测、多架构构建与 tag 发版工作流
├── android/             Android 宿主与 Keystore 初始化
├── linux/               Linux GTK 宿主
├── windows/             Windows 宿主
├── native/hub/          rinf 的 Rust 侧：信号层、会话、连接与认证、钥匙串、卡、安全密钥
│   └── src/             lib.rs · signals/ · session.rs · connect.rs · keys.rs · card/ · security_key/ …
├── rust/                rshell-m0：上游内核的 pin 点（再导出 rshell-core/session）+ M0 探针与示例
│   ├── Cargo.toml       上游走 git 依赖，rev 只在这里 pin 这一处
│   └── examples/        m0.rs · m0_loopback.rs · bench_frame.rs · demo_server.rs
├── Cargo.toml           根 workspace（members = native/*, rust）+ release profile
├── UPSTREAM.md          上游依赖的来源、固定版本与改动记录（规则见 AGENTS.md）
├── ios/                 Flutter 的 iOS 宿主（Runner；PrivacyInfo.xcprivacy）
│   └── RunnerUITests/   XCUITest：iPad 键鼠、后台恢复与真实旋转；真机使用 profile（M6）
├── macos/               Flutter 的 macOS 宿主（沙箱 entitlements）
├── integration_test/    M6 集成测试：终端协议、全屏 TUI、coding agent（m6_harness.dart 是公共部分）
├── test_driver/         flutter drive 的驱动：截图与报告写进 build/m6/
├── assets/              内置字体、Rust 依赖许可清单（许可页用）、应用图标源文件（icon/）
├── about.toml / .hbs    cargo-about 配置与输出模板（scripts/licenses.sh）
└── scripts/
    ├── ci/              版本校验、构建打包、签名文件管理与 Release 门禁
    ├── setup.sh         幂等环境准备
    ├── sshd-test.sh     本地验收 SSH 服务器（Docker：vim / htop / 主机密钥变更 / 私钥与安全密钥授权）
    ├── sshd-test/m6/    M6 验收服务器的内容：假 AI 上游（aimock + 剧本）、m6-* 辅助程序、agent 配置
    ├── m6.sh            M6 编排：模拟器矩阵、真机、集成测试、XCUITest、macOS profile、报告汇总
    ├── m6-report.py     把 M6 的测试报告汇总成 markdown
    ├── m1bar.sh         60Hz 帧率自检进度条
    ├── icons.py         从 assets/icon/ 重新生成各平台应用图标
    └── licenses.sh      重新生成许可页里的 Rust 依赖许可
```

整个 Rust 侧是**一个 workspace**（根 `Cargo.toml`），`native/hub` 通过 `rshell-m0`
的再导出拿上游类型，`Cargo.lock` 只有根这一个，上游 rev 也只 pin 一处。
（M0b 时代的独立 workspace 与 `ios-host/` Swift 壳已在 M1 全绿后退役，见 PLAN §13。）

## 快速开始

```bash
./scripts/setup.sh

# M0a：SSH 端到端，不需要外部服务器、不需要 Xcode、不需要真机
cd rust && cargo run --example m0_loopback

# 打真实服务器
cargo run --example m0 -- <host> <port> <user> <password>

# 性能基准（PLAN.md §4 的全部数字）
cargo run --release --example bench_frame

# M1：起一个无凭证的环回 SSH 服务器（probe / probe，密钥稳定不换）
cargo run --example demo_server -- 2222

# 或者：真实 sshd（Docker，probe / probe，127.0.0.1:2223，带 vim / htop / m1bar）
../scripts/sshd-test.sh up

# App（另开一个终端；iOS 模拟器与 macOS 都可达宿主 127.0.0.1）
cd ..
flutter pub get
rinf gen                      # 改过 native/hub 的信号结构后要重跑
flutter run -d <模拟器id 或 macos> \
  --dart-define=GUOSH_HOST=127.0.0.1 --dart-define=GUOSH_PORT=2222 \
  --dart-define=GUOSH_USER=probe --dart-define=GUOSH_PASS=probe
# 不带 GUOSH_* 时进入连接列表；带上则（debug 构建）启动后直接以快速连接打开终端，
# GUOSH_PASS 可省略（连接时询问）；
# GUOSH_CMD=top 可选——exec 模式（连上直接执行命令，M1 帧率实测用，无需键盘）。
# 60fps 视觉自检：scp scripts/m1bar.sh 到远端后，GUOSH_CMD="bash /tmp/m1bar.sh"
# 同名的进程环境变量也认（没有 --dart-define 时）：XCUITest 的 launchEnvironment、
# simctl launch 的 SIMCTL_CHILD_GUOSH_* 都走这条。
```

## 持续集成与发版

PR 与普通分支只做静态检查；`main`、`dev` 执行单元测试及多平台应用构建，下载产物见 Actions Artifacts。
推送 `vX.Y.Z` 或 `vX.Y.Z-rc.1` 标签，提交已包含在发布仓库的 `main` 或 `dev` 且完整矩阵通过后发布 GitHub Release。
Android 发布密钥仅存放于受分支与标签规则限制的 Environment；平台架构、签名状态和发版命令见 [CI 与 GitHub Release](docs/ci-and-release.md)。

Linux 需要 Secret Service；Windows 私钥使用 DPAPI，Android 使用 Keystore。
新增平台的硬件认证与实体设备验收范围见 [跟进条目](docs/followups/20260927_新增平台的硬件认证与真机验收.md)。

## 缩放与功能按钮

Windows 的 Fluent 工作区、沉浸标题栏、桌面快捷键和亚克力设置见 [Windows 桌面界面](docs/windows-desktop.md)。其他平台保留各自的导航与窗口行为。

终端支持屏幕或触控板双指捏合；Apple 平台使用 `⌘+` / `⌘−` / `⌘0` 缩放和恢复字号，Windows 使用 `Ctrl` 对应组合。
下方编辑图标打开可视化双排预览：直接拖动排序、跨排移动、从目录拖入按钮或拖出删除，保存后立即生效。
按钮等宽、无边框圆角，默认方向键为倒 T 形；支持空位、自定义文本和恢复默认。
使用说明见 [终端输入、缩放与功能按钮](docs/terminal-input-and-controls.md)。

## 真机验收

真机的覆盖、结果、限制与复测命令见 [真机 E2E 补充验收](docs/acceptance-device-2026-09-27.md)。
官方 rsHell 兼容性与审查修复回归见 [PR 审查修复验收](docs/acceptance-review-2026-09-27.md)。
签名覆盖、设备标识与测试地址只保存在本机；日志、截图和报告位于 `build/m6/`。

## 界面与终端字体

界面默认 MiSans，终端默认 MesloLGS NF。所有平台均可选择 TTF 文件、预览后应用，并分别恢复内置字体。使用方法、生效范围和许可见 [字体设置](docs/fonts.md)。

## 应用图标

源文件是 `assets/icon/` 下的 `light.svg`（亮色）与 `dark.svg`（暗色）。iOS / iPadOS 18 起按系统外观切换两版，
其余平台使用暗色版：Android、Windows 与 Linux 的图标格式没有外观变体，macOS 26 起的外观变体需要 Icon Composer 格式，
见 [跟进条目](docs/followups/20260928_macOS图标的系统外观变体.md)。
Android 13 起的主题图标使用单色层，按壁纸主题色着色。
修改源文件后运行 `python3 scripts/icons.py` 重新生成各平台图标，需要 `rsvg-convert` 与 ImageMagick。

## 上游与许可

GuoSSHell 以 [MIT 许可](LICENSE) 发布。

直接依赖官方 `hugefiver/rsHell` @ `718d9b62a8f062af8f5787b5fc27f6c5bbb4f268`，MIT。
该提交包含 GuoSSHell 所需的认证、连接与终端接口，以及顶部滚动区域的稳定行号修复。
接口与平台依赖边界见 [`UPSTREAM.md`](UPSTREAM.md)。

终端渲染使用团队 fork `MoyuTeams/terminal_view`（基于 `Termphin/terminal_view`，MIT），
以 git 依赖固定到精确提交，改动与理由同样记录在 `UPSTREAM.md`。

我们只依赖 4 个内核 crate（`rshell-core` / `rshell-session` / `rshell-platform` /
`rshell-storage`），**不用** `rshell-ui`（22,812 行 GTK4/Relm4 界面层，正是要用 Flutter 替掉的那层）。
