# Windows 原生文本与布局单元

本目录是显式 opt-in 的应用私有单元，不自动注册窗口、线程、subscriber 或 TSF manager。
它不包含共享 ABI、Rust matcher/encoder、Rinf hub loader、SSH transport、Dart consumer 或生产 root 接线。
普通 runner 的 CMake、主循环与默认路径不引用本目录。

## 父接线责任

`contracts.h` 的结构是 C++ 责任接口，不是另一份 FFI layout。父适配器须把
`OwnerRef/TicketRef/GeometryRef` 映射到共享 header 和 B 的实际 records。
`Callbacks` 所有方法必须同步、nonblocking，不等待 actor，不持 core 锁调用 COM。
原生对象通过 `TextStore::Create`、`NativeSurface::Create`、`LayoutTable::Create`
进行 fallible allocation；构造前必须已有 grant/`NativePermit`。`NativePermit::Memory`
指向同授额 root 的 allocator，allocator 寿命必须由 permit 覆盖。Layout 的 shared
control block 也经过该 allocator，按实际申请尺寸计费；Build/DiagnosticQuery 不替换
已有 permit。分配失败返回空对象，已占内存按原 allocator 释放并归还原 grant。

1. 已授权 builder 先登记固定 source/context、授 native permit，再由
   `ContextPool::Create` 创建实例。默认来源为 `VerifiedNative`；只有独立受控
   fixture 明确指定 `DeclaredSynthetic`。登记本身不构成物理输入证明。
2. 传入应用 UI 线程已激活的 `ITfThreadMgr` 和 `TfClientId`，调用 `Open`。
   自有终端 HWND 的 Win32 焦点就绪后调用 `ActivateDocument`。
   本单元不抢前台、不替调用方设置 Win32 焦点。
3. 在 own HWND 的每条键消息出队后、任何 TSF Test/Key、TranslateMessage、
   DispatchMessage 之前调用 `Dispatch`。`owned=true` 的原键不得再交旧窗口路线。
   Gate 唯一负责原 chord 绑定仲裁；未知 raw 即便错误获 `AllowNativeText` 也不调用
   TSF/Translate，并取消返回的同 context 票据。
4. 同一出队点调用 `DispatchPointer`，不要在 WndProc 或 Flutter 回调中补读 modifiers。
   `HandleOwnedMessage` 在 own WndProc 的 DefWindowProc 前调用，只截断第二 char/IMM/
   pointer 消费链并处理失焦、布局变化。父须在每次 native dispatch 完成后以及 TSF
   异步 edit session 完成的 UI 周期调用 `Pump`；不能让已有 async lock 无唤醒地挂起。
5. `ReservePayload` 在 native buffer 接受文本前授整个有界事务；`Payload` 只填原票据，
   offset 连续、UTF-8 chunk 不切 scalar，最后由 `Finish` 封口。回调不得在每个 chunk
   提前发送或复用 permit；B queued/inflight 各自仍保有原 grant charge。
6. SourceGeometry 必须是 core 已确认的 frame/viewport/font/transform。编辑文本的
   glyph/ACP rect 通过 `SetTextLayout` 同 edit revision 提供；不能用固定 cell 宽度
   猜 emoji、wide glyph 或多行 composition 的候选窗范围。无有效 rect 返回
   `TS_E_NOLAYOUT`，空 document 可用已确认 cursor rect。

## 文本、原票据与退役

`TextStore` 实现 `ITextStoreACP`、`ITfContextOwnerCompositionSink`、`ITfTextEditSink`。
document/context 由真实 TSF manager 创建并 Push、AssociateFocus；text-edit sink 在
context 上注册，profile activation sink 在 thread manager 上注册。

- UTF-16 ACP 的读写、选择、query/insert、read/read-write lock、异步升级均有实际实现。
  不接受孤立 surrogate，也不把成对 surrogate 截开。只有 embedded/rich formatted
  text 不支持；属性列表为空且查询结果一致。
- TSF 自身的 SetText 不反向发 OnTextChange。应用 Translate edit 的 change/selection/
  layout 通知只在 lock/RequestLock 栈外发出，通知及其只读 TSF edit session 仍属于
  原 dispatch scope。
- 一个 store/context 只绑定一个 ticket，跨键 composition 保留这个 ticket。
  `OnEndEdit` 枚举真实 TSF composition ranges，维护 frozen prefix 和可编辑区；
  preedit 不经过 payload 出口。最终只有 store finalizer 提交，重复 callback 不重复
  Finish，不按文本内容去重。
- **封口后不能在这个 context 再 Bind 新 ticket。** 父须等待 core lifecycle/revoke
  fence、native retirement，登记新 ContextId 并创建新 context。这个保守轮换规则避免
  无事务标签的旧 TSF callback 被解释成新事务；不是已完成的主界面连续输入接线。
  只平衡已封口 ticket 的 Up 可由 `lifecycle_only` 获 TSF KeyUp，不得开新事务。
- 没有排他独立编辑证明的异步非 composition edit 被拒绝；本单元不伪造
  `begin_independent_text`。父后续若需要该能力，须提供原 edit session 的可证来源。
- `BeginRevoke` 先进入 CoreRevoking。回调返回 false 表示 Busy，UI 不等待；父在真实
  core fence 完成后送回 `CoreFenceCompleted`。只有 NativeRetiring 的 composition
  termination、focus/association/sinks 解除、外部 store COM 引用释放和派生队列清空后
  才 `ConfirmRetired`。超时不构成退休。新 dialog/password 的 editable 状态由父在
  此确认之后启用；本目录不采集这些控件。
- fixed pool 有 8 个槽；未退休、外部引用仍活的槽不能复用，ContextId 单调且不复用。
  池满拒绝新准入。native permit 随 store/context/tombstone 存活，不在提交时提前归还。
  最后 Release 将 permit 移到析构栈外，直到对象及其 backing allocation 真正释放才退款；
  Layout 的 weak control block 同样保有 allocator permit 到最后一次 deallocation 返回。
- 可重入 callback 的活栈持有自身 COM root；这只保证内存寿命，不代表输入仍有效。
  每次消息泵/TSF 调用返回后须再次核验原 event/context/ticket/scope；撤权、封口或 scope
  失效后不再调用下一段 TSF/Translate。确认回调期间仍为 NativeRetiring，重入 Pump
  只能得到 pending；失败可在回调退栈后重试，不能先发布 Retired 或回收 pool 槽。
- RequestLock 的单 pending 槽合并权限。READ 不能抹掉先前接受的异步 RW；每轮授予
  有界次数，其余留待 Pump。容量拒绝在读取传入 UTF-16 内容之前完成。

## TranslateStore 与 dead 状态

TSF Test/Key 只在同源可信 GateAllowNativeText 和原 ticket 就绪后发生。
ConsumedDeferred、TerminalDeferred、Rejected 均不进入 TSF/Translate。
TSF 未处理且有 edit/selection/composition 副作用时取消 `AmbiguousTextRoute`。

Translate 只调用一次，只对原 HWND 在当前 scope drain：普通键为
`WM_CHAR..WM_DEADCHAR`，system 键为 `WM_SYSCHAR..WM_SYSDEADCHAR`。
两类派生队列必须在开始前为空；scan、extended、system context、repeat count 须吻合。
另一类派生消息、超过固定 drain 限额或未知归属使该路线关闭，原事务取消。
不会取走 `WM_SYSKEYDOWN/UP`，也没有 IMM RESULTSTR、WM_IME_CHAR 或
DefWindowProc 的第二提交链。

当前没有 kernel dead-state retirement 的设备证明。已知 dead metadata 在 Translate
之前返回 `DeadStateRetirementUnproven`。若 OS 意外产生 dead 通知，则保留 dead-pending
并禁止确认 retirement；不能凭销毁 HWND、超时、typed reject 或清空本地字符串启用
新 editable。没有 mutating ToUnicode、合成清除键或切换用户布局。
这个出口是完整 dead 支持的缺口，不是删除 dead 支持的最终验收。

## 布局 metadata

`LayoutTable::Build` 由**既有**已授额管理 worker 调用，本单元不创建线程。
输入为 UI 同 MSG 的 HKL/profile/epoch、worker clean/no-Translate/no-Attach/no-context
生命周期证据，以及 documented kernel isolation 和 UI dead 前/中/后实验证据。
任一缺失返回 `LayoutStateNotIsolated`，不能启用字符 raw。

表为固定 `512 scan × 8 Shift/Caps/AltGr states × 8 UTF-16 units`。
用 MapVirtualKeyEx 往返核验 VK/scan，ToUnicodeEx 只使用 flag 4。
去 Shift/Caps 的 raw 与保留 Shift/Caps 的 legacy 分别来自实际查询；结果区分
无字符、dead、单 scalar、多 scalar，不猜 US、不 lowercase、不截首 scalar。
查询结果只作 metadata，不能用来生成 NativeText、Kitty/mouse bytes。

`DiagnosticQuery` 仅供已授权受控诊断，结果始终 `Verified=false`，不能通过 Lookup
成为可信字符来源。runtime 持有原表 shared lease 到 context 释放；换表或同 HKL 的
profile 通知都先 revoke 原 context。父必须提供新的、完整 profile revision。
队列态的 LeftCtrl 无法区分物理 Ctrl 和 AltGr 合成 Ctrl；该组合及右 Alt 目前返回
`AltGrUnverified`，保留原 modifier，不猜测 meta 或删除 Ctrl。

文档依据：

- [RequestLock](https://learn.microsoft.com/en-us/windows/win32/api/textstor/nf-textstor-itextstoreacp-requestlock)
  与 [Document Locks](https://learn.microsoft.com/en-us/windows/win32/tsf/document-locks)
  规定同步拒绝、异步升级及通知边界。
- [ToUnicodeEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-tounicodeex)
  明确 flag 4 不改变状态，也明确已有 kernel dead state 可能影响结果。
- [GetKeyboardState](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getkeyboardstate)
  的线程队列隔离声明不是额外的 kernel dead-state 隔离证明。
- [Profile activation sink](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nn-msctf-itfinputprocessorprofileactivationsink)
  安装于 thread manager 的 ITfSource。

## 固定预算与验证边界

每 store 固定 UTF-16 4096 units、UTF-8 12288 bytes、4097 个 ACP rect，加 COM/sink/
selection/transaction 状态；实际 `sizeof(TextStore)` 是父授额的下界，不是 OS TSF
内部内存峰值证明。每 Translate dispatch 另有固定 4096-unit 临时数组；表 cell
storage 为 131072 bytes，另计 LayoutTable 控制字段。父还须计 pool、shared permit
控制块、OS COM 分配、B queued/inflight、上下文同时存活成本。

`native_fixture.cpp` 使用 run-owned 隐藏窗口、真实 Windows TSF manager/document/
context/edit session 与真实 TranslateMessage，文本全部为固定 synthetic fixture。
输出只含 numeric labels/statistics；不打印用户文本、profile 标识或其他窗口信息。
独立注入 callback 证明 native 时序，不证明真实 Rust gate、同 Rinf hub 或 SSH bytes。

fixture 覆盖 ACP/UTF-16、同步嵌套拒绝/异步升级、真实普通/空/大文本、跨 scope
composition 和取消、相同文本重复事务、payload failure、late-context 引用、geometry、
有限派生 drain、同 HKL epoch 失效和池满额。profile callback 的直接调用只测失效机制，
不是实际 TIP 切换；composition 的 RequestEditSession 不是物理 IME 输入。
fixture 编译限定的 boundary probe 只在本实例重入自有 HWND 消息或取消回调，用于验证
失效后零后续 native 副作用，不是全局 hook，也不冒充自然发生的跨线程 PeekMessage 事件。
scoped allocator 观测真实 allocate/deallocate 的尺寸、授额与退款顺序；失败分配和最后
COM caller 在 callback 内释放均有行为检查。这些检查不代表 OS TSF 内部分配预算已通过。

运行 `build_fixture.ps1` 时指定新建的任务临时输出目录；脚本只用既有 MSVC v143、
Windows SDK 和 C++17，拒绝覆盖已有目录。`-Cleanup` 只删除本 fixture 已知产物及空目录。
`-LibraryOnly` 仅编译三份 native 源码，不定义 fixture probe 宏、不链接或运行测试宿主。
清理拒绝目录/产物 reparse point 和类型变化；最终使用非递归 Directory.Delete，遇到
未知文件直接失败，不提示确认、更不递归删除。
实际物理输入/IME、非 US、dead 取消隔离、worker 隔离、AltGr、候选窗视觉验证，以及
父主循环/Dart/Rust/pointer reporting/GUI action/password fence 接线均需独立验收。
本目录的 hit-test 与同消息 modifiers 回调**不是完整 Pointerreport 实现**，也不是
Windows Wave C、三平台 4C 或 P1 的完成证据。
