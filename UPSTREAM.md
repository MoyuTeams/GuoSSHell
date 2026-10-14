# UPSTREAM.md —— 上游依赖记录

本仓库使用上游依赖的规则见 [AGENTS.md](AGENTS.md#上游依赖2026-09-29-起)。
每个 fork 或 git 固定的上游依赖在此记一节；直接使用 crates.io／pub.dev
发布版且未修改的依赖不单独记录。

## rsHell（Rust 内核，团队 fork）

GuoSSHell 使用 rsHell 的团队 fork，通过 git 依赖固定到精确提交。
`native/hub` 经 `rshell-m0` 的再导出使用核心配置与会话类型，声明位于
`rust/Cargo.toml`；桌面 PTY／进程平台的应用侧接缝在 `native/hub/Cargo.toml`
直接声明同一提交的依赖。解析结果统一记录在根 `Cargo.lock`，全部 rsHell 包只有
这一个来源。

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/hugefiver/rsHell |
| fork | https://github.com/MoyuTeams/rsHell ，分支 `feat/local-pty-writer` |
| 固定提交 | `b4311370e633c08837d0a2d18fd77ec874305827` |
| 上游基线 | `08d8df5ab975c75b5a9772a4e52fd79695ca9d11`（上游分支 `feat/desktop-config-transactions`） |
| 认证与终端接口合并提交 | `d71e81c5b7e786d3981e12d6a24a296ff74d3dae`（上游 PR #1） |
| 许可证 | MIT，Copyright (c) 2026 hugefiver（副本见 `rust/LICENSES/rsHell-MIT.txt`） |

```toml
rshell-core = { git = "https://github.com/MoyuTeams/rsHell", rev = "b4311370e633c08837d0a2d18fd77ec874305827" }
rshell-session = { git = "https://github.com/MoyuTeams/rsHell", rev = "b4311370e633c08837d0a2d18fd77ec874305827" }
rshell-storage = { git = "https://github.com/MoyuTeams/rsHell", rev = "b4311370e633c08837d0a2d18fd77ec874305827" }
```

桌面宿主直接使用 `portable-pty-psmux =0.9.6`（依赖别名 `portable-pty`）和
`rshell-platform =0.1.0` 的原生 PTY、进程句柄及进程树接口，两者也精确固定到
`b4311370e633c08837d0a2d18fd77ec874305827`。这些接缝用于应用侧分离阻塞
I/O 和取消所有权，不包含第三方源码副本或仓内 path 覆盖。

fork 相对上游基线只改 `rshell-session` 的本地 PTY 输入：`LocalPtyTransport`
的写入交给专用写线程，经 4 MiB 有界队列按块入队；队列满时 `write` 异步等待且
可取消，已入队的块按序写出，关闭时丢弃未写出的部分并保持原有关闭时限。Unix
上 PTY master 为非阻塞描述符，读写线程等待可读/可写（Linux 用 poll，macOS 用
select），写线程另等一条随输入端关闭的唤醒管道，PTY 写满时关闭也能立即回收写
线程。公开 API 不变，附带写线程与非读取子进程的回归测试。属于理由①：原实现在会话
actor 所在的运行时线程上同步写 PTY，子进程不读输入时大段粘贴会阻塞该线程，
会话无法处理关闭等命令；写入发生在传输内部，应用只能拿到 `SessionTransport`
接口，无法在外部改变其写入方式。

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
| 配置事务 | `SqliteRepository::load_configuration()` / `commit_configuration()` 提供配置快照、持久版本、冲突检测和受引用保护的配置档删除 | 桌面设置只在原子提交成功后确认保存，旧移动设置继续保留局部更新语义 |
| 应用元数据 | `TerminalSettingsV1.extensions` 与解析结果按命名空间透传 JSON 数据，旧配置缺省为空且序列化省略；存储沿用完整配置 CAS | 解释字体绑定版本、保护未知载荷，并将绑定与配置档一起原子保存 |
| 阶段化按键 | 独立 `TerminalKeyEvent` 与 `TerminalEngine::encode_key_event` 按真实 Kitty flags 编码逻辑键、Repeat、Release 和 Super；旧输入与默认实现保持兼容 | 提供可靠的布局未移位键、旧适配码与事件阶段；文本和 IME 使用独立提交路径 |

认证与终端接口已在上游合并提交中提供；配置事务与可选元数据接口固定到上游功能分支。
该分支扩展 `rshell-storage`：生产配置快照、SQLite 持久版本、原子条件提交、
配置档删除及引用保护，并让旧保存入口更新版本。此扩展属于理由③：应用无法
通过原有独立保存 API，把设置、配置档和版本检查放进存储 worker 的同一事务，
也无法通过仅供测试的删除入口提供生产删除；应用侧补偿不能保证崩溃安全。
`rshell-core` 的可选 `extensions` 仅保留应用元数据，不解释内容或版本，解析与绑定合并
同样保留；沿用既有 `settings_json`，不新增数据库迁移。此扩展也属于理由③：原固定
模型不能保留应用字段，应用侧独立文件无法让字体绑定参与配置档的同一配置事务。
阶段化按键接口也属于理由③：旧接口只有适配后的键与修饰位，既不能保留未移位
逻辑键和旧表示，也不能表达 Repeat／Release；应用只得到粗粒度显示状态，不能
读取编码时的真实增强位并复用原编码器。新接口不修改旧 `TerminalInput` 或其 serde。
`with_connect_timeout` 的上限包括用户交互时间，暂停用户等待计时仍由调用方完成。
`sync_deadline` 只提供截止时刻，调用方必须主动调用 `end_sync`。

该提交还包含桌面只读 selected helper、受限私有流协议和资源审计检查点。
正常 Native 依赖固定到连续压缩包完整解压的 russh 修正；既有异步签名和传输接口
提供语义化结果使用提示，公开签名与执行行为不变。
helper 不改变存储公开 API、schema、认证或 trust；正常 V2 Prepare/Start 的生产
接线及应用生命周期验收尚未完成，仍保持默认关闭。全量 P1 证书延后，
不作为本分支正常功能交付前件。
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

## Alacritty（终端状态机，团队 fork）

`rshell-session` 使用该依赖处理真实终端状态；GuoSSHell 经固定的 rsHell 提交
解析同一份包，不在应用内复制源码，也不维护第二套键盘协商栈。

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/alacritty/alacritty |
| fork | https://github.com/MoyuTeams/alacritty ，分支 `feat/kitty-keyboard-state` |
| 固定提交 | `78f36350907b9145a16a6f12e01bfec34ad5b3dd` |
| 上游基线 | `94e7c8874e526b1e67b349d9ba30ddf81669119e`，`alacritty_terminal 0.26.0` |
| 许可证 | Apache-2.0 |

补丁仅限原生 `term/mod.rs` 的两处行为及其私有回归测试：

- Kitty set 在当前原栈帧上应用 replace／union／difference，与实时 mode、query
  和 pop 恢复保持一致；空栈零结果不造帧，已有零帧保留，主／备用屏相互隔离。
- 满栈 push 只驱逐最老的 keyboard frame，保持 4096 容量，不删除 title frame。

两项均属于理由①：上游状态缺陷阻断键盘事件恢复，容量缺陷可由终端输出触发
panic。应用公开 Handler 不能原位更新私有栈帧，也无法在保全空栈、容量和标题
语义的同时规避原生满栈错误。补丁不改 vte parser、public API 或增强位能力。

## russh（原生 SSH 与实验 scoped 入口，团队 fork）

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/Eugeny/russh |
| fork | https://github.com/MoyuTeams/russh ，分支 `feat/p1-scoped-native` |
| 固定提交 | `392265d2b622fdce3fd57f836db134773c475950` |
| 上游基线 | `4882af71cf27ea5293636bf4985ef296dcf20896`，`russh 0.62.5` |
| 许可证 | Apache-2.0 |

新增显式 scoped 连接、资源 root、时钟／协作让出及受保护认证载荷检查点。
分配前准入、取消与最后 backing 释放由该入口的资源所有者承担；旧构造、
legacy Handler、认证接口及 SSH wire 保持。应用不复制库源码，不改 Tokio/util。

正常压缩解码按每次调用后的输入和输出计数判断完成，输出缓冲恰好填满时继续
扩容读取，保持原最大解压大小限制。连续 CHANNEL_DATA 与复用字典均逐包完整
交付，覆盖 `zlib` 和 `zlib@openssh.com`。此修正属于理由①：库内解压器保留未读
输出会截断 SSH string 并使后继包串入；应用无法通过公开 API 正确接管其内部状态。

属于理由③：普通回调发生在库内解码、排队和缓冲增长之后，应用无法覆盖
这些分配的准入、秘密副本擦除及 task/IO 的实际最后持有者。该入口仍为实验
检查点；完整网络活图、crypto/compression 与正常应用 Start 尚未验收。
`rust/Cargo.toml` 与固定 rsHell 使用同一版本、提交和后端 features，避免双实例。

本分支正常 Native 继续使用既有稳定入口。未整合的 scoped 资源、类型化熵错误、
借用 DH、cipher/MAC 和 inline 同步候选保留但延至分支后加固，不因此升级应用 pin。
候选中未完成的源图或构建中间态不能作为已验收产品能力。

## crypto-bigint（共享参数擦除与借用工作区，团队 fork）

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/RustCrypto/crypto-bigint |
| fork | https://github.com/MoyuTeams/crypto-bigint ，分支 `feat/p1-secret-params` |
| 应用固定提交 | `9a4f70ace040b57f5f5cffc0d4389d2676baee9d` |
| 借用工作区库候选 | `14296e9491e6b28cc70d5d2fb5818f2e40ae5161` |
| 上游基线 | `2b54d248cce00457e3afb5650d9b14632ef4a116`，`crypto-bigint 0.7.5` |
| 许可证 | Apache-2.0 OR MIT |

根 Cargo patch 固定同一版本和提交，不复制源码。应用固定提交的补丁为 boxed Montgomery
参数的私有共享 owner 增加最后释放擦除：启用 `zeroize` 时，在原位置擦除辅助字段，
清零原 modulus、one、r2 backing 后释放；共享值存活时保持不变。清理不分配，
不改变公开构造、访问、比较和 clone 契约，也不改变固定精度参数的 Copy 语义。

属于理由①：固定依赖的 RSA CRT 预计算参数原 backing 在释放前仍含秘密。
RSA、ssh-key 和应用只能取得只读共享参数，无法在应用侧安全保证最后 owner
擦除；对共享只读引用强转、只清独立副本或提前修改存活值均不满足原契约。
本补丁不改变算法、SSH wire、认证或 trust；boxed 参数的擦除证据不代表完整
网络资源和认证秘密持有图已经闭合。

库候选额外提供 `WorkspaceLayout`、`PowOperation` 和 `RandomBelowOperation`：
调用方提供对齐的 `21*n` Limb 工作区，以显式预算推进模幂与无偏采样；库不拥有
内部 heap backing。库清理 scratch 与显式秘密 continuation，调用方负责指数、
最终输出及完整 allocation 的擦除、释放和最后退款。旧 API 与默认行为保持。

借用工作区属于理由③：原 boxed 运算在库内创建私有临时 backing，应用无法在
各分配点之前准入，也无法擦除不可访问的内部秘密副本。候选已发布不等于应用
已接线；真实 SDK owner、初始连接／重键、其他目标及完整资源图仍须验收，应用
固定提交不因该库候选的发布而变化。

借用工作区候选的应用接线延至本分支之后；已使用的共享参数擦除补丁保留。
本分支不以 borrowed 运算或完整密码学内部擦除证书作为正常 SSH 前件。

## miniz_oxide（scoped 压缩工作区，团队 fork）

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/Frommi/miniz_oxide |
| fork | https://github.com/MoyuTeams/miniz_oxide |
| 工作区接缝源码基线 | `44e43c7786e379b2b1a7fde4aa0e63be719e583d`，包目录 `miniz_oxide`，版本 `0.8.9` |
| 应用当前来源 | crates.io `miniz_oxide 0.9.1`，由 `flate2 1.1.10` 消费 |
| 许可证 | MIT OR Zlib OR Apache-2.0 |

属于理由③：该源码基线的压缩字典、编码表和 scratch 由库内私有 Box 持有，公开
接口无法提供分配前准入、内部暂停点及释放前擦除。接缝限定为调用方持有的
workspace 与源级有界推进，不复制库源码到应用，也不增加 flate2 fork。

该 fork 接缝尚未验收或接入应用。旧 flate2 构造、默认行为及正常压缩保持；
scoped 路径不得靠过滤协商或永久拒绝压缩替代交付。独立 borrowed codec 只能在
真实 owner、正常双向压缩、重键及三目标证据完成后进入应用正式精确依赖；`0.8.9` 候选源图
不能代替应用当前 `0.9.1` 的源图证据。

工作区接缝及算法内部有界推进延至本分支之后。现有 fork 和未提交候选保留，
本分支不接入该候选、不过滤正常压缩，也不为该高保证目标新增 HMAC 等依赖 fork。

## terminal_view（终端渲染，团队 fork）

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/Termphin/terminal_view（pub.dev `terminal_view` 0.2.0，fork 自 xterm.dart 4.0.0） |
| fork | https://github.com/MoyuTeams/terminal_view ，分支 `feat/key-source`（基于 `guosh/frame-source`） |
| 固定提交 | `e8f23569c6e07a04dad11a3459d456d8d71c0b4c` |
| 上游基线 | `19c6ebb4bb03898f3d626c588acd0e0a1d92c152`（Termphin `main`） |
| 许可证 | MIT，随包保留上游 `LICENSE` 与 `NOTICE` |

```yaml
terminal_view:
  git:
    url: https://github.com/MoyuTeams/terminal_view.git
    ref: e8f23569c6e07a04dad11a3459d456d8d71c0b4c
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
| 键盘事件携带触发它的硬件事件（`TerminalKeyboardEvent.source`）及当时采样的左/右 Alt；`keyInput` 同名可选参数 | `e8f2356` | ③ 接缝：桌面有序输入需要物理键、是否连发与左右 Alt 来源交 Rust 按终端 profile 编码，库内处理按键后只交出逻辑键与合并的 Alt |

Termphin `main` 在上游基线之后的提交尚未并入 fork。升级时先把 fork 变基到新的
上游提交，去掉上游已等价修复的改动，更新本节的固定提交与表格，并运行库测试
及应用的输入、选区、鼠标上报回归。
