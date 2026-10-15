# App scoped 网络 runtime：接口、资源源式与启用前件

## 消费边界

本目录交付应用自有非阻塞 TCP、poller、单调 timer、有界协作预算、S intrusive
executor 和已收费原生线程单元。生产模块注册、Cargo feature/pin、两库适配及 P1
Start 接线归父所有。独立 `selected_p1_native_network` 通过 `#[path]` 消费本目录、
稳定 model/resource 及完整 `session_owner/mod.rs`，保留 S（含 reserved）的原回归
层级。网络新增测试不连接 legacy Native，不构造 SSH、认证或用户配置；S 回归仍
只证明其原覆盖面。allocator 仅由同一 foundation 模块注册一次。

`NetworkRuntime<R, IO, TIMERS>` 接管一个真实、已收费的 `R`，只 move，不 clone
其 Source/Secret/其他子图。rsHell/russh 接入时，R 可以直接是 russh 中唯一的
`ResourceRoot<AppReservation>`。本目录没有仿写中性 scope traits 或跨库 Root；
`AppScope` 仅保存原静态 ProcessLedger 和固定 Admission，并调用其 detailed 准入。

消费接口：

- `try_new(scope, R)`、`handle()`：收费的 binding 与固定注册表。
- `handle.connect(SocketAddr, Cooperative)`：非阻塞连接 future；结果为持根
  `NativeStream`，其 `peer()` 从真实 getsockname/getpeername 返回 SocketPeer。
- `NativeStream` 实现 Tokio IO traits；只使用协议 traits/ReadBuf，没有 Tokio
  runtime、net registration、spawn、time、sync channel 或 split backing。
- `handle.sleep_until(std::time::Instant)`：独立 Sleep；支持 reset 和取消。
- `collect_ready(wait)`：内嵌就绪批次，可保留真实旧 IoEvent；`drive(wait)` 交付事件。
- `Cooperative::bind(F)`：未装箱的 `Budgeted<F>`，每次实际 F poll 重置预算。
  stream 必须使用同一预算；可通过 `stream.cooperative()` 取得 fallible holder。
  协议内部 read/drain/decrypt 等循环也须在实际工作前消费预算，不只包最外层 await。
- `BoundedExecutor<F>`：接管实际 InlineSessionOwner，复用原 InlinePollList；每轮
  访问不超过固定 visits 个不同任务，重复 wake 只影响 ready 位。
- `TaskAdmission<F>`：分配前计算实际 InlineLayout，detailed 预约 header+F，保持
  move-only Grant 与真实未装箱 F；不声称已经安装或提供 join result。
- Windows `ThreadJob<F,R>`：明确 unsafe 源证明前件的原生线程单元，不是通用 DNS/
  signer worker 服务。取消撤销结果交付，不强杀闭包；结果仍在真实 owner 中销毁。

## 分配、容量与准入式

记 `O<T>` 为既有 `OwnerLayout::actual::<T>(0).backing`：

```text
extend(Layout<RcMemberLayout<Grant>>, Layout<ResourceBody<T>>)
  → extend(零长度 u8 tail) → 原始 padded Layout
```

每个 O 的构造均为 checked Layout → `try_reserve_detailed` → fallible global alloc
→ 原位初始化。无网络 Vec/String/Map/heap queue，无扩容或 realloc。初始化 OS
失败用当前调用栈的固定 fault 槽传回准确 Os 类别；NULL 与额度不足不混淆。
该 fault 槽不跨 attempt、不用 snapshot 推因果、不格式化错误。

| backing | 实际类型／容量式 | 准入与最后所有者 |
| --- | --- | --- |
| binding | `O<Binding<R>>`；R inline，不重复计其外置根 | Runtime/0 slots；Core、每个 socket/timer record 持有；最后 R/Startup 析构、header free 后退额 |
| registry | `O<Core<R,IO,TIMERS>>`；Mutex 内有 `[Option<IoOwner>;IO]` 和 `[Option<TimerOwner>;TIMERS]`、closed 和 checked generation | Runtime/0 slots；runtime/handle/stream/cancel/Sleep 持有；所有注册撤销后仍等最后 holder |
| IO | `O<SocketRecord<R>>`；Mutex 内含唯一 raw socket、generation、2 个 Option<Waker>、peer/fault/连接状态及 binding holder | Runtime/1 slot；注册表、stream/cancel、OS snapshot、晚 IoEvent 持有；socket/wakers 先析构，最后 header free 后退額 |
| timer | `O<TimerRecord<R>>`；deadline/generation/状态、1 个 Option<Waker> 和 binding holder | Runtime/1 slot；注册表、Sleep/cancel、driver snapshot；取消/到期均不直接退額 |
| coop | `O<Budget>`；固定 limit/remaining/yielded 原子，无 waker 保存 | Runtime/0 slots；Budgeted F 和对应 stream/协议 holder |
| task | `InlineLayout::actual::<F>()`，真实 S Header extend 真实 F | Runtime/1 slot；须由 reserved installer 消费 TaskAdmission 的同一 Grant；S 最后 raw waker/cancel/header dealloc 后退額 |
| blocking | `O<JobRecord<F,R>>`；两个状态原子、Option<F>/Option<R>、显式 TLS index，全是实际泛型 inline 字段 | Runtime/1 slot；thread callback 与唯一 OS join owner；闭包/结果/TLS 清理、线程退出、header free 后退額 |

IO/TIMERS 是真实已付 backing 的容量，不套用 Source 数或 helper 四槽。表满在
新 record allocation/socket 创建前返回 RegistryFull；原表和原 grant 不变。没有
隐式 grow；需要更多容量必须先创建与其实际容量匹配的收费实例。任何后续 resize
实现须在旧表仍存活时预约完整新表，并保留 old+new 共存额度，本接口不提供未收费
增长。BYTE_LIMIT 仍是原 `403_701_760`，所有 backing 使用同一原账本。

## 实际处理预算与 waker 上界

每轮 IO/timer 各最多检查 32 个注册槽，cursor 独立轮转；每个就绪注册至多交付
两个 waker。WSAPoll 每轮至多 32 个实际 fd，不连续 drain；wait 至多 10ms，并
受本批次最近 timer deadline 缩短。所有 fd 在 OS 调用期间由 snapshot strong
holder 保持；并发取消只 shutdown，不在该调用中 closesocket。

read/write 一次 syscall 至多 16KiB；ReadBuf 初始化也只处理这 16KiB，不初始化
任意大 remainder。连续 Ready 的任务通过同一 Cooperative 的实际工作计数让出，
耗尽后每次 poll 只安排一次无分配 wake。不可分且超过预算的协议工作不得以反复
Pending 代替分块或已收费 blocking 作业。

每个 IO record 只有 reader/writer 两个 waker 保存点；每个 timer 只有一个。
替换时至多额外一份 clone 暂存；取消/交付只 move 既有 waker 到栈，不建 wake
queue 或再次 clone。ReadyBatch/IoEvent 仅 clone actual SharedOwner，不 clone
waker。每个 SharedOwner clone 可失败；S RawWaker clone 的总上界仍必须由父的
完整生产图证明，包括 Context/cancel、库/broker/split、全部同时挂起 IO/timer
和并发替换临时持有者。本目录的有限闭包测试按实际保存点分别计算 HolderBound，
没有把固定 fixture 常数或峰值移给 normal SSH。

注册数组和批次不是同一分配：注册数组内嵌在 Core backing；本轮至多 32 个
IO/timer owners、generation/PollEntry、32 个 WSAPOLLFD 和 ReadyBatch 都是有界
栈值。父若把批次或其他返回值放入 task/heap，必须计算其实际外层 F/容器 Layout，
不能因为本实现使用栈就把上层 owned allocation 计为零。

## 取消、迟到与最后释放

stream/cancel 注销只撤销旧 registration 的交付资格。IoEvent 同时持旧 root
及原 generation，不能通过复用 index 获取新 socket；注销后 deliver 返回 false，
丢弃真实旧 event 才能释放其 holder。Sleep reset 使用新的 checked generation，
旧 snapshot 不触发新 deadline；取消后不能 reset/rebind 为新 attempt。

closed/cancel/fault/结果状态及全部 waiter 槽已计入 header/backing，数据流量不
能占满它们；cancel/shutdown/remove 不做分配或重新准入。source/epoch 失效阻止
新分配，不改已有 root 的绑定或释放旧额度。

实际顺序使用既有 SharedOwner 和 S 内核：先取 Grant/Layout 到栈，payload/全部
外置根析构，dealloc 原 header/backing，再 refund。Socket Drop、取消、timer 到期、
任务 ready、一次驱动、Weak 数量或 joinReady 都不是 refund 事件。原生线程的 join
owner 等 `WaitForSingleObject(INFINITE)` 返回真正 thread exit，再 CloseHandle；
结果/显式 TLS slot/owner 留到 ThreadJob 析构，仍先 dealloc 后 refund。结果 ready
不准借用结果，取消后的晚结果也不交付。无强杀或 timeout 退款。

## OS 与 TLS 所有权

Windows binding 在额度取得后调用 WSAStartup，最后 holder 才 WSACleanup；每个
socket 在额度取得后 socket/ioctlsocket/connect，最后 SocketRecord 才 closesocket。
peer、sockaddr、WSADATA、SO_ERROR 参数均为固定栈空间。socket 缓冲、Winsock
provider、OS thread/stack/TEB、TLS index 属系统资源，不伪装成 Rust bytes 或 SQLite
C quota；每个线程/socket 创建失败保留准确 OS code，没有新系统权限。

网络 poller 在父的现有驱动线程上运行，本模块不创建网络线程、不访问 Rust
thread_local、std thread context 或 Tokio TLS。父常驻线程的已有 TLS/bootstrap
根不由本目录重新计零或退款。ThreadJob 使用 CreateThread 和自己取得的 Win32
TLS index，callback 设置借用真实 job root 的 TLS value，返回前清空；join owner
活过 callback、线程真实退出及 TlsFree。callback 的 Rust 调用图仅包括已证明 F、
原子与 SharedOwner 操作；任意 std/Rust TLS、隐藏分配、panic/unwind、外置结果
必须先有额外源证明和 owner，否则不能调用 unsafe try_start。

## 目标证据与必要未签前件

实际目标：`rustc 1.98.1` / `48a229ceaefd4985c50990b14116b6d856af0985`，
`x86_64-pc-windows-gnu`，Cargo 默认 dev/test graph，panic unwind，测试独立 allocator
recorder；生产不增加 global allocator。windows-sys 锁定 0.61.2，现依赖统一启用
Foundation、Networking_WinSock、System_Threading；若依赖图收窄，父须在现 pin 的
feature 层显式保留它们，不能升级或替换为框架。

实际 final Layout（size/align，仅对应上述构建和类型）：R=()、IO=TIMERS=16 时
binding 80/8、registry 632/8、socket 264/8、timer 152/8、coop 96/8。受控 loopback
actual F 为 152/8、S Header offset 96，inline backing 248/8。捕获 23-byte 数组、
返回 17-byte 数组的实际 blocking closure/result owner 是 128/8；不是 dyn 指针或
占位 future。不同 R、容量、F、compiler/target 必须重新计算，不能复用这些数字。

仍须由父闭合的接口／证明：

1. S 的稳定 `try_install_reserved(F, Grant<'static>, HolderBound)` 已检查实际 Layout
   并接管同一已取得 Grant；新 generic spawner 接线须消费 TaskAdmission 中的同一
   Grant，不能退额再预约。本目录尚未接入该 spawner，不将 S/reserved 的回归或
   已安装有限演示称为完整 scoped spawn 链。
2. normal Session F、捕获的真实 Prepared/FrozenClone/SourceHolder、全部库 waker
   图及 rooted completion/输出；缺图用 `Unsupported(ProductionHolderGraph)`。
   S 当前消费 Output 后销毁，不能用它代替完整 russh Task join/result root。
3. 已验证两库新 API 的精确 pin、唯一 ResourceRoot/AppReservation 接线及全部
   protocol/auth/secret/queue/crypto 分配前接缝；本目录不读取独立 fork 临时源码。
4. DNS 服务的固定 job/result 槽、解析请求/迭代器/结果/错误图，以及 signer/
   credential 的闭包、外置结果和真实 TLS 闭包。没有这些源证明分别使用
   `Unsupported(DnsClosureAndResult)`、`Unsupported(ExternalSignerClosureAndResult)`，
   不偷用 std resolver.collect/spawn_blocking。ThreadJob 单元不自动签这些调用。
5. macOS/Linux 的原生 poll ABI/平台 source closure 与目标化运行。当前非 Windows
   Startup 明确 `Unsupported(UnixPollerSourceClosure)`，Socket 为不可构造类型，
   不默认为 raw=0、假成功或回退 Tokio。std Instant、固定注册和协作部分已使用
   跨平台公开能力；没有 macOS/Linux 设备/CI，不声称实跑。其他目标整体证书由父
   target gate 控制，GNU 数字不签 MSVC/macOS/Linux。

以上是当前启用前件，不将完整正常网络交付降为可选工作。当前 Windows App runtime
可以正常运行真实小 TCP 连接；这些前件不把已证路径永久改为拒绝。

## 独立验证与清理

在项目根，以 PowerShell 执行，每个 Rust 命令保持 `RUSTUP_AUTO_INSTALL=0` 和
`GUOSH_GUI_FIXTURE` unset；Cargo 一律 locked/offline。owned rustfmt 使用
`--edition 2024 --config skip_children=true`，只传本目录 11 个 rs 文件和独立 test
文件，不 workspacefmt 或遍历其他写面。

```powershell
$env:RUSTUP_AUTO_INSTALL = '0'
Remove-Item Env:GUOSH_GUI_FIXTURE -ErrorAction SilentlyContinue
cargo test --locked --offline -p hub --test selected_p1_native_network
cargo test --locked --offline -p hub --test selected_p1_native_network network_tests:: -- --nocapture
cargo clippy --locked --offline -p hub --test selected_p1_native_network -- -D warnings
```

10 个新增 runtime 测试覆盖：真实 SocketPeer/TCP 读写和 IO traits，无 ambient Tokio
client runtime；真正 read/write WouldBlock 后取消；注销后保留的旧 OS event 和晚
waker 各自持额；timer 独立到期/重置/取消及 40 个注册跨批次唤醒；重复 wake/
连续 ready 下不同任务轮转；byte/class-slots/固定表满的 typed 拒绝、NULL 分配；
source ABA/epoch 失效不释放旧根；实际 closure/result/显式 TLS/OS thread exit
后最后 dealloc/refund。逐笔 recorder 只位于独立 test binary，free 观察点仍持原
grant；数学使用上述实际 Layout/容量式，不用峰值设置额度。

受控 std listener/peer 是 run-owned loopback fixture，端口由 OS 临时分配，没有
server 进程、GUI、配置、私钥、密码、信任文件或外部 signer。所有 listener、peer、
客户端 raw socket、registration、timer、waker、TLS slot、thread handle 都有精确
owner，测试结束后 ledger 为零；取消或测试 unwind 也由同一 RAII 清理。fixture
环境的 Tokio context 查询在审计之外，仅确认没有当前 runtime；不是生产 TLS
成本证书。既有 foundation 测试仍只证明其原覆盖面。
