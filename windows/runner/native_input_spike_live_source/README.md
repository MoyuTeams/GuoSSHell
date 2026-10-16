# Wave A 父级 native source 注入组件

本目录只交付 `SourceFixtureHost` 和组件 probe，不注册生产 terminal，不修改 SDK channel、
main loop、CMake、Dart、Rust、共享 ABI、依赖或 P1-control。默认 `FixtureOptions{}` 关闭：
不创建 HWND、owner 或 TSF；只有父显式开启的专用 fixture 能使用此组件。

## 父的最少接点

父在现有 runner 中编译 `source_fixture_host.cpp`、已稳定的 bridge、WindowSource 和
TsfProbe，包含共享 `native/hub/native_input_spike.h`。不定义
`GUOSSH_LIVE_SOURCE_LAYER_TEST`；该宏只用于本目录的 LayerSpy probe。

1. **SDK channel／专用界面**：父继续持有同一 STA/platform thread 上的
   `BootstrapHost`。真实 Flutter 公共 SDK nonce 回应经现有 `Verify` 成功之后，才调用
   `host.OpenSource(parentOwnedHwnd, instance, bootstrapHost)`。本组件只读取
   `verified_core_module()`，从该已 PIN 映像取八个共享 ABI 2 导出，并逐个核验映像范围、
   `VirtualQuery` 的 `MEM_IMAGE/AllocationBase` 与 `FROM_ADDRESS` 同 module。不重做
   Preflight／SDK 握手、不加载／复制／卸载 DLL，也不接受 Dart 提供的函数地址、native
   pointer、owner 或 capability 数字。父保留其当前产物指纹记录；旧 standalone 产物不是
   本组件的实际 caller 证明。
2. **main loop**：在 `GetMessage` 后、任何 TSF、`TranslateMessage` 和 Flutter 分发之前
   同步调用 `BeforeTranslate(msg, candidateIntent)`。只处理 self-created child 的同一个
   `MSG`；不将 native key 与另一个 Flutter event 拼接。`Handled` 不 translate／dispatch，
   `DispatchOnly` 只 dispatch，`ExistingRoute` 保留原路线。候选为既有 0..16 intent，
   此层不匹配 chord、不执行 GUI action。其他 HWND 完全保留原路线，不读取其键态。
3. **fixture 焦点**：父明确将自身窗口焦点交给专用区域时可调用 `FocusOwnFixture()`；
   仅当前焦点已属于父／其 child 时才聚焦本组件 child。没有 foreground 操作、全局
   keyboard listener 或布局切换。父必须避免既有 Flutter child 焦点路径抢回此区域。
4. **Dart 专用按钮**：父的 native handler 可串行调用 `TestNamedProbe(F1..F24, phase,
   candidateIntent)`、`TestUnknownScalarProbe(phase)`、`InspectNumbers()`、`PeerMode(0..4)`
   和 `ReadPeerBytes()`。
   handler 只返回固定数值或固定 fixture 字节，不能透传原生身份。probe 至多一条在途，
   使用 `PostMessage` 投递本组件 child；后续 matching MSG 的 candidate 只是一条合成
   intent 关联，不构成物理来源认证。`OriginEvidence` 始终为 `Unknown`。
   两种 probe 返回 Ok 仅表示排队成功；真正的 admission／seq／action 以随后
   `InspectNumbers` 的 observation 为准。它们共用原来的一条 occupied probe，不新增容量。
   `ReadPeerBytes()` 保留原 512-byte 缓冲布局，但实际 ABI 请求固定为 128，返回长度必须
   不超过 128；错误或超额回复清空 bytes／size，不能把更大的本地缓冲当作 ABI 许可。
5. **blur／deactivate／搜索／设置／认证框／窗口关闭**：父先调用 `BeginClose()`，之后
   通过正常 UI loop／定时入口 `PollClose()`。这不是新 GUI／密码 editable owner 的授权；
   父必须取得真实 core 撤权与 source 宿主确认，才允许后续 editable owner。

这里没有新增 channel 方法、Rinf ABI、P1-control keyboard route 或 Dart 消费端。
`Numbers.last.result.action` 只观察 intent，不在本波提前创建 tab／split／zoom。
父未来的完整 UI 消费端仍须重新核对旧 key/text/pointer consumer 的隔离与 action 票据。

固定负向接口的签名为：

```cpp
std::uint32_t TestUnknownScalarProbe(source::KeyPhase phase) noexcept;
```

该接口只投递固定 `VK_A` 的 Down／Repeat／Up，candidate 恒为 0；scan／extended 和 named
probe 一样使用当前 fixture HKL 的 `MapVirtualKeyEx`，不硬编码 US scan、不调用 ToUnicode、
不铸造 Some／NativeText ticket。实际同 HWND MSG 经父 pretranslate 和现有 bridge 分类为
Unknown，不产生 bytes／action，不进入 TSF／IME。root／minted context 与原 capability
保持绑定，disabled／wrong-thread／stopped／invalid-phase／occupied 和退休取消合同不变。
接口没有外部 vkey、字符串、raw header、地址或任意 Dart key 参数，也不开放字符输入、
非 US 文本或物理来源的新能力；父只需在自己的专用 handler 接这一个 native 方法。

## 单次能力与固定退休生命周期

- HWND、线程、父的 `HINSTANCE` 和 STA apartment 在创建前核验；实际 child 的
  `GWLP_USERDATA`、线程与 `native_context=&WindowSource` 由既有 bridge 在 bind 时核验。
  capability 的全部字段由 Rust 工厂返回，绑定后原样保存，不可 retarget／重铸。
- 每进程最多一次合格创建尝试、一个 root、一条管理线程 work、一个在途 probe、
  一份最后数值 observation；observation 上限 128，溢出停止 owner。source／bridge
  使用既有固定 32 槽拒绝 ledger 和 256 槽 trace。上限是本组件事实，不是 P1 heap 证明。
- UI 的 core 操作只沿既有 try-lock／fixed ledger 路径同步直返，没有 join、wait、sleep、
  actor 或重放。Busy Down 维持到原 Up；满额停 owner，不让 Flutter／IME 接管。
- `WindowSource::BeginRevoke()` Pending 时，UI poll 重试 bridge 的真实撤权；只有真实
  Confirmed 才调用 source 的 `ConfirmRevoke()`。source 的 `Shutdown()==S_OK` 完整退栈
  后才设置 `actual_shutdown_confirmed` 和调用 `ExportRetirement()`。
- Pending 时 root 保持 source、bridge、observation sink、capability 与尚未退休的 HWND／
  TSF 所有者。父应保留 parent 和原 UI thread，继续正常消息处理；不能用 Pending 当
  析构许可，也不能关掉 UI 线程后期待管理线程处理 STA 资源。
- 专用 management thread 只持有 root 中固定的 copied retirement；实际 Rust join 返回
  后 inspect，再 release-publish 固定结果区。UI acquire 看到完成才读取结果。管理线程
  不捕获 receiver `this`、不调用 Dart／GUI、不向旧 HWND 投递完成消息。发布后不再
  解引用 root；UI 在成功确认后关闭线程 HANDLE 并销毁 root，不等待线程。
- 成功退休还要求实际 snapshot 的 stopped、worker_done、worker_joined 为 1，charge／
  inflight 为 0。管理线程期间 UI inspect 返回 Busy，不并发调用 core 或读取未发布结果。
  DLL 继续 PIN 至进程退出；core join 不是 whole-hub／VM finalizer 的卸载许可。
- receiver scope 必须退出时，父优先移动保存 `DetachForUiPoll()` 返回的 native
  `RetainedOwner`，仍在原 UI 线程调用其 `PollClose/InspectNumbers`。receiver 析构或
  retained handle 在 Pending 时析构，都只把所有权留在唯一 orphan root；原 UI 可用
  `TakeRetainedOwner()` 接回，不能重复领取。移动／析构不释放活 source 或复制能力。
- 若父不再 poll、撤权不能确认、join／最终 inspect 失败或最后资源不能退休，固定 root
  保留到进程退出，关闭输入并占满唯一容量。不能热回退、再开 context 或销毁仍被借用的
  owner。线程创建失败未移交 work，可由下一次正常 UI poll 重试；已运行的 join 失败不
  自动重放。任何 lifetime 完成后也不允许新 context，迟到完成不重启。

所有 receiver／retained owner 生命周期操作由原 STA/UI 串行管理。跨线程只允许组件自身
的原子 handoff；外部不得在 UI 调用栈中并发销毁 receiver 或跨线程转移 STA 所有权。

## 原型能力与真实验证范围

core 的固定 12×3 fixture 绑定由唯一 Rust policy／实际 `DefaultTerminalEngine` 决定：
F1 Clear、F2 NewTab（action 1）、F3 Split、F4 `1b 5b 33 7e`、F5 `7f`、F6 `08`。
本组件不实现 matcher、encoder、engine、mode cache 或 GUI action 消费端。真实这些
绑定、模式切换和 stream 输出须由父在同 SDK caller 上核验，不以 LayerSpy 响应通过。

named fields 只复用既有 bridge 的同 `NativeKeyEvent` message／scan／phase／queue state
校验。字符、dead、AltGr／RAlt、PROCESSKEY／PACKET 和来源不足保持 Unknown；不放行
native IME。`TextRawLegacy=Some` 必须由同事件来源证据支持，HKL metadata 的存在不是
未移位字符证明。本组件不作字符／dead basecode 推断。ABI 2 没有 NativeText ticket；
`TextImePointerSupport()` 明确 Unsupported，child 的 char／IME／pointer MSG 以
`E_NOTIMPL` 拦截，不返回假成功。

## 编译与组件测试

```powershell
& windows/runner/native_input_spike_live_source/build_and_test.ps1 -OutputDirectory <opencode 临时父目录>/guosshell-wavea-live-source-20261012
```

脚本复用已可用的 MSVC x64／SDK，以 C++17、`/W4 /WX` 同时编译父注入版和 LayerSpy
版。共享头直接提供尺寸／对齐断言，不另写 ABI 副本。输出只允许上面的全新 exact target，
拒绝覆盖；确切七个编译产物登记并在 finally 删除，不递归删除或清理其他 worker 资源。
没有安装、SDK handshake、Rust build、Rinf start、生产配置访问或 DLL 加载。

六个独立进程的 scoped cases 覆盖：默认 off／unverified gate／非法 instance；实际自建
child HWND 和同线程绑定；F1–F6 Down／Repeat／Up 参数和数值回传；唯一 HWND 处理范围；
固定 peer payload／严格 128 请求与超额返回拒绝；char／dead／IME／pointer Unsupported；
固定 VK_A 的三条真实排队 MSG、candidate 0、同 immutable physical/context、Unknown
零 bytes／action／TSF、跨线程／非法 phase／occupied 拒绝；来源 Unknown 和 AltGr 拒绝；
Busy Down 锁存／32 槽满额停用；真实撤权 Pending 与持有 text-store COM 引用时的 root
保留；显式／析构 retained 转移、唯一领取、移动 handle 和最后 Pending handle 析构后的
接回；receiver 已消失时实际非 UI join、原子完成和禁止再开 lifetime。

测试响应仅是 **LayerSpy**，PostMessage 是 **synthetic**，origin 是 **Unknown**。
真实 Windows HWND／TSF 生命周期属于组件证据，不等于真实 Rust caller。没有测试真实
Flutter SDK live、真实 current DLL 的 policy／transport、物理按键、IME commit、pointer
交互、正常 GUI 回归或完整 G1–G4；这些仍由父完成集成与现场核验。所有非本组件文件与
生产入口保持本任务不写的边界。
