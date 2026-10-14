# 桌面扩展需求与进度

桌面扩展的目标：让 Windows/macOS/Linux 桌面端对齐其他终端工具的常用特性——三桌面统一的 Fluent 工作区、终端 profile 与应用级设置、桌面键位条、绑定动作、本地 Shell 标签与 System OpenSSH。iOS/iPadOS/Android 保持现有界面、行为、存储与认证安全边界不变，仍走旧 `ConnectRequest`。依赖 fork 与精确 pin 记录在根 `UPSTREAM.md`。

## 总体约束

- 技术栈维持 Flutter + Rust/rinf。Rust 是业务、配置、终端状态、输入编码与 viewport 的唯一权威；Dart 只维护表单草稿、焦点、布局与渲染投影，不引入第二套配置规则、SSH 业务或 ANSI 解析器。
- 旧移动 `ConnectRequest`/`InputRequest` 与桌面 V1 `DesktopConnectRequest/Result` 的信号布局、必填字段与语义不变；新能力以带版本的独立扩展入口并存。
- 系统文件选择器与系统硬件认证提示继续使用系统界面；Native SSH 的 host key fail-closed；密码、PIN、私钥、passphrase 不写入日志、命令行、环境或测试报告。
- Windows 为原生实跑收敛平台；macOS/Linux 由三平台 CI 构建与测试，GUI 与沙箱未实跑时如实标注未验证，不冒充通过。
- 分组、tags、旧配置导入不在范围。

## 本分支范围与取舍

- 应用侧安全底线：Native SSH 的 host key fail-closed、凭据不落日志/命令行/环境、配置事务与快照、应用自持资源预算与生命周期。
- selected_p1 资源证书、scoped russh/crypto-bigint、allocator 审计等高保证加固不在本分支交付范围，按[分支后深层安全加固风险](followups/20261014_分支后深层安全加固风险.md)搁置。相关测试由 hub 的 `experimental-candidates` feature 门控，CI 独立步骤运行且不计入门禁（见[桌面开发验证](desktop-validation-ci.md)）。
- Windows 私有原生输入窗口（NativePort、`desktop_native_host`、`desktop_session_controller`、`windows/runner/desktop_native_input`）代码保留，但不接线、不开放；桌面输入统一走 Flutter 有序通道（见[原生输入窗口方案搁置](followups/20261008_桌面键盘同源仲裁接缝.md)）。

## 配置全集（设置目标）

以 shellx 领域配置为基准。颜色选项：`Default`、`OneDark`、`SolarizedDark`、`SolarizedLight`、`Dracula`、`Monokai`、`Nord`、`GruvboxDark`、`TokyoNight`、`CampbellPowershell`，应用主题与 profile 终端颜色分别保存。

| 层级 | 字段 | 生效规则与验收要点 |
|---|---|---|
| 应用 | `default_terminal_profile`、`color_scheme`、`key_bindings` | 默认 profile 控制新会话；应用配色立即生效；应用绑定经 Rust 与 profile 绑定合并。 |
| Profile 身份 | `id`、`name`，创建/复制/重命名/删除/切默认 | ID 稳定；默认与被引用 profile 由 Rust 拒绝删除；已有会话持有有效快照不产生悬空引用。 |
| 终端基础 | `terminal_type`、`initial_cols`、`initial_rows`、`answerback` | 同一 resolved profile 同时进入引擎与 transport；远端 PTY TERM、本地 TERM、ENQ 回答实际符合配置；初始尺寸是期望网格，首个 PTY 以真实布局几何为准。 |
| 显示 | `font_family`、`font_size`、`color_scheme`、`scrollback_lines` | profile 字体、真实度量与颜色贯通；缺失字体提示回退但保留配置；滚回与光标/前背景实际生效。 |
| Profile 按键 | `key_bindings` | 同 chord 的 profile 绑定覆盖应用绑定；重复/非法 chord 或未知 action 拒绝；文本编辑焦点不触发终端动作。 |
| Alt/Meta | `left_alt_as_meta`、`right_alt_as_meta` | 左右 Alt 分开配置与测试；AltGr/Option 组合字符及 IME 不重复编码（现有缺口见[V2 输入剩余缺口](followups/20261014_桌面V2输入剩余缺口.md)）。 |
| 协议 | `enable_csi_u`、`enable_kitty_keyboard` | CSI-U 按 profile 固定开/关；Kitty 按配置门控与真实远端 flags 进退。 |
| 鼠标 | `mouse_reporting` | 与远端请求的鼠标模式双门控；关闭时本地选择/滚动可用。 |
| 滚动 | `scroll_on_output`、`scroll_on_keypress` | 四种组合分别在历史区验证输出/按键/提交/粘贴；移动保留原强制滚底行为。 |
| GuoSSHell 扩展 | 键位条显示、两排排布、自定义文本、空位、恢复默认 | 桌面默认不显示可开启完整编辑；移动默认显示且协议/存量排布不变。 |

校验范围沿用上游：行列 1–999、滚回 100–1,000,000、字号 6–72 有限数值、非空名称/字体/answerback；资源限制由 Rust 返回「配置值/有效值/限制原因」。绑定 action 集合为三个 `send:` 序列（VT220 Delete、DEL、BS）及 `clear_scrollback`、`new_tab`、`split_vertical`，不得扩展为本机命令执行入口。

Profile 参数（终端参数/协议/绑定/滚回）在新会话或显式重启时取快照生效，UI 明示；应用配色、字体分槽、键位条、透明度保持即时行为。

## 字体绑定契约

- 应用内部使用有版本判别类型：`InheritTerminalSlot`（继承终端槽）、`ExplicitFamily`（显式 family）、`ExplicitCachedTtf { hash }`（显式缓存 TTF）；无元数据旧记录投影为 legacy，运行走旧终端槽，不猜测历史意图。
- 绑定保存在 `TerminalSettingsV1.extensions["guosshell.font"]`（有版本载荷），与 profile 创建/复制/删除同属一个 configuration 事务，使用整体 revision CAS；未知版本/不可解析载荷原样保留并返回不可编辑原因，拒绝会改变 family 或绑定的写请求。
- 显式绑定（含继承）用不可变会话快照，不被槽更新覆盖；legacy 路径保留旧终端槽即时监听。缓存 blob 保守不删：profile 删除、槽替换/重置、导入失败回滚都不移除可能被引用的文件。
- profile 选 TTF 不偷改全局槽：先持久化验证后的不可变 blob，再以 CAS 发布模式/hash；预览不持久化。
- 移动渲染始终取终端槽，不受桌面模式影响。

## 阶段与状态

| 阶段 | 内容 | 状态 |
|---|---|---|
| 二 | Rust 桌面契约：版本化设置/profile 协议、完整配置 CAS 存储、会话目标模型、能力信号、字体绑定接缝 | 完成 |
| 三 | Fluent 目录、连接编辑、密钥管理、认证对话框接入常驻工作区 | 完成 |
| 四 | 统一工作区、V2 会话与终端输入、终端 profile 生效、连接选择 profile | 主体完成，输入缺口见下 |
| 五 | 应用级设置、profile 管理、桌面键位条接线 | 进行中 |
| 六 | 本地 Shell 标签 | 待做 |
| 七 | System OpenSSH | 待做 |
| 八 | 三平台汇合验收 | 待做 |

### 阶段四：工作区、V2 会话与输入

- 桌面三平台终端窗格统一走 V2 Prepare/Start（桌面目标默认开启，移动目标关闭，移动端仍走旧 `ConnectRequest`）。Prepare 不含密码、不启动传输；Start 核验同身份、字体投影与实测几何后一次性转交运行会话；事件按 source + identity + generation + seq 过滤，防迟到与重复。
- 输入：Flutter 的按键、文本、鼠标与剪贴板经有序通道（`DesktopOrdered`）交 Rust，由 `desktop_input_policy` 按会话 profile 匹配绑定、左右 Alt 并按键盘协议编码；壳动作（`new_tab`、`split_vertical` 等）由 Rust 回报 Dart 执行；丢失松开事件后再次按下按隐式松开处理。Dart 侧实现在 `lib/src/terminal/desktop_ordered_session.dart`；`terminal_view` 团队 fork 提供按键来源与左右 Alt。
- 连接编辑器可选终端配置（`DesktopSaveConnection` 带 `profile_id`），操作按钮固定页脚。
- 已知输入缺口见[桌面 V2 输入剩余缺口](followups/20261014_桌面V2输入剩余缺口.md)。

### 阶段五：应用级设置与 profile

控制器与草稿（`lib/src/desktop/settings/`）、App/Profile 表单、结构化绑定编辑器、字体资源区与键位条编辑器的组件已备。进行中：接入共享设置入口，完成 profile CRUD/默认、应用配色与键位条的 Rinf/SQLite 往返及焦点验收。

### 阶段六：本地 Shell

复用上游 rsHell 的 `LocalPtyTransport`；团队 fork `MoyuTeams/rsHell` 分支 `feat/local-pty-writer` 为本地 PTY 写入增加专用写线程与有界队列，待接线，为下一节点。应用侧 `native/hub/src/local_shell/` 与 `system_ssh/` 原型保留在实验门控下，不进入产品路径。

### 阶段七：System OpenSSH

与其他终端工具一致，在本地 PTY 中运行系统 `ssh`，使用用户自己的 ssh 配置、agent 与 known_hosts；主机密钥确认与密码由 ssh 在终端内交互。应用不自行读取或转存这些凭据。待实现；原「隔离 System SSH / 三角色 helper」设计不再采用。

### 阶段八：汇合验收

合并各阶段证据：Native/本地/System 三类会话的协议、TUI 与退出矩阵，移动关键集回归，标准/窄窗口、缩放、分屏、焦点与高对比度验收，移除可见 Material 回退。逐平台区分「原生运行通过/CI 构建通过/CI 测试通过/失败/未验证」；macOS/Linux GUI 未实跑须如实标注，macOS 沙箱对本地进程与系统 ssh 的影响须实测后再声明。

## 验证命令

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
flutter analyze --no-pub
flutter test --no-pub
```

Windows 原生验收入口：`scripts/ci/windows_auth_check.ps1`。

- 默认 `auth` 目标：目录 → Fluent 首见认证 → V2 Native SSH 回显 → 管理往返。
- `-Target profile`：经设置协议建 profile 后，验证 `TERM=vt220`、answerback、F5 绑定 `send DEL`、左 Alt 作 Meta/右 Alt 不作 Meta，并与默认 profile 对照。

另有 `integration_test/desktop_catalog_smoke_test.dart`。三平台 CI 由 `.github/workflows/desktop-validation.yml` 承载。

## 当前状态

| 验证面 | 证据 |
|---|---|
| Windows 原生 | `windows_auth_check.ps1` 两个目标与 `desktop_catalog_smoke_test` 通过 |
| Flutter / Rust | Flutter 全套、Rust workspace 测试通过 |
| macOS / Linux | 由三平台 CI 构建与测试；GUI 未实跑，不视为验证 |

CI 状态以对应提交的实时作业结果为准，见[桌面 CI 待收敛](followups/20261014_桌面三平台CI待收敛.md)。

### 后续任务顺序

1. 阶段五设置接线。
2. 本地 Shell：接线 `feat/local-pty-writer` 与 `LocalPtyTransport`，开放本地标签。
3. System OpenSSH：本地 PTY 运行系统 `ssh`。
4. 阶段八三平台汇合验收。

待完成事项清单见[桌面检查点待完成验收](followups/20261012_桌面检查点待完成验收.md)。
