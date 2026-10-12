# 独立 Local process owner 的当前边界

## 运行面与安全

`desktop_local_shell` 独立 integration 通过 `#[path]` 消费本目录。正常 hub 模块、
dispatcher、registry、Prepare/Start、Dart、字体和网络均不接线，不改变公共 API。
Windows `LocalProcessOwner::launch_probe` 直接消费 `pty_owner::PtyOwner`，提供
独立非 UI 消费者的 write/resize/next_event/shutdown；`PtyOwner::launch` 提供
独立 reader/writer/控制根的所有权接缝。两者的构造器均仅测试编译可用，生产完整
root 工厂尚未提供。

已有 `pwsh.exe` 使用 `-NoLogo -NoProfile -NonInteractive -Command`。固定 fixture
仅做合成 Unicode echo、控制台尺寸、输出洪泛/等待和退出，不执行 UI 命令、不读写
用户文件或 profile/rc。普通产品的 shell 选择和正常启动配置策略不由这些测试改变。
测试无 SSH profile、信任、认证 responder 或 RemoteHost 身份。

`validate_local_fields` 须在 Local 分支内、任何 saved SSH 查找之前消费。没有
saved-Local 契约时任何非空 `connection_id` 拒绝；host/port/username/command/
password 不得作为本地启动材料。完整阶段配置、字体及输入消费者由父随后接线。

## 固定来源与可消费接口

依赖均为 rsHell `b72a83bf337c959b3e40c6ef5a255413fc09c0f9`、default features：
`portable-pty-psmux` 0.9.6（lib `portable_pty`）、`rshell-platform` 0.1.0；事件与固定
launch 材料使用同 source 的 session/core 公开类型。hub 的两个桌面直依赖由父装配。
源码只经 offline locked metadata 的精确 manifest 路径读取，不复制源码或使用缓存
path 依赖。本模块没有改变上游、manifest、lock 或安装任何工具。

- `reserve_probe_root`/`app_root_shape` 消费父稳定 `selected_p1/resource` 的
  `OwnerLayout::actual`、Ledger、Grant 和 `SharedOwner`，不重建预算实现。
- `PtyOwner` 持有 Job、child、master、最后 master clone 与两条 scoped worker 的
  join 权；正常路径的 slave 在 spawn 返回后释放。basic 的 Manual 模式由独立
  非 UI owner 自持 writer、创建 reader；自然收口另有 scoped completion worker。
  测试停点可保留部分初始化，没有 detached worker。
- `WriterInput::Reserved` 借原 root 的 8 MiB 内嵌 tail；`TwoStage` 借有限固定测试
  数据。每次最多准入 8 KiB 的一个同步系统写，短写按原字节偏移继续，无重编码、
  重排或 replay。`Complete` 与 `Closing`/`Failed` 分型；取消未准入数据不是正常写
  成功。`Closing.committed` 是成功返回的系统调用所接受的前缀，错误中的在途调用
  可能另有部分字节，不能据此重放。
- `Manual` 接受调用者原字节并复用相同系统写门，不预置 echo/尺寸输入、不缩小
  8 MiB 输入上限。其同步写在独立非 UI fixture 消费者线程上调用，不作为在途取消
  证书；A1 的在途写使用独立 writer worker 与原线程 handle。
- `request_cancel` 返回后禁止下一次写准入；`shutdown` 分别等待 writer join、
  Job 终止、真实 child wait/reap、最后 HPCON holder Drop 与 reader join。重复成功
  shutdown 只返回已完成结果，不重发析构观测。它们是独立非 UI owner 的同步接缝，
  不能直接从 UI/actor 回调调用阻塞清理。

## 自然完成源与尾输出

`PtyOwner::poll_natural_exit` 对自身仍持有的原 child handle 调用
`WaitForSingleObject(handle, 0)`。`WAIT_TIMEOUT` 保持 child/writer/master 与读端不变；
不读取 final code、不发布 Exit、不从 PID 重开对象，也不调用上游 `try_wait`。
只有 `WAIT_OBJECT_0` 后才调用 `GetExitCodeProcess`，一次读取并缓存 final code。
等待或读取失败返回错误，不用默认码或取消结果生成自然 Exit。

`LocalProcessOwner::next_event` 将已 signaled 的 owner 移交独立 scoped completion
worker。writer 的实际 Drop → child 原对象 wait/reap → master/最后 HPCON Drop →
reader join → 所有已创建 worker 返回 → completion join 分别观测。最后 HPCON
的同步关闭不在事件消费者上执行；reader 在此阶段继续向固定单槽交付尾输出，
即使槽满也不丢弃。消费者持续取槽，最后 join 后再次取槽，再发布一次 Exit。
自然 final 23 不按成功码 0 处理；995 只属于被取消的系统写结果，不能替代自然退出。

显式 shutdown 或放弃消费时改为 drain-only，避免满 UI 槽卡住最后 HPCON 与 join。
未完成 owner 的 Drop 同样切到 drain-only；这里是独立非 UI 清理接缝，并非可直接
调用的生产 UI 适配器。受控完成源覆盖退出码可见但原对象未 signaled 时的保持，
以及 signaled 后仅读取一次 final code；真实 fixed fixture 覆盖尾输出先于 Exit、
原对象 final 23、各实际关闭边界和再次请求事件时不重复 Exit。

Windows LibraryContract 来源为 `portable-pty-psmux/src/win/{conpty,psuedocon,
procthreadattr}.rs`、`filedescriptor` 0.8.3 的 Windows `Write` 和
`rshell-platform/src/process_tree/windows.rs`：

1. Native master/slave/clone 共享私有 `Arc<Mutex<Inner>>` 控制根；reader 是输出
   pipe handle 的副本，writer 是取出的 input pipe，并不另持这个 Arc。
2. Windows native writer 的一次 `write` 对应同步 `WriteFile`；`flush` 无系统 I/O。
   reader/writer Drop 释放各自 box/pipe handle。最后控制 holder 的 Drop 调用同步
   `ClosePseudoConsole`，不是只发关闭消息。
3. `spawn_command_in_job` 的 fallback 能替换 ConPTY/pipes。业务 reader/writer 必须
   在该调用返回后获取，不能继续使用 spawn 前的 endpoint。
4. Job 为创建时包含与 kill-on-close。只调用自己 Job 的 terminate；不依赖
   `ChildKiller::kill` 的成功返回来推断 child 已回收。child 不作为 Future 被 poll，
   不创建该接口的 detached waiter/waker。

## Windows 实际取消交接与观测

`IoState` 原子门区分 OPEN、IN_CALL、CLOSING、CLOSED。准入 CAS 与关闭共用该状态；
取消关闭 OPEN 或标记唯一在途调用，writer 在实际系统调用返回后越过关闭门并发布
CLOSED/CancelConfirmed。确认后该 worker 无新 write/flush 路径，确认不替代 Drop。
确认记录先于 CLOSED 的 Release store；controller 对 CLOSED 的 Acquire 读取同时
取得确认字段，不能读到已关闭但确认序列为零。Manual 交接使用同一发布函数。
重复 cancel 不重新发布 dispatch/确认序列；确认描述系统写已返回，不描述 writer Drop。

writer 线程通过 `DuplicateHandle(GetCurrentThread())` 复制自身真实 handle，记录
自身 ID，controller 在调用前核对 `GetThreadId`。仅借该原 handle 调用
`CancelSynchronousIo`，不按裸 ID 打开别人的线程、不使用 ThreadKill。ABI 是已锁
`windows-sys` 0.61.2/Windows SDK 的 HANDLE、BOOL；本地私有 kernel32 声明不新增
父持 feature。现有 C++ 输入 bridge 只有 HWND/STA 身份，不能借作 writer 的权限根。

标记成功、`ERROR_NOT_FOUND`、系统 write 返回和 worker 关闭确认分别记录；取消
调用本身不等待系统调用完成。若请求发生在系统调用实际进入前，有限控制循环继续
观察原线程直到 writer CLOSED，不能将 `ERROR_NOT_FOUND` 称为取消成功。满 UI
单槽时 reader 保留正常在途输出；取消关闭后停止 UI 交付但继续实际 read/drain，
自然完成保留交付直到 EOF。取消不等待读端 mutex。writer join 在 Job terminate/PTY Drop 之前，不能以 killChild 或
closePTY 推导写已返回。

独立探针覆盖 owned 前拒绝、无 child 的部分初始化收口、首次写前取消、8 MiB 部分
系统写后真实阻塞取消、两次系统写间取消、确认后的准入竞争、重复关闭及自然完成。
reader/writer/最后 master 的实际 Drop、child wait、每个 join 与 scope 返回均在相应
完成边界核对，包含 worker 自有 root capture 和返回后取消的 completion holder。
std packet/closure 的逐项 allocator 源与布局仍须纳入下面的完整源审计，不能将 join
记录替代这部分 H/C。最后 app backing 真正 dealloc 后才返额。
同一 fixture 进程内第二套独立 PTY 在第一套系统写等待时完成 TERM、93×31→107×37、
Unicode、末尾输出和自然 exit 23。这是同进程独立 owner 证据，不是 Fluent GUI 或
共享生产 executor 验收。

## 操作错误与未完成清理

`Transport` 保留实际操作失败；`CleanupIncomplete` 表示仍有受持资源，不能按成功
或最终退款处理。可恢复的控制失败记录原操作错误，继续等待真实 CLOSED、原 child
wait/reap、各 Drop/join 与 Job Drop。已完整清理的失败不由 Drop 再抛 panic；shutdown
缓存原失败结果，重复请求不会把失败变成功或重写已完成序列。

PtyOwner 的 Root 字段位于资源字段之后，显式回收期间保持同一根。资源结束观测
分别检查 ledger 仍持额，最后 app backing 实际 dealloc 后才退款。故障探针覆盖
thread-target 创建拒绝、一次取消控制错误、一次 child wait 操作错误与单次 unwind；
这些用例没有 caller 的 Root clone。独立自然 completion 同样没有外部 Root observer，
实际 completion join 后 owner 自持额度，直到 owner Drop 才释放。

非 UI Drop 对未完成清理保持根与全部 join 权直到真实收口，不 detach/forget 或在
unwind 时二次 panic。shutdown 的原取消/child 等待期限不提高；超时返回保留态。
持久平台错误下的 Drop 可能保持 pending，这不是限时成功或生产关闭证书。完整
预留清理执行面及其有限收敛仍属于 G3 前件，独立监督强回收不能使该路径通过。

测试输出在原 32 KiB cap 内积 raw bytes，匹配完整字节序列后才解码；不逐事件损坏
UTF-8、不让产品重编码。单 fixture 的真实 PTY 每次读取限制为一字节，覆盖跨事件
2/3/4 字节字符，保持 TERM、尺寸、原 child 身份和自然 final 23。

外层 supervisor 的期限保持 12 秒，fixture 终止和 shell handle 观测各为 2 秒。
只使用本轮 spawn 原 handle，核对 PID/创建身份后回收本 fixture；shell 另持真实
query/synchronize handle 观测退出。强制回收必须使取消用例失败，不能 detach、
提前退款或丢弃回包来通过。批准临时根内每轮独占 `guo-local-<run>.report` 有 32 KiB
上限；释放文件句柄、核对 canonical 父目录后精确删除并确认不存在，无目录递归删除。
fixture 的原 shell 观测句柄在应用实际清理/wait 完成后保持到 supervisor 取得同一
创建身份；确认使用仅本 fixture 的单字节 stdin 管道，不是 pwsh 的 PTY 输入，也不
参与应用完成判定或推迟 writer/master 的关闭。未确认仍受外层期限约束，不按 PID
消失判成功、不重试重开的进程对象。

## H/C 源式与物理 Root 责任

已闭合的应用探针 backing：

```
H_app(n) = OwnerLayout::actual::<AppRoot>(n).backing.size(), 0 <= n <= 8 MiB
C_app(n) = 0
AppRoot = SourceToken + DispatchGate + AtomicUsize 序列
        + 26 个 AtomicUsize 观测槽 + Mutex<OutputSlot> + Windows IoState
OutputSlot = [u8; 8192] + usize
```

IoState 的控制/统计与 `OnceLock<ThreadTarget>` 内嵌，input 为同一 backing 的 tail；
无另一份输入 Vec、动态 UI 队列或 cleanup 通知。Runtime grant 先于 app backing
allocation；额度不足或 allocation 失败不产生 child。原 epoch 失效后使用已有
root/句柄/worker 清理，不请求新 Source grant。Root body Drop（含真实线程 handle）
→ 原 backing dealloc → grant return；allocator 在 dealloc 边界核对仍持额。

当前 Rust 1.98.1、`x86_64-pc-windows-gnu` 独立 integration 测试构建实测
`H_app(0)=8640`、`H_app(17)=8664`、`H_app(8 MiB)=8397248`，alignment 8；
AppRoot body 8560、IoState 104、PtyOwner inline 208 bytes。测试构建包括内嵌一次性
故障位与读 chunk 限制，实际构造始终按对应 cfg 的 typed layout 计算。
SDK `FILETIME` 为 8/align 4，
`OwnedHandle` 为 8/8，`TerminalSize` 为 16/4。
数值只用于本 target 证据，运行用 actual typed layout，不是跨 target 常量。

**H_app 不是完整 process-root，Windows 正常取消通过不放行 G3。**

```
H_process = H_app + H_launch/config/TERM + H_command/environment/retry_peak
          + H_pty/child_boxes/private_shared_inner
          + H_thread_packets/closures/names/completion
          + H_event_delivery/poller/consumer + H_cleanup_peak
C_process = 实际 C allocator 源核实的应用 backing（完整闭包未提供）
OS_external = 线程栈、内核 pipes/ConPTY、Job/进程/线程句柄、child/后代资源
```

环境 BTree/OsString、大小写转换、UTF-16 参数/env/cwd、命令克隆与 fallback 同时存活
峰值、私有 Arc/Mutex/Inner、spawn 属性 Vec 和 Box<[HANDLE;1]>、std scoped packet/
closure/线程名及错误清理均须 actual-typed、先 grant 后 owned/spawn。completion
closure 捕获完整 PtyOwner，不能把其 inline 大小当作免计的线程栈；事件 Output Vec、
异步 poller 的状态/定时器及测试消费者 runtime 也不包含在 H_app。8 KiB 读 frame
在实际线程栈；OS 栈/内核/child RSS 不能冒作 Rust/C backing，
也不能据 Root 的 C_app=0 推定整个图无 C backing。不借网络/helper 四槽证书。

完整源预算、各真实部分创建失败点与许可失败路径尚未认证。特别是 child spawn 后
reader endpoint 复制或读线程创建失败时，原同步 HPCON Drop 无独立可取消关闭接口；
当前不能证明已有洪泛输出下的失败析构期限。自然 completion worker 创建失败与
各读取/等待错误的完整资源预留及实际故障收口也尚未认证。须由应用侧预留稳定 drain worker/
endpoint 交接并实际证明；无法做到时再升级所需最小关闭/控制接缝，不能假定当前
测试已经覆盖，也不据此宣布全部 Local 不可行。完整生产构造器在这些前件闭合前
不提供，不是正常 Local 永久拒绝策略。

Linux/macOS 的代码/SDK、同步写及 Unix writer Drop、后代进程树、Windows fallback、
MSVC、GUI/沙箱与完整配置/字体/Source/Actor/registry 尚未实际验收。本 Windows GNU
探针不作为 A1 全平台、G3、P1 或 Stage6 通过证据。
