# 桌面扩展进行中工作

桌面 Fluent 扩展的目标是在 Windows/macOS/Linux 完成完整工作区、输入、设置、本地 Shell 与隔离 System SSH，移动端行为与协议不变。当前处于阶段四中段：输入与资源安全的主体架构已落地并提交检查点，正常生产入口默认关闭，未完成范围见文末与[检查点验收跟进](followups/20261012_桌面检查点待完成验收.md)。

依赖的团队 fork 与精确 pin 统一记录在根 `UPSTREAM.md`（rsHell、Alacritty、russh、crypto-bigint）。

## 会话与传输（4A）

- 独立 V2 `Prepare/Prepared/Start/Cancel` 与有序事件已实现，DTO 位于 `native/hub/src/signals/desktop_session.rs`。旧移动 Connect/Input、桌面 V1 布局与 `DESKTOP_CONNECT_VERSION=1` 不变。
- Prepare 无密码、不启动传输；Start 接收密码后立即进入 `SecretString`，核验同身份与真实首次几何。事件按 source + identity + generation + seq 过滤；Accepted 不等于已认证。
- 新 V2 Prepare/Start 默认关闭：`Preparations::default()` 直接返回 `PolicyUnavailable`，仅测试构造器可开启（`native/hub/src/desktop_prepare.rs`）。在资源预算证明完成前不开放正常入口。

## 终端输入（I-G / 4C）

- 独立输入策略已交付：`native/hub/src/desktop_input_policy.rs` 提供 raw chord 优先匹配 profile/应用合并绑定、16 项默认候选、消费锁存至物理 Up、Repeat 不重复壳动作；未知来源 typed 拒绝，不猜 US 布局。
- 输入架构采用「单一原生输入 surface + Rust 先仲裁」：Flutter 公共按键事件没有原生事件 ID，不能与另一通道的元数据做同事件配对。键盘/文本/指针统一进入应用私有的 Windows 输入窗口（TSF 前门控、Translate 前仲裁），Rust 策略决定消费、拒绝或放行文本链，文本/IME 提交保持单一来源票据。
- 原型代码由 `native-input-spike` feature 控制，默认关闭。已验证：同 SDK 隔离握手、合成按键的精确编码字节、TSF 生命周期与 COM 退休安全、测试宿主三项 ownership 修正。未完成：物理键盘、非 US 布局、真实 IME/文本/指针、ABI 3 与宿主联调。设计约束记录在[桌面键盘同源仲裁接缝](followups/20261008_桌面键盘同源仲裁接缝.md)。

## selected 读取与资源预算（P1）

目标：配置读取全部经过分配前硬预算，只有可证明的预算不足才 typed 拒绝，不按库规模、页大小、编码或扫描数拒绝正常小 selected。

架构分三部分：

1. 只读 helper 子进程（rsHell `selected-read-helper`）：独立 SQLite 内存 arena 承担 C 侧硬界，请求级 fail-closed（首次实际配额不足永久失效该请求），同一只读事务返回 selected 原始字段与组存在性事实；构建期来源经 `BuildEvidence` 与实际产物绑定。
2. 应用侧预算账本（`native/hub/src/selected_p1/`）：`resource`（ledger/owner/秘密）、`decode_shape`、`domain_decode`、`font_source`、`font_provider`、`session_owner`、`reply_port`、`control_runtime`、`helper_runtime`、`helper_artifact` 各单元分别交付，覆盖分配前 grant、no-Weak 最后持有者、秘密 exact backing 与 wipe→free→退额顺序。
3. 构建与整合：`scripts/ci/build_selected_helper_artifact.ps1` 记录实际编译来源与产物哈希；`helper_artifact` 单元把候选产物接入应用读取链到完整 domain 快照。

未完成：正常 Prepare/VM 注册、跨进程最后持有者（VM finalizer）、WAL/打包/三平台验收。整体 P1 停止线未解除。

## 网络运行时（russh scoped 接缝）

rsHell 依赖的 russh 在库内分配与认证秘密上无法从应用侧闭合，因此使用团队 fork 增加显式 scoped 入口：应用自有 executor/poller/timer/stream 持有运行根，库内每处动态增长在分配前准入，密码/keyboard-interactive 从第一份 owned 副本起受保护并在释放前擦除。旧公开 constructors、SSH wire、认证模式与 host key 信任不变。

当前进度：认证→PTY→数据→关闭的库内路径与内存对端测试通过（client 无 Tokio runtime）；剩余 flush 缓冲、server 发起 channel、公钥/证书/外部签名、crypto/RNG/TLS、压缩的完整源闭包。crypto-bigint fork 为共享 Montgomery 参数补最后持有者擦除。相关链接见根 `UPSTREAM.md`。

## 本地 Shell（阶段六前体）

应用侧自持 PTY owner 已验证：自然退出以原进程对象 signaled 为门、8 MiB 堵写真实取消（995）、跨读事件 UTF-8 累计、单次 unwind 主错误保持、资源根持有到全部 worker join。`native/hub/src/local_shell/` 当前为测试构造器；完整进程根、持久平台错误收敛、其他平台与 GUI 注册未验收。无合法 saved-Local 契约时非空 `connection_id` 在进入 SSH 分支前拒绝。

## 隔离 System SSH（阶段七前体）

旧 System transport 继承用户配置与 agent、不等待认证，不能直接开放。设计为应用专属 config/known_hosts + 三角色短命 helper（KnownHostsCommand 绑定实际握手公钥、受保护 ASKPASS 管道、认证完成回调），每 attempt 校验双方进程身份；首次信任落盘、变更 fail-closed、写失败即断开。当前已验证封闭 `-G`/`-V` 环境与 ORDER 回调的真实调用；真实认证、IPC 身份、PTY 与三平台 hook 行为未验收。不读取用户 config/agent/known_hosts，不托管私钥导出。

## 桌面设置（阶段五）

- 控制器与草稿（`lib/src/desktop/settings/`）：App/Profile/生命周期/TTF 的未决写入隔离、冲突草稿保留、显式核对后解除，query 门槛按请求身份区分；已删除目标的生命周期与字体入口安全禁用。
- 表单（`forms/`）：App 三字段、Profile 16 字段、结构化绑定编辑器（六类动作、F1–F24、Unicode、四修饰符）、数字原文保留。
- 资源区（`resources/`）：双全局字体槽复用既有字体管线、Windows 外观单写者、关于/许可。
- 键位条（`keybar/`）：完整双排编辑器、全应用单写者与跨关闭未决隔离。

待接线：共享入口路由、字体 RID/slot 确认、外观无 RID 协议的全应用 writer、`KeyBarWriter.shared` 统一旧写入口、真实 Rinf/SQLite 往返与 NativeFocus。组件测试不替代这些验收。

## 验证与 CI

- 已验证（Windows 本机）：Guo all-targets check、Rust 库测试、Flutter 全套与分析、rsHell core/session/storage 回归、helper 各 target、scoped russh 双 SDK 库测试与 RSA 擦除审计、本地 Shell 43 项、System scope/helper、C++ 输入组件 415 项断言。
- CI：检查点 push 触发的 `desktop-validation` 运行中，未计三平台通过。
- 未覆盖：macOS/Linux GUI 与沙箱、实体键盘/IME/指针、真实网络认证链、三平台打包与签名。

## 边界

- Rust 是终端状态、输入编码与 viewport 权威；不引入第二套解析器，不从硬件事件制造文本。
- 移动端与桌面 V1 协议、认证与信任策略不变；未验收能力保持默认关闭。
- 每阶段完整验收后独立提交；本检查点不是阶段四完成。
