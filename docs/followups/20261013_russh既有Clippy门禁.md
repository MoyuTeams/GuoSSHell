# russh 既有 Clippy 门禁

## 问题与现状

团队 russh 的 scoped 资源接缝仍为实验路径。其库在既有 GNU 工具链下，以 `ring,rsa,flate2` features 执行严格 Clippy 检查未通过；诊断位于 agent 的 map 遍历、共享 session 的饱和运算，以及 server 返回 `Result<_, ()>` 的公开方法。局部 scoped、真实连接及旧 KEX 回归通过，不替代此门禁或完整网络验收。

Windows 的全 examples 检查也未通过：`client_exec_interactive` 使用仅在非 Windows 目标声明的 `termion` 和 `tokio-fd`。库与测试目标检查通过不代表全部 examples 可编译。

## 下一步

核对受支持构建组合与上游检查规则，区分内部实现修正和必须保留的公开接口，并明确 interactive example 的目标边界。保持既有 API、协议与认证行为，不通过压制检查或改变返回类型换取通过。完成必要的最小兼容修正后重跑严格检查及受影响回归；正常网络仍须完整资源图和三平台证据。
