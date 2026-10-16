# Wave A native caller 桥接准备

此目录只提供固定 `native-input-fixture-v1` 的同步 ABI 2 适配，不接生产入口。
普通 Widget、Dart、Rinf、SDK、Rust core、父宿主及 CMake 均不由此目录修改。

## 父的最小接点

1. **Begin**：父在实际 public SDK 回应已 Verified、fixture-only、ABI 2、pointer64、
   owners=0 后，从该进程寿命 PIN 映像 `GetProcAddress` 取得共享头的八个 typedef，
   填 `FunctionsTable`，标记 `ParentVerifiedSameSdk`。标签表达父的证据责任，
   不是认证实现。空表、缺函数、Unavailable 或未显式 opt-in 均不开 owner。
2. 持有不换地址的 `WindowSource`、`NativeInputBridge`、数值观察 sink；
   将 `bridge.callbacks()` 给 `SourceOptions`，再 `source.Create`。
   open 只在实际 HWND 已创建的 bind callback 中发生，`native_context=&source`。
   `SpikeOwner` 的 handle/context/session/generation/identity 全部来自工厂并原样保存。
3. **OwnMSGPreTranslate**：只给该 HWND 的消息在 TSF、TranslateMessage、Flutter 之前
   调用既有 `source.PreTranslate(msg, candidate)`。candidate 是既有 0..16 意图，
   Rust 先决定；bridge 不实现 chord matcher、默认 GUI action、mode cache 或 encoder。
4. **CheckOutput**：`CopyCapability` 只在绑定 UI 线程复制固定 capability。父通过
   表内 `inspect/peer_mode/peer_read` 观察真实 fixture；数值 sink 提供原 admission 的
   status/disposition/seq/action/byte_len。`GateReply` 没有 seq 字段；text permit 始终空。
5. **Retire**：UI 调用 `source.BeginRevoke`、完成 `source.Shutdown` 退栈，再将
   `ExportRetirement` 的固定 capability 交给专用非 UI 管理 caller 的
   `JoinOnManagementThread`。UI、key callback、MethodChannel 不等待 worker。
   revoke 确认只代表停止准入，worker_done 不是 join；join 也不是 DLL 卸载许可。
   Bridge 与 sink 必须长于 Shutdown 退栈及所有 callback，映像由父 PIN 至进程结束。

## 来源与拒绝生命周期

只映射 F1–F24、箭头、Enter/Escape/Tab/Backspace/Delete/Insert/Home/End/PageUp/Down。
可信 named 字段来自同一个 `NativeKeyEvent` 的 message/vkey/scan/extended/phase；
modifiers 只读取该事件保存的 queue keyboard state，并核验 aggregate 与左右键一致。
code 与 legacy 相同，不调用 ToUnicodeEx、不猜字符、US 表或 lowercase。
physical 为固定不可变 native domain 加真实 scan/extended；缺 scan 停 owner，绝不造 1。

键态不可得、RAlt/AltGr/synthetic Ctrl 无法排除、字符/dead/未知域、PROCESSKEY/PACKET、
消息字段矛盾或不合法 candidate 都以 Unknown 送 Rust typed reject。
bridge 的固定 32 槽只锁存来源／准入拒绝到原 physical Up，不复制绑定策略；满额停 owner。
本地 Busy 拒绝的 Up 也送原 physical 的 Unknown；Rust UI/policy ledger 保留拒绝至 Up。
Consumed/Encoded 正常映射；numeric action 只观察合法 Down 的 1..16，Repeat/Up 不再交付。
validate_text/cancel_text 返回安全 false，并查询真实 support；此波无文本或 paste ticket，
绝不返回 AllowNativeText、fake S_OK 或自造 ticket。
pointer、IME、composition、char 仍 Unsupported；这不表示字符输入永远不可实现。

## 固定数值观察步骤与证据层级

- F4 Down→Up 加 candidate14：binding bytes `1b 5b 33 7e`，无 action14。
- F5 Down→Up：binding bytes `7f`。
- F2 Down→Repeat→Up 加 candidate14：仅 Down 得 action1，后两次 action0。
- peer mode1/2/3 分别写实际 `ESC[>1u`/`ESC[>2u`/`ESC[>3u`，mode0 写 query；
  实际对端应依次读 `ESC[?1u`、`ESC[?2u`、`ESC[?3u`。
- Unknown Down 后同 physical 的可信 Repeat/Up 不产生 bytes/action；撤权后不复活。
- 暂停真实 peer read 后，inflight=1 的原槽跨 worker tick 仍 charged；恢复实际对端
  read 后 charge 才归零，不用“getter 数量”代替输出和在途观察。
- mode3 固定 F7 Down 为 `ESC[18~`（5 bytes），Up 为 `ESC[18;1:3~`（9 bytes）。
  压力前提是实际 admitted/effective payload 大于 128 且尚未 peer read，不是固定
  UI press 数。16 press／3000ms 有限预算未建立前提时明确失败，不给稳定通过。
  Busy Down 用 Repeat 无输出及原 Up 一次关闭生命周期，之后才是独立 press；
  Accepted Down 的 Up Busy 不补发 Up，仅在同 producer、无 remote baseline 下以新增
  seq 确认后处理，最后由完整字面 peer payload 证明唯一 Up。seq0／bytes0 不是完成。
  读前记录 next/last、charge/inflight 和生命周期状态；实际读按 offset 比较全部字面
  payload，拒绝缺失或重复 bytes，末尾空读且 full-write/charge=0 才通过。
- 非 UI join 后 inspect 应为 stopped=1、worker_done=1、worker_joined=1、charge=0。

`build_and_test.ps1` 使用既有 MSVC 19.44/x64/C++17/W4WX、SDK 10.0.22621；
拒绝覆盖确切临时目标，只清理本次产物。默认运行注入 spy，不称 actual Rust。
`-StandaloneFixture` 复用父只读 BootstrapHost 的 explicit fixture-only Preflight，
仅解析既有受控 DLL 的同映像导出，不复制 DLL，不模拟 SDK Verify。
测试创建隐藏、自有 HWND，不强设 foreground 或改变用户布局；自有线程 focus 不改变
原 foreground。PostMessage 路径也始终标注 synthetic-only，不称物理事件。
spy 来源测试仅改变新测试线程的 queue state，验证 event snapshot 而非 later state。
UI 只处理命令、消息和同步 gate，所有 output 等待与 join 都在非 UI 管理 caller。
测试宿主只允许一个命令持有共享块，使用单调 cmdid／匹配 ack 归还 ownership；post/wait
失败停止普通命令和共享结果读取，保留 UiRun 到明确 UI 退出，不把 timeout 当完成。
迟到 completion 不得归给下一命令。Shutdown 非 S_OK 时保留停用的
source/bridge/sink/root，后续正常 UI 入口继续清理；只有真实 S_OK 退栈才交接退休
capability 和退出 scope。`--safety` 的受控读前 barrier 与真实 held text-store 引用
验证这些合同，不以并发 UB 或活栈 delete 制造失败，不构成 actual Rust 证据。
初始化等待失败也不归还 ownership：专用管理路径先等 ready 发布或真实 UI thread exit，
此前不读取 root/created/capability/observations，不投递未发布 root。已发布后终结清理并
确认实际线程退出才释放 UiRun/handles；wait failure 未取得同步时保留 owner 并报告失败。
读前初始化 barrier 与独立 atomic watchpoint 验证此错误路径，默认初始化 timeout 仍为
5000ms；强制 timeout 仅属于受控回归。
即使真实 DLL 输出通过，也不证明实际 Flutter
进程中的同 SDK native caller、物理按键或完整 4C。实际同 SDK source 需父串行 main hook
和 integration 专用 entry；本目录不是生产启用或文本／pointer 验收。

验证限制：历史 standalone 间歇失败的具体原因尚未独立闭合；当前探针前提和当前
受控 candidate 的字面输出证据不能回填历史失败窗口，也不构成生产调度稳定性验收。
失败时保留固定数字诊断交父复核，不删除断言、不在 UI 等待或重放同一输入。
