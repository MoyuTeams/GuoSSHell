# W1-S 单操作接口、源式与启用前件

本目录仅交付独立运行单元，父在 `run_prepared_session`、`run_session_inner`、`establish`
注册消费；不修改 normal execute、公共 AuthPlan/SessionTransport、认证或信任契约。
完整 Snapshot/Source/CloneShape 仍由对应 sibling 提供，未就绪不能签 Start 证书。

## Inline 与真实根

`InlineSessionOwner<F>::try_install` 消费已构造的真实 F，以
`Layout::new::<Header>().extend(Layout::new::<F>()).pad_to_align()` 在 heap 前取得
进程 ledger Runtime grant，再 fallible global alloc 和原位 pin。capture 的图在构造 F
之前已有各自 operation/secret/source root，不从 F 的 inline layout 推出它们为零。

`InlineSessionOwner<F>::try_install_reserved(F, Grant<'static>, HolderBound)` 消费已准入的
同一个 move-only Grant；与 `try_install` 共用安装内核，不退额重申请、不 clone/split，
也不 tighten 额外已批准占用。按真实 F 的 checked Layout 验证 Rust bytes 至少覆盖
Header+F，class 为 Runtime 且 slots 至少为 1；C bytes 不替代 Rust bytes。错误 class
返回 `UnsupportedEnvelope`，字节/slot 不足返回 `RustBudgetExceeded`，NULL 返回
`AllocationFailure`。未发布 guard 在 layout/额度/alloc 失败时先析构 F 及捕获图，再退
原 Grant；成功后的原 Grant 在同一 Header 随最后 ref dealloc 后才释放。

父可将 `network_runtime::TaskAdmission<F>` 的 `future / permit / holders` 直接 move 到
此接口。此接口不重新查询 admission、epoch 或 source；caller 在构造 owned captures
前完成 data-before-owned，并负责取消与新 entry 的最后准入线性化。accepted Grant
即使 epoch/source 已失效仍支持转移和正常清理，但不据此恢复新 entry。out-of-line
root 和完整生产 holder graph 仍是 caller 前件，不从 fixture holder 数推断。

唯一 owner 访问 F。CancelHandle 跨线程只置 cancel/ready；poll 返回后由 owner 检查、
销毁 F。Ready 先 Complete，再销毁 F/output。RawWaker 仅操作 Header 原子计数和 ready，
不访问 F、Tokio、TLS、通知对象或分配队列。Intrusive `InlinePollList<F>` 的 link 内嵌
Header，每轮最多访问 caller 指定的不同节点，重复 wake 不绕过轮转。不施加 helper 的
4 槽运行 cap。父须实际持续驱动该 list，ready 位本身不承诺线程调度实时性。

所有 Header 引用均计数，最后 ref 取原 Layout/Grant 到栈，dealloc 整块，再 return。
Waker 可比 F/owner/list 存活更久，所以账本必须 static/process lifetime；模块仍須在
callback/poller 存活时 loaded。没有借用局部账本生成可逃逸 Waker 的入口。

实际 integration 产物诊断：Header96/8，包含真实 AuthPlan/SecretOperation 和 Source holder
的 RetainFuture312/8，checked inline backing408/8；实际固定 controls<u32,u32,2,1>136/8。
这些数来自最终编译类型，不用它们代替父 run_prepared_session 的实际 F。

`HolderBound::from_source_proof` 是 unsafe 证明输入，必须计 owner、Context、Cancel、
所有库内部 waker、clone 和跨线程回调。CAS 在 max 之前检查，绝不 wrap。RawWaker clone
不能返回错误；违反已证明界时直接 abort，不 unwind、不少增 ref、不泄漏或永久占额。
当前 fixture 源图的同时 holder 上界为 8；这不是完整网络库的 holder 证书，后者未闭合。

## 四类 gate

### Session 控制

实际父源：`session.rs` supervisor 的 loading/running JoinSet、finished unbounded channel；
Start 的两个 unbounded command/reply channel、watch、captured context/source/target
running.spawn；`run_session_inner` 的 frozen_target.clone、每 attempt target.clone、host
clone；首次 engine 的 selected.clone 和上游 `alacritty_adapter.rs` retained settings.clone。
`connect.rs` 的 saved_password、load/unlock/store credential 与 signer 链还使用
spawn_blocking/通道/闭包结果。终端无关输入不在本目录改造范围。

本目录 `InlineSessionControls<C,R,N,M>` 是实际内嵌 storage：两个固定 ring、close 位、
一次 completion 位；没有 channel Arc/Block/Watch backing，payload 必须带自己的 root。
`InlinePollList` 替代本次 P1 running task cell，helper loading 由父 C 单元接缝处理。
父现有函数仍接收 Tokio UnboundedReceiver，**不能把这个新类型声称已经兼容接线**。
最小应用私有差异是为 P1 run/handshake 增加借用控制视图/原位接收分支，V1 原签名保留；
不能强行把自有 ring 转成未收费 Tokio receiver。

`reserve_before_attempt`/`reserve_before_engine` 接受 generic borrowed T 和 unsafe
`FrozenCloneShape<T>` 的完整源证明，不定义伪 Snapshot/Source 或 BuildFingerprint。
grant 必须移进真实目标/transport/engine root，不能 clone 后补额或从 slot 取走即退。
捕获 selected 的 blocking task 最小接缝是 closure 和结果各捕获带额 app root，所有外部
cell/waker 保有的控制 backing另证明；JoinHandle 结束不是 Cell free。

### Native boxed 操作

锁定源：rshell-session `b72a83bf337c959b3e40c6ef5a255413fc09c0f9` 的
`transport/mod.rs:104–122`、`transport/native_ssh.rs:101–282` 及 handler/config/channel/auth；
async-trait `0.1.92`，Tokio `1.53.1`，russh `0.62.5`。
实际 SDK `rustc 1.98.1`、compiler `48a229ceaefd4985c50990b14116b6d856af0985`，
GNU x86_64 Windows、默认 Cargo dev graph，无生产 RUSTFLAGS 修改。

对实际依赖 crate 的私有 monomorph `-Zprint-type-sizes`，generated F 而非 Box 指针为：

| 方法（实际 native_ssh 行） | F size/align | 捕获与 suspend 根 |
| --- | --- | --- |
| connect 197 | 8608/16 | self/request 引用、broker；bounded(connect_inner)；内含 TCP/auth/channel futures |
| next_event 206 | 80/8 | self、channel.wait receiver；输出留在 root |
| write 218 | 3336/8 | self/bytes、bounded async、data_bytes send；输入复制在 channel_mut 后 |
| resize 232 | 3112/8 | self/size、bounded window_change send |
| shutdown 249 | 3128/8 | self、taken channel/handle、eof/close/disconnect sends |

source 给 captures、await 次序和 allocator 调用类；compiler 给实际 generator 的 variant
field/offset/tag/alignment；测试在**调用前**拿上述 Layout grant、调用实际 public 方法，
逐笔 recorder 核对 actual pointee alloc/free。这些 layout 只对上述 actual build 适用。
`NativeFutureLayouts::from_monomorph_proof` 要求 caller 的真实构建证明；没有自动签发或
自行填充 fingerprint。不同 SDK/cfg/features 必须重取诊断，不是调用后 size_of_val 授额。

`NativeCall<T>` 保留 boxed F、实际 output 和 grant；Ready 仅销毁 boxed F，result 仍由
root 持有，root drop 后返额。`UnconnectedNative` 实际接管刚 new 的 transport，不暴露
可变 transport：invalid-size connect 在 TCP 前退出、next_event 无 channel、write 空值
成功或 channel_mut 失败先于 bytes.to_vec、resize 在校验/无 channel 处退出、shutdown
无 handle/channel 成功。此有限源码图没有内部 spawn、queue 或逃逸 Weak，是真实可执行
方法分支，**不是全部 Start 或已连接方法的证书**。合法网络 request 不调用此 validation
入口，父必须提供其独立网络 gate，不把普通 Start 永久路由到拒绝分支。

网络阻断精确根：`NativeSshTransport::connect_inner:123–140` 新建 StrictClientHandler
并调用 `russh::client::connect_stream:995–1064`；后者在 `1048` spawn
`Session::run<StrictClientHandler,TcpStream>`，handler 持 host/verifier/broker。
真实 SDK 诊断这个 run F 为 13376/16；russh-util spawn wrapper 为 26768/16；Tokio
未装箱 Cell 为 26880/128（同时也有 spawn 大 future 装箱分支，不能只计此 Cell）。
成功、失败、取消都可能留 handler、channel/Msg/Reply、IO、任务/waker 外部引用；
公开 Handle::Future `954–965` 只 await join，disconnect 只 send；均无 final-dealloc hook。
write 的 Vec 会转入 russh queue，resize/shutdown 的 message/handle 在后台 session 中消费。
`native_ssh/channel.rs:36–49` 在 PTY/shell success 前可将 server events 不断 push 到
pending_events；`55–59` 把 received data.to_vec 交成 Output。该增长路径没有 public
分配前接缝，不能只用 method F 的 layout 覆盖，也不能以 timeout 当字节数界。

**未能在当前边界签发的具体方法**：正常 network connect、connected next_event/write/
resize/shutdown。缺完整 session task/message/crypto/IO 增长图、waker holder 总界及真实
last-root。最小上游差异若应用私有不可逃逸根无法证明：在内部 task/registration 的实际
最后 dealloc 提供 app operation root 回调/所有权接缝，而不是 join 回调；无需改变公共
AuthPlan/SessionTransport 参数。当前无该修改授权，不改 russh/fork/SDK。应用侧先消除
P1 自己的 task/queue 不解决库内根。

### Auth interaction

实际 public AuthPlan `auth.rs:84–121` move SecretString，clone host/identity。
Password 成功分支 `.ok_or(CredentialMissing{host:profile.host.clone()})` 是 eager：
即使有 password，第二份 host 仍构造再销毁，所以 bound 是两份 host 字节加可选 path。
GNU PathBuf clone 的 encoded bytes 计真实 out-of-line 长度；不会读 identity 文件。
`prepare_auth_plan` 先 reserve，再以稳定 LockedSecretString 的同 backing operation
转换建真实 AuthPlan/错误；整个 result 在 LockedSecretOperation 内析构，秘密不 clone。
独立审计观察唯一秘密 backing：zeroize → free → secret grant return。
真实 transport authenticate 后的 password/string/response copy 不由此工厂证明。

具体额外 auth 路径：`native_ssh/auth.rs:27–33` 将 password.expose_secret().to_owned()
交 authenticate_password，原 SecretString 到 await 后才 drop；`191–233` clone 完整
keyboard-interactive request、collect 答案普通 String 后交 russh；`237–246` scrub 仅
排一个 secret-free method，不证明所有 queue/task backing 已清零/free。PublicKey 路径
还有 load_secret_key、Arc<PrivateKey>；Signer 路径有签名 Vec 扩容和 boxed 外部 sign。
这要求额外 clone/共存/受保护 backing/last-holder 源式，不可把 LockedSecretOperation
的单 backing 证明推广到网络 auth。若库内实际 first-owned copy 无法保持现秘密契约，
最小差异需在该具体 copy/queue 所有权点建立受保护 operation 接缝，交父核对；本目录
不改公共 auth 类型、认证模式或上游 secret 实现。

`transport/mod.rs:211–219` 为 mpsc(capacity16) 和 Arc<Mutex<BTreeMap>>；
`request:161–188` 建 oneshot、pending.insert、PendingRequest Arc、send、await response；
PendingRequest drop remove，respond remove/send。具体布局源式：

- `oneshot_upper<T>`：std Arc 两原子 header + Tokio Inner 的 state、Option<T>、两个
  Task(Waker)。InteractionResponse actual Inner64/8、ArcInner80/8，源上界覆盖。
  实际 InteractionBroker::request 未装箱 F632/8；将它装进 inline owner 时仍由泛型
  actual Layout 计入，不把这份栈值另记成 heap allocation。
- mpsc Block 为四 machine-word Header + `[T;32]`，两字段顺序上界；bounded semaphore
  不预分配全部 blocks。Chan 是两个 cachepad128、Notify、bounded semaphore、两个
  counters、RxFields。`bounded_chan_upper` 按每字段最坏 128 padding求和，非复制私有
  struct；锁定 GNU source 无 loom/parking_lot/tracing。actual Chan384/128、Arc512/128。
- pending insert 的 BTree node 由稳定 `btree_nodes` 计算；request 前先取得 oneshot 和
  `n+2` 个 node 的保守 split 上界，request owned graph 另计，usize 全 checked。
  节点实际归 channel 内 PendingResponses；remove 最后 entry 仍保留空叶根。
  其节点 grants 由 channel 所有者承担，直到 map 及所有实际 Arc/回调 holder 的 backing
  析构之后才归还，不以 request future drop、reply done 或 pending.is_empty() 替代。

`create_interaction_root` 是 unsafe 接缝：caller 必须证明 live_blocks（含 close/grow
竞争）、全部 broker/receiver/request/回调不可逃逸，先销毁 requests，再全部 channel
holder；后续 insert 节点额度保留到 channel 内 PendingResponses 真实析构之后。
单 request fixture 用 actual Chan384/128 和 1 Block：一次 send+最后 close 共 2 个位置，无并发 grow；
不把 mpsc capacity16 当一般所有并发路径的 block 数证书。真实 fixture 执行 pending
insert、recv、respond(Cancel)、request 析构、两个 channel holder 析构及 late wake。
真实 auth handler 逃入网络 task 时，这个有限证明不再适用；生产必须补其完整根。

### Poller/context/reactor

`BorrowedReactor` 只借用实际 Handle.enter，不创建 driver，不可跨线程移动；要求原
runtime 已由 persistent 根计额并驱动。caller 给本线程实际 TLS/enter/通知总源式，拒绝
零额，unsafe 前件要求直到 IO/waker 完成及 TLS dtor 后保持根。

具体非零成本和晚根来自 Tokio `runtime/context.rs:36–128`、
`context/current.rs:22–95`（HandleCell、enter clone/stack guard）、精确 std GNU
`sys/thread_local/os.rs::Value<T>`/AlignedSystemBox（System allocation）。已有 reactor
不等于新 poller 线程 TLS 为零；新 Runtime 的 scheduler/time/IO driver/park/waker/线程
全部另外证明，不能复用 helper 的 184/1672。

IO 每 source 在 `registration_set.rs:56–69` 新 Arc<ScheduledIo>；deregister 在
`74–81` clone 入 pending_release Vec；driver `104–113` 才移出/清理，shutdown 还会
构建 Vec。`scheduled_io.rs` reader/writer/Waiter wakers 可在 F/drop socket 后存活。
因此只持 operation 到 socket Drop 也不足：需要 actual driver释放/所有外部 IO ref
证明。最小应用私有差异是 persistent runtime owner 显式覆盖其增长和 shutdown 析构；
如果要逐 operation 精确归还而现公开接口无接缝，则需要 registration/task 最后释放
接缝，不能以一次 enter 结束或一轮 drive 替代。完整 GNU TLS/new-driver/last-root 证书
尚未交付，Windows 单机 fixture 也不宣称 mac/Linux 通过。

## 独立验收与限制

目标 `selected_p1_session_owner` 复用稳定 W1-L 及其 test-binary-only 固定 allocator
recorder，生产没有 global allocator。审计 secret/source/inline/auth request/native boxed
分支的每笔 alloc/realloc/dealloc，当时真实 held grant 覆盖所有观测 live Layout 之和；
另核对原 Layout size/alignment、fallible NULL、不同 opcode 零余量的 beforeCall 次数0。
没有仅以 peak 设置预算。fixture expected schema 来自上述源字段/调用类，而非 doc locks。

实际 future 保留 LockedSecretOperation 与带额 SharedOwner<String> source 关联根；
这个 source 是合成关联根，不是 TTF SourceF 或 domain Snapshot 证书。tests 实际执行
future/外部 waker 各释放顺序、intrusive公平轮转、poll 中 Cancel、真实 TCP read 和 duplex
write 阻塞取消、late reactor waker、真实 Native localhost SSH identification 阻塞取消。
localhost fixture 仅真实 TCP/SSH identification，到达 kex 前即取消，不读配置/credentials/
trust file、不改变认证；不是完整受控 SSH 成功连接/reconnect 或已连接方法证明。

runtime/socket/Native identification fixture 根在 test scope 内实际销毁；其 bootstrap、
System.alloc TLS、网络 crypto/task/IO 分配尚无完整审计分类/证书，明确不算零、不从
其测试成功推导 Ppersistent/完整 Start 绿色。逐操作 library 内所有 OS/global OOM typed
恢复不是本计划承诺。完整 Start 启用仍须上述具体 gate 以及 sibling 真实 Source/Snapshot
证明闭合；本目录可编译和正常单操作成功不替代这些必要事实。
