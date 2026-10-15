# rsHell 的重复 must_use 诊断阻断严格 clippy 检查

## 问题与现状

rsHell 在 Rust 1.99 下存在两组既有的 `clippy::double_must_use` 诊断：core 模块 16 项，以及 session 中 `ExternalSigner::sign` 和 `SessionTransport` 五个异步方法的 6 项宏生成诊断。后者来自 `async_trait`，相关声明和依赖未修改。

以 `-D warnings` 执行 core/session 检查时，core 诊断阻断检查；单独对 session 执行 `--no-deps` 的严格检查也被上述 6 项诊断阻断。因此不能把 session 或其新增测试的严格 lint 检查记为通过。

该问题不属于桌面兼容键盘事件或 Kitty 原生状态修复的批准补丁范围。本次不修改相关声明，不屏蔽 lint，也不弱化检查参数。相关功能测试的结果不能替代严格 clippy 通过。

## 下一步

单独评估上游相关声明的 `must_use` 意图和 `async_trait` 宏生成代码，按锁定工具链核对诊断与修正范围。取得必要修改授权后，以最小补丁保留调用者诊断语义，并重新执行 core/session 的严格 clippy 检查及受影响回归。
