# Windows Wave A 独立输入探针

此目录是默认关闭、仅固定 fixture 的应用侧可行性探针，不是 4C／P1 验收，
也不是生产启用许可。没有接入现有 runner／Dart／Rust；不加载第二个 hub，
不实现编码器、绑定表或协议 mode 缓存。

## 父串行接线责任

`window_source.h` 只定义 C++ 责任接口，不是 Rust ABI。父须按真实同一 hub 的
exports／共享 header 适配 `BridgeCallbacks`，不得直接把结构体跨 FFI 传递。

- `bind(user, created_hwnd, identity)`：仅在明确 opt-in、固定 fixture、自有
  HWND 创建后向同 hub 工厂登记。返回的 cookie 关联原 source/session/generation，
  `context_epoch` 不可复用、溢出停用。数值本身不是可铸造 capability。
- `key`：同步 nonblocking disposition；读取原 capability 的真实 policy。
  `NativeKeyEvent` 保留原 MSG、scan/extended/phase、取队列事件时的键盘状态
  与布局。它们不是可信 raw/basecode 的证明。父负责 Unknown／已知域外／可信
  分类；不准为了 IME 把 Unknown 当作无绑定。
- `default_candidate_intent`：父注入候选意图，native 不匹配／执行 16 个候选。
- `validate_text`：确认原 immutable context、事件和已预约 ticket 的真实关联，
  才能进入 TSF。测试 callback 的数值只是合成 fixture，不是真实 Rust ticket。
- `cancel_text`：本波无实际文本能力，TSF 探测后取消原预约；必须使用原 context，
  非阻塞且有预留通路。失败停止 owner 并请求撤权，不能放行旧消费者。
- `revoke`：停止新 admission 后确认 core 撤权。Pending 保留停用 context；
  父取得真实确认后才能 `ConfirmRevoke`，随后退休 TSF／宿主确认。

`user` 与 context 从绑定到清理完成不可换成 current pane。函数指针要求
`noexcept`，bridge 也须隔离 Rust unwind；本目录没有 C++ 字符串分配／FFI。
所有入口和析构在创建的 STA/UI 线程调用；COM 初始化由宿主负责。

创建期间的撤权、宿主失活、`WM_KILLFOCUS`、销毁和 `Shutdown` 都锁存停用。
bind 尚未返回身份时，撤权只能返回 Pending；Accepted 返回后必须对原身份
实际调用 revoke，不能重新启用 admission 或初始化一个已停用 HWND 的 TSF。
Create 在 bind 和 TSF 初始化后都复核窗口与停用状态，停用后不返回成功。

message loop 在任何 Flutter key、TSF、`TranslateMessage` 之前串行调用：

```cpp
const auto decision = source.PreTranslate(msg, candidate_intent);
if (decision.route == LoopRoute::Handled) continue;
if (decision.route == LoopRoute::ExistingRoute) TranslateMessage(&msg);
DispatchMessage(&msg);  // DispatchOnly 不经过 Translate
```

未创建、未 opt-in 或非本 source HWND 时完全返回 `ExistingRoute`，不观测输入。
被消费、编码、拒绝或 Busy 的键不进 TSF／Translate／Flutter。直接 dispatch
到本 HWND 的 key 是 `MissingPreTranslate` STOP，不是假装 WndProc 拦截够早。
父还须仅对该 fixture 在宿主失活时调用 `NotifyHostDeactivated`，避免现有激活
逻辑把焦点交回 Flutter child；本目录未修改这些共享 hooks。

## 文本与资源边界

真实建立 TSF thread manager/document/context，`ActivateEx` 不激活键盘布局。
仅在 gate 返回 Allow、可信 raw 标记、同源合法 ticket 和真实焦点均成立时，
才调用 `TestKeyDown/Up`；仅其报告 eaten 时调用 `KeyDown/Up`。

`ITextStoreACP` 的 COM 身份／sink／只读状态／窗口查询真实实现；ACP 锁、编辑、
selection、attributes、文本／候选 geometry 返回 `E_NOTIMPL`。不 grant 空锁，
不假报空 store 成功，不保存／提交 composition 或 commit。因此本波最终
`TextStoreUnsupported` STOP：不支持普通文字、dead／AltGr、真实 IME、独立提交
或跨键 composition 票据，不得据此接通实际文本编辑。

不调用 IMM result、`ImmGetVirtualKey`、`ToUnicodeEx` 或第二条字符提交链。
`VK_PROCESSKEY`／`VK_PACKET`、无 scan、缺键盘状态和未知 raw 不能获准 TSF。
派生 `WM_CHAR`、`WM_IME_CHAR`、IMM composition 均为
`CharacterAssociationUnsupported`，没有 FIFO 配对、内容去重或 ignore-nextChar。
Pointer 为明确 `PointerUnsupported`；没有 same-event pointer 完整实现，不能
据此关闭父既有 pointer 路径。

每个 source 只绑定一次，不改 owner／epoch；本地 Busy 拒绝 ledger 32 槽，
Repeat 不重新 gate，Up 只通知原 bridge 释放，不复活文本。满额停止 owner。
trace 固定 256 槽，满额停用；仅包含 event ordinal、阶段、ticks、HRESULT，
不含用户文本。TSF store 存活／退休合计最多 4 个；外部或 ACP 在途 COM 引用未释放时
`Shutdown` 返回 `RetiredReferencesPending`，保留旧 HWND，不回收贴新 epoch。
确认退休后每次最多清理 64 条仅属于旧 HWND 的排队消息；超额返回
`RetiredMessagesPending` 并保留停用 HWND，由正常 loop 后续处理，不无限 drain。
这个数量不是整个 core/outbox 或 P1 的资源证明。

父必须维持对象与 bridge 生命周期，直到 `Shutdown == S_OK` 才可销毁对象、
确认宿主退休或交给下一 editable owner。析构兜底只关闭本探针资源，不能把它
视为 Rust 撤权或真实输入法晚回调安全释放证明。撤权 Pending、取消失败、
TSF HRESULT 失败及退休引用不释放都是明确 STOP，不等待 actor、不启用新 owner。

所有 Source 入口（含 `WindowProc` 的完整本对象窗口回调）保留在途深度；
bind/key/validate/cancel/revoke、TSF Initialize/Focus/TestKey/Key/Retire/AcquireStore
及 TSF 析构另保留外调深度。在任何外层入口或外调未退栈时，重入 `Shutdown`
只停 admission，返回 `E_PENDING`／`InFlightCallPending`，不授予销毁许可。
顶层 `Shutdown` 自身不阻止清理；退栈后重试，精确退休与清理完成才返回 `S_OK`。
外调未返回时也不允许重入退休正在使用的 TSF。非法直接 delete 在途对象仍属
调用方违反生命周期契约，析构不能替代撤权确认。

客户端独立调用 ACP `AdviseSink`／`UnadviseSink` 时，本对象另持无堆分配的
scope self-reference，不依赖 Source 入口深度。引用从首个外部 QI 前持续到
全部候选／关联 Release 和 active／revision 访问结束；最后一次 scope Release
可能析构本对象，之后不再访问其成员。client 在外调内释放自己的 store ref 后，
root 与在途 self-reference 仍共同阻止 `CanRelease()`；退栈释放 self-reference
后才只剩 root。临时引用只是对象存活保障，不是 Source 销毁许可；ACP 方法的
`S_OK` 也不是宿主退休确认，仍须正常顶层 `Shutdown` 完成撤权、退休、引用检查与
队列清理后返回 `S_OK`，父才可销毁或交接 editable owner。其余 ACP 方法没有
外部接口调用，不增加编辑／锁／属性／geometry 支持；IUnknown 保持标准引用计数语义。

sink 的 canonical identity 与接口候选仅在局部持有；两次外部 QI 后都核验
active 与关联 revision，变化返回 `E_ABORT`，停用返回 `CO_E_OBJNOTCONNECTED`，
不会覆盖重入安装的新 sink。相同 canonical identity 的重复 Advise 仍更新 mask，
不增加关联引用。Unadvise／析构在任何外部 Release 前成对摘除 sink、identity 与
mask；单个 COM 指针也先摘除再 Release。revision 不回绕，耗尽停用。Stop 只
关闭 admission 并失效候选，不在 core 确认前释放系统关联；退休／Unadvise 再清理。

## 独立编译与验证

`build_probe.ps1 -OutputDirectory <批准 temp opencode 下的确切新目标>` 使用既有
MSVC 19.44.35207.1／v143、SDK 10.0.22621.0、C++17，`/W4 /WX /utf-8`，
链接系统 `kernel32/user32/gdi32/ole32/uuid`。TSF 使用 COM 激活，不依赖额外 msctf import
library；没有安装操作。输出目标叶名固定为
`guosshell-wavea-windows-probe-20261008`，拒绝覆盖已有目录，逐个清理本次创建的
三个 obj 和 exe，不递归清理。只编译本目录，不调用 Cargo／Flutter 或正式 runner。

测试宿主创建自己的可见根窗口与输入子窗口；仅严格模式请求本次根窗口 foreground，
仅 `PostMessage` 合成 key 到本次 HWND，不使用全局 hook／SendInput、不改布局。
两轮 OS queue barrier 观察 gate／TSF 顺序、consume/reject 的零 char、Busy
锁存、同源 ticket／未知 raw 拒绝、重入撤权、Pending 确认、迟到 COM callback
拒绝、退休 cap、有限队列清理、漏接 loop 的 STOP、外部销毁与本次窗口／COM
清理。callback 是合成 disposition，故只证明
原生 gate 与消息路由，不证明真实 Rust engine／同 hub／真实输入字节。

`-Cases full` 为默认严格模式，保留自有窗口 foreground 门槛。
`-Cases review-findings` 仅运行重入复现；`-Cases synthetic-all` 运行同一组原断言
与重入断言。两个合成 selector 以非主动激活方式显示根窗口，不请求 foreground，
只运行自有 UI 线程消息，仍检查本线程焦点 HWND，并明确打印
`foreground=UNVERIFIED full_foreground_acceptance=UNVERIFIED`，
不能替代严格模式的前台证据，也不能证明实际设备或输入法输入。

COM 重入 fixture 使用实际 `ITextStoreACPSink` QI／AddRef／Release，在首次
canonical QI、第二次 sink QI、Unadvise 与成员 Release 内注入有限重入，并检查
存活引用及最终引用归零。`InitializeStoreForProbe` 仅实例化同一个只读 ACP 实现，
仍共享 4-store 容量，不注册系统 TSF；这些合成回调不代表系统 TSF 曾实际触发。

ACP 在途 fixture 覆盖 Advise 首次／第二次 QI、Unadvise QI、相同 identity
更新 mask 时的候选 Release，以及 Unadvise 的最后候选 Release。外调内确切释放
一次 client store ref 并 Retire，仅观察释放资格，root 始终保留到外层方法返回，
不删除活栈对象。断言检查外调中 root + self 的 2 refs 不可释放、方法返回后仅余
root 1 ref、停止关联清理、sink 引用最终归零及 root store 清理。

无交互 session、严格模式的自有窗口不能 foreground、现有 TSF 不可激活时打印具体 STOP，
不能把未执行当通过。真实设备按键、非 US／dead／AltGr、标准 IME commit／
候选窗、实际 focus 切换晚回调、mouse reporting／selection、同 hub Rust ABI
与 stream 均须由后续波次单独取得证据；生产默认-off 构建回归由父接线后验证。

## 本波实际证据

本机既有 x64 工具下，独立编译／链接通过，实际运行报告
`compiler_msc_full_ver=194435207`、`pointer_bits=64`、`source_bytes=8768`。
`synthetic-all` 实际运行保留的 82 个断言与 ACP 在途的 20 个断言，
`summary_checks=102 summary_failures=0`；foreground 报告 UNVERIFIED，未请求前台。
严格 `full` 的既有运行停在 `run_owned_foreground_unavailable`，本轮未重试，不能
报告其完整前台验收通过。根窗口与子窗口／window class、COM store、合成 sink
和四个临时构建产物均完成清理。

允许的合成 Down／Up 实际各调用一次 `TestKeyDown/Up`，均未报告 eaten，
故 `KeyDown/Up` 调用计数为零。这证明 gate／ticket 校验先于实际 Test 调用，
不能冒充已观察到标准 IME 的 KeyDown／commit 链。原型默认关闭没有创建
HWND／注册 TSF／调用 bind；这不是尚未接线的正式 runner 默认-off 回归结果。

真实同 hub Rust、GNU/MSVC ABI、stream、物理设备、非 US／dead／AltGr、标准
IME、完整 pointer 与生产路径均未验证；当前文本与 pointer 路径明确 Unsupported。
