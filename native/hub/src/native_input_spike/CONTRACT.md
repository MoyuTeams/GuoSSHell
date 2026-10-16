# 原型 B 私有契约

本目录仅用于显式 opt-in 的测试终端。没有生产 Prepare、真实配置仓库、SSH 凭据、
Rinf/V1/mobile 接线或生产资源证书。共享 header 和各平台 loader 由父整合者唯一维护。
Rust `repr(C)` 定义是实际布局来源；这里不是第二份可编译 header。

## 私有 ABI 3

既有 open/key/revoke/join/inspect/peer_mode/peer_read/support 与以下三个导出同属 hub：

- `guossh_native_input_spike_command(cap, Command*, Reply*)`
- `guossh_native_input_spike_projection(cap, Projection*)`
- `guossh_native_input_spike_copy(cap, ActionLease*, bytes, capacity, count)`

GNU x64 实测布局（其他目标须分别核验）：

| record | bytes |
|---|---:|
| `SpikeOwner` | 48 |
| `SpikeOpen` | 56 |
| `SpikeKey` | 96 |
| `SpikeAdmission` | 80 |
| `SpikeSnapshot` | 176 |
| `TicketRef` | 40 |
| `PayloadGrant` | 16 |
| `ActionLease` | 32 |
| `Command` | 136 |
| `Reply` | 120 |
| `Projection` | 928 |

所有 request 的 version/size 必须精确相等。`SpikeKey.native_event` 非零，
fixture `layout_revision=1`；`same_event_state` 是适配器冻结的同事件状态，不从当前
Flutter modifiers 补造。布局更换须撤销旧 context，不能改写活 owner 的 revision。

open 必须显式提供 origin：1 为 DeclaredSynthetic，platform=0；2 为 VerifiedNative，
platform 为 Windows=1/macOS=2/Linux=3；registration 非零。VerifiedNative 的权限前件
是 app-native builder 已验证 source-owned surface、UI 线程及排他 terminal scope。
数值检查不证明 HWND/NSView/IMContext 的 OS 来源；它不是从 Dart 数值签发能力的入口。
合成事件不能登记成 physical/native-text 验收证据。

`SpikeOwner` 的原 context、handle、source、generation、identity 必须全部匹配。
`TicketRef` 固定原 owner/context/slot generation/transaction/effect。异步 closure 持原
ticket，不查询 current pane 或 latest text。所有指针只借用至 FFI 返回，无跨调用保存。

`SpikeSnapshot` 独立报告 scope/root delta、captured profile revision/cell metrics、
frame revision、sync epoch、remote received/applied 和撤权/退休状态。charged_bytes 是
queued/inflight chunk window，retained_bytes 是完整 payload/编码结果的保留额度；
二者不混为一个峰值，也不将这些观测字段冒充 TicketRef、PayloadGrant 或 admission receipt。

### Command

公共字段顺序为 version/size/opcode/flags、ticket、grant、args[6]、bytes/length。
flags 只接受 0/1，文本调用不超过 4096 bytes，chunk 另限制 32 bytes 且完整 UTF-8。

| opcode | 行为与参数 |
|---:|---|
| 1 | 原 ticket preedit；args[0]=严格递增 edit revision，bytes=当前 preedit |
| 2 | payload grant；args[0]=完整输入容量。Paste 必须已到 cursor 才授额、开始 clipboard read |
| 3 | grant chunk；args[0]=精确连续 offset，flags=final，bytes=完整 scalar 分块 |
| 4 | finish；args[0]=commit revision。相同已完成 revision 返回 kind=0，不重新发送 |
| 5 | cancel 原 Open ticket；锁忙时进入固定 completion mailbox，不申请普通 effect 槽 |
| 6 | 独立 native edit；args=context/edit_scope/retired_through。scope 递增，前一 text 必须已闭合；拒绝硬件事件后不再给该 context 独立 text 权限 |
| 7 | ordered control，见下表 |
| 8 | 仅 claim 队首 ActionLease 一次；无可 claim action 返回 Busy |
| 9 | ACK；args=owner/context/effect/action/target/result，result=Applied(0)/Failed(1)/Cancelled(2) |
| 10 | 同 9，确认 action 已进入 pending-revoke，再线性化 core revoke；不是等待自己的 fence |
| 11 | core revoke；锁忙使用固定 revoke cell。之后只有原 ACK/retirement/inspect 可达 |
| 12 | 原 context 的 native retirement 确认；必须已 revoke，且所有 claimed action 已 ACK |
| 13 | 只读 cursor 状态：effect=已 apply effect，kind=core revoked；不签发 receipt/grant/ticket |
| 14 | 仅 fixture binding chunk；args[0]=1..32，且没有在途槽。text/paste/key encoder 仍用 32-byte scalar chunk |
| 15 | 取消当前 deadline notice 并从真实 engine 原绝对 deadline 重新 arm；不重置 timeout |
| 17 | fixture pointer；args=frame/column/viewport_row/button/kind/modifiers，进入相同 cursor |

独立 edit 的 native caller 必须证明 edit_scope 的原生排他、前一 context 回调静默，
不能在收到旧异步 callback 时临时取最新 scope 数值。没有该 native 前件不能调用 6。

control 的 args[0]：1 resize(cols,rows)，2 clear，3 selection(start row/col,end row/col)，
4 copy，5 viewport(top)，6 FrameAck(revision)，7 action(既有 1..16)，8 clear selection。
Paste action 返回原 ticket，不派发可以绕过票据的普通 GUI action。

pointer 的 button=Left(0)/Middle(1)/Right(2)/WheelUp(3)/WheelDown(4)，
kind=Press(0)/Move(1)/Release(2)/Scroll(3)。修饰位同 key。它只是 B 有序接缝，
不代表 C 的完整 gesture、OS capture、滚轮策略、双击词边界或 candidate window 已验收。

### Receipt 与完成

admission.seq 是主准入序号，effect 是原 effect 槽；member/grant/update/finish/ACK
各有 receipt，但不会为同一 composition 新增输出位置。key admission 的 byte_len/action
不冒充已经发送的 bytes 或已执行的 GUI action。disposition：0 reject、1 deferred、
2 consumed/no-send、3 原 native text lifecycle（闭合 ticket 的 Up 不准新建 text）。

Open text、write pending 和 claimed GUI action 都阻挡后续 engine/Screen/GUI effects。
从队列取出 packet 不退款；同一个写 future 持续至完成或明确关闭，不取消重启 partial
write。多 chunk 保留同一原 effect。physical Up 之后，cycle 仍持有 Release 槽，直到其
最后一个 write/ACK 完成；晚到 Unsupported Down 会拒绝其所有延期 Repeat/Up。

copy 只能读取仍 Claimed 的 Copy lease，输出最多 128 bytes，属于固定 geometry 的
fixture 上界。调用返回后才做外部 clipboard 写，随后 ACK。projection 只读原 source
的 frame/effect tuple；Arc 提取有固定 lease，锁外转换 fixed cells 和写 caller buffer。

support：1 pointer/2 text/3 paste 为 fixture 接缝，不签发完整 native 适配资格；4 生产控制拒绝，
5 返回 ProductionNotReady(11)，6 返回 ScopedNativeTransportUnavailable(12)。
TextCapacity(13) 取消超额事务；ActiveKeyBudget(14) 不驱逐旧物理生命周期。

## SyncDeadline 源码前件

唯一 engine 的实际地址、source generation、checked mutation epoch 和真实 absolute
deadline 组成 witness。只有 advance、resize、clear、end_sync 会在本接缝调用可变
engine；每次调用前递增 epoch。encode_key_event/encode_mouse 在固定 adapter 中只读
mode；CommittedText 只转移 String；render/selected_text/viewport_bounds 不更换 sync
processor。没有 reset/rebuild/recover 的旁路或 engine 后台任务。

固定 rsHell 的 `alacritty_feed::advance` 在解析新输入前检查 sync_expired 并 stop，
然后再消费输入；因此普通 output 到期会真正 flush，而不是只使 witness 失效。
没有 deadline 变更的 ordinary advance 仍保留原绝对时间。A 结束/B 开始会更改 epoch；
即使 timestamp 相等，A notice 也只 stale-settle，不能 end 当前 B。

一个 Cell 处于 Idle/Armed/Waiting/Queued/Applying。wake 先去武装；Waiting 仅在真实
槽释放时重试，并在释放临界区内先占普通槽，优先于新 ordinary admission。
Queued/Applying 保留同一个 witness，不重读当前 deadline 更新其身份。完成、stale、
显式 cancel 后重新 snapshot；revoke 后 Idle。worker 的生命周期服务 tick 不重建
Waiting/Queued 的到期 timer，不重复申请 notice。

## ScopeBudget 与生产差分

`budget.rs::manifest` 在 owner 构造前 checked 合计：fixed owner/slots/tickets/preedit/
UI ledger/cancel/revoke/SyncTimerCell、8192 retained grant、4096+12 paste workspace、
32-byte encoder scratch、单个 32-byte chunk window、32-byte read record、每方向128
stream、三份有界 frame（含 Vec→Arc 同时存活）、一次 projection pack 与 bounded copy。
Root 八 lifetime 的 delta 乘法也 checked。输出 work area 是 owner 的常驻保留额度，
不是执行后才测得的峰值；action-only 不占 Data window。每个 grant 先记额度再分配
String；binding 存已解析 enum，不 clone 绑定字符串。

输入域固定：profile 六条合法 binding，payload 最大4096、geometry 最大12×3、history=0，
远端只接受 `MODES` 的完整 fixture record，lifetime 最多32次。无任意 OSC/超长 title/
零宽图；render 最多36个 ASCII cell，copy 最多三行，颜色只作固定 enum 转换。
这些是测试域，不是生产配置 cap。4-byte binding 以2-byte chunk证明完整发送；大 text/
paste 独立使用超过32 bytes的多 scalar输入，paste仅一对 bracket边界。

底层 vte 0.15.0 的 `SyncState::default` 已预分配2MiB；该已有 engine 类在 manifest
单列，不伪装成32-byte queue成本。既有 engine 私有图、profile/font root、allocator与
runtime worker 类继续依赖 A 的条件边界；本 delta 不构成完整生产 root 图证明或 H 证书。
真实 captured profile、OS clipboard snapshot、native context/TSF buffer/layout table
及 scoped production transport 的资源证明由其原 owner 提供，未获前件不得启用生产。

Root 仍只在非 UI 实际 join 后返还 worker 名额。worker_done 不能代替 joined，也不能
代替 claimed action ACK 或 native retirement。永久登记槽不重用 context；第九次停用。
Core revoke 同时终结本 fixture 的 transport owner；pending write 由原 worker 取消并关闭
stream 后才退款，不等待 peer 恢复读取，也不重放未知交付的剩余 bytes。其他 owner 独立。
