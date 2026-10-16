# SDK bootstrap 的隔离边界

这里只实现独立 fixture 的 Rinf bootstrap 和同 module 证明，不创建 engine、owner、
输入订阅、WindowSource 或 ticket，不执行认证、网络、真实配置或钥匙串访问。

## 父级最小 Rust hook

共享 `lib.rs` 用原有 `native-input-spike` feature 和 Windows 64 位条件注册
`mod native_input_spike_sdk;`。在 main 的 Tokio runtime 创建成功后、任何
`shutdown::run_until`、`app::run` 或 lifecycle 启动之前，使用同一谓词：

```rust
if native_input_spike_sdk::fixture_only_mode() {
    runtime.block_on(native_input_spike_sdk::run_fixture_only_until_shutdown());
    runtime.shutdown_timeout(shutdown::RUNTIME_LIMIT);
    return;
}
```

该代码必须使用与 module 注册相同的 cfg 条件。默认构建和只打开 feature 的构建仍走
原有分支；仅受控构建 shell 中的 `GUOSSH_NATIVE_INPUT_SPIKE_FIXTURE=1` 才选 fixture。
这是不同的启动模式，不是普通 GUI 兼容性证明。父级未接上述 hook 时不能启动本 fixture。

## 私有协议

- mode query：`extern "C" fn guossh_native_input_sdk_fixture_mode() -> u32`，不分配，
  默认 0，双门且 Windows 64 位为 1。
- SDK endpoint：`unsafe extern "C" fn guossh_native_input_sdk_probe(*const u8, usize,
  *const u8, usize)`，使用 Rinf 公共四参 ABI，不修改 V1/codegen。
- request 精确 32 bytes、小端：`GNSQ`、u16 版本 1、u16 操作 1、u32 显式开启 1、
  u32 reserved 0、16-byte 非零 nonce；binary 长度 0。
- response 精确 48 bytes、小端：`GNSR`、u16 版本 1、u16 status 0、u32 core ABI 2、
  u32 pointer bits、16-byte nonce、u64 可变 Atomic marker 地址、u32 fixture mode 1、
  u32 attempt 1–8。signal 名 `GuoNativeInputSdkProbeReply`。

marker 只用于进程内映射关联，不显示或记录地址。只有 main 实际进入隔离等待分支才
发送回应。先到请求保留在一个固定槽位；晚到请求直接回应，不新增线程或 async 任务。
启动发布与请求发布共用 AtomicU8 控制字，包含 ENTERED/CLOSED 位和
EMPTY/WRITING/READY/CLAIMED 阶段。请求的 READY 发布与启动的 ENTERED 发布以
AcqRel RMW 排入同一修改序：先发布 READY 时启动消费者负责，先发布 ENTERED 时
请求 producer 负责；二者都必须以同一 CAS 取得唯一 CLAIMED。payload 的 relaxed
写在 READY release 前完成，读取在 CLAIMED acquire 后进行；直到回应 guard 析构才
释放单槽。占槽失败不覆盖、不等待；writer 最多两次 CAS，预算、唯一 claim 与
关闭检查发射门各一次 CAS。
无锁等待、actor 等待、无界 spin 或自动重试。

fixture 等待 future 的 lifetime guard 在 shutdown 返回、取消或 unwind 时置位关闭，
关闭位不可清除；关闭后不能新占槽或消费，重复 enter 不恢复。发射门 CAS 与关闭在
同一控制字上线性化，关闭先发生则不发送。已 post/inflight 的回应仍由 Dart 的关闭／超时状态
拒绝，不能宣称 VM 交付被撤回。仅实收 matching nonce 且 native 核验通过才能成功。
Rinf 的发送 Ok 不表示 delivery，Dart 必须实收 matching nonce，native 再核验映射。

## 固定资源事实

静态控制类为一个 AtomicU64 marker、一个 AtomicU32 永不重置的预算、一个 AtomicU8
阶段／模式控制字、固定 16 个 AtomicU8 nonce 字节与一个 AtomicU32 slot attempt。
最多一个 ready/writing/claimed payload；guard 只有固定 nonce/attempt 和借用，不分配。
每进程最多 8 次合法
回应尝试；错误长度、版本、操作、reserved、零 nonce、缺 feature/opt-in 的请求在
owned copy 前拒绝，不分配错误对象或主动日志。FFI catch_unwind 不使 panic 跨 ABI；
有效指针与借用寿命仍是 caller 合约，不能靠数值校验证明。

每次回应应用侧 Vec payload 精确 48 bytes，binary Vec 为空；Rinf/allo_isolate 另有
endpoint/封包和 VM external typed data/finalizer 控制对象，其 allocator 布局不是这里
的固定 payload 大小。预算只限制尝试次数，不把 VM post 的返回值当 delivered，也不
宣称生产 P1 resource certificate。模块必须保持进程 lifetime；GUI 关闭不允许卸载。

独立 integration target 是源码／布局及固定屏障同步测试，不证明 live SDK。
测试模式仅捕获固定 reply 交接，不执行 VM post，不启动 app/Dart runtime 或真实配置；
真实四参 FFI 的启动竞争样例必须在 feature 与受控编译标记都开启时运行。
