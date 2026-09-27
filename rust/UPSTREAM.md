# UPSTREAM.md —— 上游来源、依赖方式与许可

GuoSSHell 直接使用官方 rsHell 仓库的内核，通过 git 依赖固定到精确提交。
`native/hub` 经 `rshell-m0` 的再导出使用这些类型，依赖来源统一在
`rust/Cargo.toml` 声明，解析结果记录在根 `Cargo.lock`。

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/hugefiver/rsHell |
| 固定提交 | `718d9b62a8f062af8f5787b5fc27f6c5bbb4f268` |
| 分支快照 | `master`，2026-09-27 |
| 认证与终端接口合并提交 | `d71e81c5b7e786d3981e12d6a24a296ff74d3dae`（上游 PR #1） |
| 许可证 | MIT，Copyright (c) 2026 hugefiver（副本见 `LICENSES/rsHell-MIT.txt`） |

```toml
rshell-core = { git = "https://github.com/hugefiver/rsHell", rev = "718d9b62a8f062af8f5787b5fc27f6c5bbb4f268" }
rshell-session = { git = "https://github.com/hugefiver/rsHell", rev = "718d9b62a8f062af8f5787b5fc27f6c5bbb4f268" }
rshell-storage = { git = "https://github.com/hugefiver/rsHell", rev = "718d9b62a8f062af8f5787b5fc27f6c5bbb4f268" }
```

固定提交同时包含短的顶部滚动区域中 `stable_row` 的修复，普通输出和同步输出
共用稳定行号记账。上游后续提交不会自动进入构建；升级时需同时更新 manifest
与根锁文件，并验证内核回归、GuoSSHell 会话与认证测试及 iOS 编译。
全新环境首次构建需要网络；离线构建前需准备好 `cargo fetch --locked` 的缓存。

## GuoSSHell 使用的上游接口

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

## iOS 与 macOS 依赖边界

### keyring 的 iOS `protected` feature

上游 `rshell-storage` 使用 `keyring 4.1.5`。iOS 仅有 protected data store，
GuoSSHell 通过下列目标依赖统一启用所需 feature，无需修改上游 manifest：

```toml
[target.'cfg(target_os = "ios")'.dependencies]
apple-native-keyring-store = { version = "1.0.1", features = ["protected"] }
```

启动时由 `register_credential_store` 注册 protected 存储，
`keyring-core` 与上游 keyring 使用相同版本。

### portable-pty-psmux

`rshell-session` 通过仓库内的 path 依赖引用
`third_party/portable-pty-psmux`；作为 git 依赖使用时，Cargo 从相同 rsHell
提交解析这个包。根 `Cargo.lock` 将其固定到同一个 git 来源，GuoSSHell
无需配置 `[patch.crates-io]`，也不从 crates.io 解析另一份同名包。

这个包基于 `portable-pty-psmux 0.9.6`，上游增加了 Windows Job handle
支持与测试 feature。Windows 专用代码不参与 iOS / macOS 编译。许可与补丁
来源说明保存在 `LICENSES/`，副本与当前固定的上游提交一致。

### iOS 不使用的本地传输

`local` / `pty` / `system_ssh` 传输仍参与 Rust 编译，但 GuoSSHell 的 iOS
会话只调用原生 SSH。最终链接需启用 `-Wl,-dead_strip`
（Xcode 的 `DEAD_CODE_STRIPPING = YES`），以移除未使用的
`_openpty` / `_login_tty` / `_fork` / `_posix_spawnp` 等导入。
编译通过不等同于这些本地传输在 iOS 上可用。

## 许可文件

| 文件 | 内容 |
|---|---|
| `LICENSES/rsHell-MIT.txt` | rsHell 的 MIT 许可（Copyright (c) 2026 hugefiver） |
| `LICENSES/portable-pty-psmux-MIT.md` | portable-pty-psmux 的 MIT 许可（Wez Furlong） |
| `LICENSES/portable-pty-psmux-PATCH-NOTES.md` | 上游 portable-pty-psmux 补丁来源说明副本 |

MIT 许可声明需随软件或其重要部分一起分发；发布时应保留这些第三方许可。
