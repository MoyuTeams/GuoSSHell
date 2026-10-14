# 桌面扩展需求与进度

桌面 Fluent 扩展的目标：在 Windows/macOS/Linux 用 Flutter `fluent_ui` 完成 GuoSSHell 全部页面与操作的桌面实现，新增本地 Shell 与隔离 System OpenSSH 两种 transport，接入 shellx 全部终端配置；iOS/iPadOS/Android 保持现有界面、行为、存储与认证安全边界不变。整体方案与验收口径以本文件为准；依赖 fork 与精确 pin 记录在根 `UPSTREAM.md`。

## 总体约束

- 技术栈维持 Flutter + Rust/rinf。Rust 是业务、配置、终端状态、输入编码与 viewport 的唯一权威；Dart 只维护表单草稿、焦点、布局与渲染投影，不引入第二套配置规则、SSH 业务或 ANSI 解析器。
- 旧移动 `ConnectRequest`/`InputRequest` 与桌面 V1 `DesktopConnectRequest/Result` 的信号布局、必填字段、队列与语义不变；新能力以带版本的独立扩展入口并存，不提升版本号拒绝旧客户端。
- 系统文件选择器与系统硬件认证提示继续使用系统界面；不降低 host key fail-closed 保证；密码、PIN、私钥、passphrase 不写入日志、数据库、命令行、环境或测试报告。
- 实现检查点可以独立提交到 `feat-desktop`，但必须标明未完成的接线与验收；阶段完成仍以完整验收为准，不把局部交付当阶段完成，最终范围不缩水为 MVP。
- Windows 为原生实跑收敛平台；macOS/Linux 以三平台 CI 的检查/测试/构建保证可移植性，GUI 与沙箱未实跑时明确标注未验证，不冒充通过。

### 本分支安全与可靠性边界

本分支完成本软件界面、应用层和平台宿主可控的保证：认证与 host-key 信任、凭据保护、权限隔离、配置事务与快照、真实生命周期、应用自持资源预算、背压、超时和取消。正常 Native SSH 可使用现有稳定依赖入口，不以替换底层运行时作为交付前件。

难以通过应用侧简单实现的第三方内部秘密擦除、全部可达分配的源式准入、全进程硬内存包络、算法内部有界推进、冷锁零隐藏 heap 与整体编译后常时性证明，按[分支后深层安全加固风险](followups/20261014_分支后深层安全加固风险.md)评级延后。原 P1 的 `403701760` 全量包络与 `SourceClosed` 不再是本分支交付条件，也不作为已实现保证对外宣称；应用已实施的有效预算和安全清理继续保留。

真实使用路径上的内存不安全、泄密、信任绕过、错误竞态、死锁及不可控资源耗尽不能仅记录延期。实验候选不自动接入正式构建；旧接口、正常配置和压缩行为不缩水，三平台功能验证及未验证场景披露保持。

## 配置全集（设置目标）

以 shellx 领域配置为基准，不扩大为 GTK 应用的其他产品功能（分组、tags、旧配置导入不在范围）。颜色选项：`Default`、`OneDark`、`SolarizedDark`、`SolarizedLight`、`Dracula`、`Monokai`、`Nord`、`GruvboxDark`、`TokyoNight`、`CampbellPowershell`，应用主题与 profile 终端颜色分别保存。

| 层级 | 字段 | 生效规则与验收要点 |
|---|---|---|
| 应用 | `default_terminal_profile`、`color_scheme`、`key_bindings` | 默认 profile 控制新会话；应用配色立即生效；应用绑定经 Rust 与 profile 绑定合并。 |
| Profile 身份 | `id`、`name`，创建/复制/重命名/删除/切默认 | ID 稳定；默认与被引用 profile 由 Rust 拒绝删除；已有会话持有有效快照不产生悬空引用。 |
| 终端基础 | `terminal_type`、`initial_cols`、`initial_rows`、`answerback` | 同一 resolved profile 同时进入引擎与 transport；远端 PTY TERM、本地 TERM、ENQ 回答实际符合配置；初始尺寸是期望网格，首个 PTY 以真实布局几何为准。 |
| 显示 | `font_family`、`font_size`、`color_scheme`、`scrollback_lines` | profile 字体、真实度量与颜色贯通；缺失字体提示回退但保留配置；滚回与光标/前背景实际生效。 |
| Profile 按键 | `key_bindings` | 同 chord 的 profile 绑定覆盖应用绑定；重复/非法 chord 或未知 action 拒绝；文本编辑焦点不触发终端动作。 |
| Alt/Meta | `left_alt_as_meta`、`right_alt_as_meta` | 左右 Alt 分开配置与测试；AltGr/Option 组合字符及 IME 不重复编码；未知来源保持旧总 Alt 行为并明确标记。 |
| 协议 | `enable_csi_u`、`enable_kitty_keyboard` | CSI-U 按 profile 固定开/关；Kitty 按配置门控与真实远端 flags 进退；Super 与 press/repeat/release 仅在配置允许且实际协商满足时编码。 |
| 鼠标 | `mouse_reporting` | 与远端请求的鼠标模式双门控；关闭时本地选择/滚动可用。 |
| 滚动 | `scroll_on_output`、`scroll_on_keypress` | 四种组合分别在历史区验证输出/按键/提交/粘贴；移动保留原强制滚底行为。 |
| GuoSSHell 扩展 | 键位条显示、两排排布、自定义文本、空位、恢复默认 | 桌面默认不显示可开启完整编辑；移动默认显示且协议/存量排布不变。 |

校验范围沿用上游：行列 1–999、滚回 100–1,000,000、字号 6–72 有限数值、非空名称/字体/answerback；资源限制由 Rust 返回「配置值/有效值/限制原因」。action 集合为三个 `send:` 序列（VT220 Delete、DEL、BS）及 `clear_scrollback`、`new_tab`、`split_vertical`，全部必须真正生效，不得扩展为本机命令执行入口。

Profile 参数（终端参数/协议/绑定/滚回）在新会话或显式重启时取快照生效，UI 明示；应用配色、字体分槽、键位条、透明度保持即时行为。

## 字体绑定契约

- 应用内部使用有版本判别类型：`InheritTerminalSlot`（继承终端槽）、`ExplicitFamily`（显式 family）、`ExplicitCachedTtf { hash }`（显式缓存 TTF）；无元数据旧记录投影为 legacy，运行走旧终端槽，不猜测历史意图。
- 绑定保存在 `TerminalSettingsV1.extensions["guosshell.font"]`（有版本载荷），与 profile 创建/复制/删除同属一个 configuration 事务，使用整体 revision CAS；未知版本/不可解析载荷原样保留并返回不可编辑原因，拒绝会改变 family 或绑定的写请求。
- 显式绑定（含继承）用不可变会话快照，不被槽更新覆盖；legacy 路径保留旧终端槽即时监听。缓存 blob 保守不删：profile 删除、槽替换/重置、导入失败回滚都不移除可能被引用的文件。
- profile 选 TTF 不偷改全局槽：先持久化验证后的不可变 blob，再以 CAS 发布模式/hash；预览不持久化。
- 移动渲染始终取终端槽，不受桌面模式影响。

## 阶段划分与状态

依赖顺序：接入融合 → 二 → 三 → 四；五依赖二/四；六依赖四；七依赖二/四；八汇合全部。

### 已完成阶段

| 阶段 | 内容 | 提交 |
|---|---|---|
| 二 | Rust 桌面契约：版本化设置/profile 协议、完整配置 CAS 存储、会话目标模型、能力信号、字体绑定接缝 | `100ba353` |
| 三 | Fluent 目录、连接编辑、密钥管理、认证对话框接入常驻工作区；Windows 原生认证/管理往返验收；三平台 CI `37183281852` 通过 | `e2ae0a44` |

### 阶段四（输入实现检查点已交付，完整验收未完成）：工作区、终端外壳与完整输入策略

范围：沿 `WindowsShell → Workspace → 常驻 TerminalPane` 扩展共用工作区，Native SSH 完整消费 profile；macOS/Linux 收敛到同一状态模型；补全连接选择器、终端工具栏、会话状态、选择菜单、关闭/重连等 Fluent 操作。Local/System 本阶段不可用。

**会话与传输（4A）**——已实现：

- 独立 V2 `Prepare/Prepared/Start/Cancel` 与有序事件。Prepare 无密码、不启动传输，固定目录目标并单次读取 configuration 捕获字体资源；Start 接收密码后立即进入 `SecretString`，核验同身份、字体投影与实测几何后一次性转交运行会话；`Accepted` 不等于已认证。
- 准备态有界生命周期：有限有效期、每 session/generation 至多一个未消费态、全局数量与实际资源预算上限；取消/替换/过期/消费/退出均终结并释放，迟到结果不复活。
- 事件按 source + identity + generation + seq 过滤，防迟到与重复。
- **新 V2 Prepare/Start 当前默认关闭**（`Preparations::default()` 返回 `PolicyUnavailable`，仅测试构造器开启），待正常生产接线、应用自持资源生命周期与功能验收完成后开放；不再等待第三方全资源闭包。不得直接删除守护代替接线。

**终端输入（4B-I / 4C）**——部分实现：

- 独立输入策略（`native/hub/src/desktop_input_policy.rs`）：raw chord 优先匹配 profile/应用合并绑定、16 项默认候选、消费锁存至物理 Up、Repeat 不重复壳动作、未知来源 typed 拒绝。
- 上游兼容键盘事件接缝（rsHell `TerminalKeyEvent` 双表示 + `encode_key_event`，Kitty flag 1/2 下 Super 与 press/repeat/release）已发布并 pin。
- 输入架构采用「单一原生输入 surface + Rust 先仲裁」：Flutter 公共按键事件无原生事件 ID，不能与另一通道元数据配对；键盘/文本/指针统一进入应用私有 Windows 输入窗口（TSF 前门控、Translate 前仲裁），文本/IME 提交保持单一来源票据。原型由 `native-input-spike` feature 控制，默认关闭。
- 正常输入采用独立 NativePort ABI 1／19 个导出，绑定原会话 actor；Windows 宿主只关联 Rinf 已加载的同一 hub 模块，不另载运行时。HWND、TSF/COM 生命周期、文本终结、动作 claim 与退休已有组件验证，组件的 C ABI 替身不能替代实际 Rinf 输入联证。独立 spike ABI 3／11 个导出仍只用于隔离 fixture。
- normal 键位条通过唯一 Dart writer 提交明确的 `GuiIntent`，不冒充物理键／原生文本来源。Ctrl/Alt 修饰快照与 revision 由 controller 持有，Rust 准入回执结算实际 latch 消费；单个待决请求不积压，5 秒超时或 `Unknown` 隔离 GUI 输入并停止重复，不自动重放。composition 结束与异步布局按原窗格／事务结算，旧回调不能清除或关闭较新的输入。
- 实现检查点保留正常生产输入 guard 关闭。物理键盘、非 US 布局、真实 IME/TIP、文本／指针与完整 GUI 输入联证，以及阶段四完整矩阵仍待验收；组件测试通过不构成生产开放条件。

**配置读取与应用资源控制**——待生产组合验收：

目标：保全完整配置与字体绑定，在同一有效快照上 Prepare／Start；应用自持的准备态、字体、队列和大对象有明确预算与生命周期。取消、过期、替换、窗口关闭和迟到结果不能复活会话或提前释放仍使用的资源，不按数据库规模、编码或扫描数拒绝正常小配置。

`native/hub/src/selected_p1/`、只读 selected helper 和产物来源检查已有独立候选。生产路径选择以功能、事务、权限和应用可控资源保证为依据，不要求为全量 SQLite／第三方分配证书更换读取架构。真实 VM 接口若采用外部 backing，必须保证最后使用者释放；其他安全所有权方案也可使用，不为取得证书强制特定 finalizer 形态。

未完成：正常 Prepare／Start 生产组合、字体与元数据投影、关闭／重连、真实边界所有权及三平台功能验证。全资源包络候选保留但不作为这些工作的前件。

**正常网络与实验 scoped 入口**：

本分支在现有 Native 入口上完成实际认证、host-key 信任、PTY、双向读写、resize、keepalive、超时、取消及重连，保持认证模式与正常压缩；应用管理输出队列、并发、错误提示和关闭状态。稳定依赖的运行时无需为取得完整资源证书而替换。

scoped russh、borrowed bigint、独立压缩 codec 及 inline 同步候选用于后续高保证加固，不作为本分支正常连接前件。其局部通过不能代表全资源闭包，未整合或存在中间态的候选不进入正常路径；已使用依赖中的有效 bug 修复与安全补丁保持。

**验收矩阵要点**（阶段四完成条件）：真实目录/快速连接选择不同 profile 且不混快照；准备态边界 fake 证明；两个不同 profile 真实改变期望区域且首个 PTY 与快照一致；ENQ 精确 answerback；输入绑定矩阵（含覆盖 `desktopCommand`/`zoom.handleKey`/`TerminalView.shortcuts` 冲突）；四种滚动组合；多字体窗格与重开；生命周期导航往返；移动回归不退化。

### 阶段五（组件已备，待接线）：完整 Fluent 设置

- 控制器与草稿（`lib/src/desktop/settings/`）：未决写入隔离、冲突草稿保留、显式核对后解除、query 门槛按请求身份区分；已删除目标的生命周期与字体入口安全禁用。
- 表单（`forms/`）：App 三字段、Profile 16 字段、结构化绑定编辑器。
- 资源区（`resources/`）：双全局字体槽、Windows 外观单写者、关于/许可。
- 键位条（`keybar/`）：完整双排编辑器、全应用单写者与跨关闭未决隔离。

待接线：共享入口路由、字体 RID/slot 确认、外观无 RID 协议的全应用 writer、`KeyBarWriter.shared` 统一旧写入口、真实 Rinf/SQLite 往返与 NativeFocus。组件测试不替代这些验收。

### 阶段六（前体已备，待生产化）：本地 Shell

应用侧自持 PTY owner 已验证：自然退出以原进程对象 signaled 为门、8 MiB 堵写真实取消、跨读事件 UTF-8 累计、单次 unwind 主错误保持、资源根持有到全部 worker join（`native/hub/src/local_shell/`，当前测试构造器）。无合法 saved-Local 契约时非空 `connection_id` 在进入 SSH 分支前拒绝。

剩余要求：通过 `LocalPtyTransport` 接入统一 transport；默认 shell 用上游平台选择，自定义启动项按结构化字段校验，不接受 UI 隐式 shell 拼接；本地不走网络权限/SSH 凭据；spawn 未完成时关闭的竞态立即接管清理；Windows Job/ConPTY 与 Unix PTY/进程组清理不只查直接子 PID；完整 process-root、持久平台错误收敛、其他平台与 GUI 注册。

### 阶段七（设计已定，前体验证中）：隔离 System OpenSSH

- 应用专属 config/known_hosts；不读取用户 SSH config/agent/known_hosts、默认 identity、全局配置；阻断环境回退绕过。
- 三角色短命 helper：`KnownHostsCommand` 绑定实际握手公钥、受保护 `SSH_ASKPASS` 管道传递认证回答、`LocalCommand` 回调证明认证完成（不证明远端 PTY）；每 attempt 校验双方进程身份。
- 首次主机信任显式确认；变更密钥 fail-closed；信任写失败立即断开。禁止 `StrictHostKeyChecking=no` 与自动接受。
- 保存了远端命令的会话必须申请远端 PTY：实际测试远端 `tty`、`TERM`、resize 与受控 TUI。
- 不自动导出应用私钥；System 与 Native 凭据体系区分。
- 当前已验证封闭 `-G`/`-V` 环境与 ORDER 回调真实调用；真实认证、IPC 身份、PTY 与三平台 hook 行为未验收。

### 阶段八：跨平台集成与交付门禁

合并全部阶段证据：三 transport 协议/TUI/性能/退出矩阵、移动原 M6 关键集回归、完整点击/键盘验收矩阵（标准/窄窗口、缩放、分屏、焦点、屏幕阅读语义、高对比度、减少动画）、移除可见 Material 回退、文档更新为最终能力与限制。逐平台报告区分「原生运行通过/CI 构建通过/CI 测试通过/失败/未验证」。

## 高风险关口

| 关口 | 内容 | 状态 |
|---|---|---|
| B1 | macOS 沙箱与本地进程能力：现有 entitlements 不能证明 shell/PTY/系统 ssh 可用；关闭沙箱或加权限超出授权 | 未实跑，明确未验证 |
| B2 | System OpenSSH 信任与配置边界：新 pin 隔离接口必须重新验证，不继承旧结论 | 设计已定，实现中 |
| B3 | 依赖能力与公共接口缺口：左右 Alt/Super、TERM 传播、非阻塞 spawn/写入/关闭、进程树回收、隔离与远端 PTY 均需对精确 pin 验证；能应用侧就不改上游，必要时最小补丁入团队 fork | 已用四个 fork 承接 |

## 验证命令（每阶段通用）

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
flutter analyze --no-pub
flutter test --no-pub
```

Windows 原生验收入口：`scripts/ci/windows_auth_check.ps1`（隔离 Native SSH 认证/管理往返）、`scripts/ci/windows_input_check.py`（环回 SSH 键盘输入字节核对）、`integration_test/desktop_catalog_smoke_test.dart`。三平台 CI 由 `.github/workflows/desktop-validation.yml` 承载。

## 当前检查点状态

输入实现检查点为 `81c3a00`，已提交并推送。正常 NativePort、Windows 宿主、Dart
`GuiIntent` 键位条、修饰回执、焦点／帧关联及 composition 生命周期已接线；冻结
profile、字体与静态配色按窗格持有。这个检查点不是阶段四完整交付，也不是生产开放。

### 验证范围

| 验证面 | 当前证据 | 限制 |
|---|---|---|
| Rust | 精确暂存树的 fmt、workspace all-targets check、Clippy 通过；hub 库 324 项通过 | 全 workspace 测试未通过，不能用库测试替代全套结论 |
| Flutter | 全项目 analyze 通过，全套 512 项通过；键位条 Widget QA 覆盖 375／768／1280 宽及辅助模式 | 不替代真实原生窗口、屏幕阅读器或设备输入验收 |
| Windows 宿主 | 有效前台条件下 541 项通过，涵盖 HWND／COM／TSF、文本终结、动作 claim 与退休 | C ABI 后端仍为替身，不证明真实物理 Raw Input 或 TIP／IME |
| Windows 主应用 | debug 构建通过 | 不证明正常输入生产链已开放 |
| 真实 runner／Rinf | 同 hub 模块关联、Prepare／Start 的 `PolicyUnavailable`、三类 `GuiIntent` 完整关联拒绝回执通过 | 不覆盖已认证 active pane 的实际输入与远端字节 |
| 三平台 CI | 工作流检查及三平台 Cargo fmt／check／Clippy 已通过；Windows、macOS 的 Cargo 单元测试失败，Linux 测试作业尚未完成 | 当前不全绿；失败日志待定位，后续 Flutter／构建步骤不能算通过 |

全套 Rust 测试的已知产物前件与并行审计限制见
[helper 产物前件](followups/20261014_Rust全套测试的helper产物前件.md)。三平台 CI
状态与下一步见[桌面 CI 待收敛](followups/20261014_桌面三平台CI待收敛.md)。CI 状态
应以对应提交的实时作业结果为准，不沿用旧提交的通过记录。

### 后续任务顺序

1. 核对当前提交的 CI 首个失败，区分正常功能回归、实验验收前件和并行共享状态问题；不忽略测试或放宽安全断言取得通过。
2. 继续阶段四正常 Prepare／Start、认证后输入与关闭／重连的生产组合，补实际 profile、字体、PTY、滚动及绑定矩阵证据。
3. 补真实物理键、非 US 布局、死键、AltGr、IME／TIP、指针及适用平台验收；满足开放条件后才调整 production guard。
4. 按依赖推进阶段五完整设置、阶段六本地 Shell、阶段七隔离 System OpenSSH，再汇合阶段八。

独立 Local Shell production 与 `selected_p1` 网络／session owner 候选仍有未提交工作，
不属于 `81c3a00`。继续任务须先核对工作树，不丢弃这些改动，也不未经核验把它们带入
正常生产路径。正式依赖身份仍以 `UPSTREAM.md` 与 manifest／lock 为准。

正常新 V2、Local/System 生产入口尚未开放；恢复条件见
[桌面检查点待完成验收](followups/20261012_桌面检查点待完成验收.md)。高保证资源候选
按风险条目延后，不代替或阻塞正常功能接线。
