# UPSTREAM.md —— 上游依赖记录

本仓库使用上游依赖的规则见 [AGENTS.md](AGENTS.md#上游依赖2026-09-29-起)。
每个 fork 或 git 固定的上游依赖在此记一节；直接使用 crates.io／pub.dev
发布版且未修改的依赖不单独记录。

## rsHell（Rust 内核，未修改）

GuoSSHell 直接使用官方 rsHell 仓库的内核，通过 git 依赖固定到精确提交。
`native/hub` 经 `rshell-m0` 的再导出使用这些类型，依赖来源统一在
`rust/Cargo.toml` 声明，解析结果记录在根 `Cargo.lock`。

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/hugefiver/rsHell |
| 固定提交 | `718d9b62a8f062af8f5787b5fc27f6c5bbb4f268` |
| 分支快照 | `master`，2026-09-27 |
| 认证与终端接口合并提交 | `d71e81c5b7e786d3981e12d6a24a296ff74d3dae`（上游 PR #1） |
| 许可证 | MIT，Copyright (c) 2026 hugefiver（副本见 `rust/LICENSES/rsHell-MIT.txt`） |

```toml
rshell-core = { git = "https://github.com/hugefiver/rsHell", rev = "718d9b62a8f062af8f5787b5fc27f6c5bbb4f268" }
rshell-session = { git = "https://github.com/hugefiver/rsHell", rev = "718d9b62a8f062af8f5787b5fc27f6c5bbb4f268" }
rshell-storage = { git = "https://github.com/hugefiver/rsHell", rev = "718d9b62a8f062af8f5787b5fc27f6c5bbb4f268" }
```

固定提交同时包含短的顶部滚动区域中 `stable_row` 的修复，普通输出和同步输出
共用稳定行号记账。上游后续提交不会自动进入构建；升级时需同时更新 manifest
与根锁文件，并验证内核回归、GuoSSHell 会话与认证测试及 iOS 编译。
全新环境首次构建需要网络；离线构建前需准备好 `cargo fetch --locked` 的缓存。

### GuoSSHell 使用的上游接口

| 能力 | 上游接口或行为 | GuoSSHell 的职责 |
|---|---|---|
| 粘贴模式 | `TerminalDisplayModes.bracketed_paste` 暴露 DECSET 2004；不计入显示残留 | 根据该状态包装粘贴内容 |
| 未保存的密码 | Password 配置允许没有 `credential_ref` | 连接时询问密码，传入 `AuthPlan::from_secret` |
| 主机密钥变化 | `KnownHostsVerifier::with_changed_key_prompt()` 允许显式确认后替换；拒绝陈旧确认并保留同一行其他端点，默认拒绝变化 | 显示变化警告，把用户确认传回内核 |
| 内存私钥 | `AuthPlan::from_private_key` 使用已解密的私钥，不读取 `identity_file` | 从钥匙串读取并在内存解密 |
| 外部签名 | `ExternalSigner` / `AuthPlan::from_signer` 使用外部签名结果；RSA 摘要按服务器协商 | 实现 OpenPGP 卡或安全密钥签名及交互 |
| 连接时限 | `NativeSshTransport::with_connect_timeout` 独立设置连接全过程的上限 | 管理网络时间预算，并在等待用户交互时暂停计时 |
| 断链检测 | `NativeSshTransport::with_keepalive` 配置间隔与未应答次数 | 配置 keepalive，并处理结束后的界面状态 |
| 同步输出 | `TerminalEngine::sync_deadline()` / `end_sync()` 暴露 DEC 2026 截止时间与结束入口 | 会话循环在无新输出时也调度到期刷新 |

这些接口已在官方提交中提供，GuoSSHell 不再依赖 fork 分支上的补丁。
`with_connect_timeout` 的上限包括用户交互时间，暂停用户等待计时仍由调用方完成。
`sync_deadline` 只提供截止时刻，调用方必须主动调用 `end_sync`。
主机密钥文件中命中的哈希或通配条目若无法安全隔离端点，上游会拒绝替换并保留原文件；
GuoSSHell 自动保存的记录使用精确端点。

### iOS 与 macOS 依赖边界

#### keyring 的 iOS `protected` feature

上游 `rshell-storage` 使用 `keyring 4.1.5`。iOS 仅有 protected data store，
GuoSSHell 通过下列目标依赖统一启用所需 feature，无需修改上游 manifest：

```toml
[target.'cfg(target_os = "ios")'.dependencies]
apple-native-keyring-store = { version = "1.0.1", features = ["protected"] }
```

启动时由 `register_credential_store` 注册 protected 存储，
`keyring-core` 与上游 keyring 使用相同版本。

#### portable-pty-psmux

`rshell-session` 通过仓库内的 path 依赖引用
`third_party/portable-pty-psmux`；作为 git 依赖使用时，Cargo 从相同 rsHell
提交解析这个包。根 `Cargo.lock` 将其固定到同一个 git 来源，GuoSSHell
无需配置 `[patch.crates-io]`，也不从 crates.io 解析另一份同名包。

这个包基于 `portable-pty-psmux 0.9.6`，上游增加了 Windows Job handle
支持与测试 feature。Windows 专用代码不参与 iOS / macOS 编译。许可与补丁
来源说明保存在 `rust/LICENSES/`，副本与当前固定的上游提交一致。

#### iOS 不使用的本地传输

`local` / `pty` / `system_ssh` 传输仍参与 Rust 编译，但 GuoSSHell 的 iOS
会话只调用原生 SSH。最终链接需启用 `-Wl,-dead_strip`
（Xcode 的 `DEAD_CODE_STRIPPING = YES`），以移除未使用的
`_openpty` / `_login_tty` / `_fork` / `_posix_spawnp` 等导入。
编译通过不等同于这些本地传输在 iOS 上可用。

### 许可文件

| 文件 | 内容 |
|---|---|
| `rust/LICENSES/rsHell-MIT.txt` | rsHell 的 MIT 许可（Copyright (c) 2026 hugefiver） |
| `rust/LICENSES/portable-pty-psmux-MIT.md` | portable-pty-psmux 的 MIT 许可（Wez Furlong） |
| `rust/LICENSES/portable-pty-psmux-PATCH-NOTES.md` | 上游 portable-pty-psmux 补丁来源说明副本 |

MIT 许可声明需随软件或其重要部分一起分发；发布时应保留这些第三方许可。

## terminal_view（终端渲染，团队 fork）

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/Termphin/terminal_view（pub.dev `terminal_view` 0.2.0，fork 自 xterm.dart 4.0.0） |
| fork | https://github.com/MoyuTeams/terminal_view ，分支 `guosh/frame-source` |
| 固定提交 | `fb19f119821cf327a3bee4f11d522826b062e54d` |
| 上游基线 | `19c6ebb4bb03898f3d626c588acd0e0a1d92c152`（Termphin `main`） |
| 许可证 | MIT，随包保留上游 `LICENSE` 与 `NOTICE` |

```yaml
terminal_view:
  git:
    url: https://github.com/MoyuTeams/terminal_view.git
    ref: fb19f119821cf327a3bee4f11d522826b062e54d
```

以下改动都在渲染库的控件、输入连接或绘制内部，应用只能通过 `TerminalView`
的公开参数和 `TerminalSurface` 接入，无法从外部替换，因此只能在 fork 中修改。
理由编号对应 AGENTS.md「上游依赖」第 2 条。

| 改动 | fork 提交 | 理由 |
|---|---|---|
| `TerminalSurface`：渲染层只依赖收窄的终端接口，由应用填入 Rust 引擎的帧，包内解析器与缓冲区不参与 | `3b4eb92` | ③ 接缝：渲染对象原先绑定包内 `Terminal` 状态机，终端状态的唯一权威在 Rust 引擎 |
| 选区由嵌入方持有：选区手势只上报意图，高亮按回传坐标绘制；触屏选区手柄作为控件层叠加 | `a3d42d9` | ③ 接缝：选区坐标由引擎计算；② iOS/Android 缺少可拖动的选区手柄 |
| `showSelectionHandles` 参数，默认开启 | `8613ae4` | ③ 接缝：手柄对鼠标选区也显示并遮挡单元格，库未提供关闭入口；应用按实际输入设备控制 |
| 文本输入配置关联当前 `FlutterView.viewId` | `37e2ee4` | ① Windows 引擎拒绝不带 `viewId` 的文本输入连接，文字与输入法提交无法到达终端 |
| iOS 软键盘回车只发送一次 | `21d04c2` | ① 同一次回车同时经动作与文本两路到达，发出两个换行 |
| 用平台 text editing delta 计算编辑；关闭智能标点 | `d6896f0` | ① 快速输入与输入法提交时重复发送字符 |
| 硬件按键按键入顺序发送，超时从最后一次文本回调起算 | `1dec90a`、`0e71e52` | ① 回车、方向键等抢在先前字符之前到达 |
| 撤销 iOS 双空格句号替换 | `70a6d30` | ① 终端收到用户未输入的退格与句号 |
| 一次插入的多行文本按粘贴处理 | `1523ff0` | ① 粘贴内容被逐行执行，无法走 bracketed paste |
| 中键按中键上报 | `e1a439d` | ① 中键被报成右键 |
| 绘制下划线（可与删除线叠加） | `1cb45ce` | ② SGR 下划线被记录但从不绘制，man 参数、vim 拼写错误等信息丢失 |
| 向远端应用上报鼠标拖动、悬停及修饰键 | `742f721` | ② 鼠标跟踪模式下拖动与悬停不上报，TUI 应用无法使用鼠标 |
| 仅用户操作能让视图离开底部 | `a80a70b` | ① 清屏回弹后视图不再跟随输出 |
| 含组合字符的单元格走完整字素绘制 | `f0a1e0d` | ① ASCII 批量绘制丢失组合标记 |
| 同目录文件使用 `package:` 导入 | `fb19f11` | 代码规范，无行为变化 |

Termphin `main` 在上游基线之后的提交尚未并入 fork。升级时先把 fork 变基到新的
上游提交，去掉上游已等价修复的改动，更新本节的固定提交与表格，并运行库测试
及应用的输入、选区、鼠标上报回归。
