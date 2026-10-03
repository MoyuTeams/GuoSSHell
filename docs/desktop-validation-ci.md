# 桌面开发分支验证

`.github/workflows/desktop-validation.yml` 是独立的无秘密验证流程：`feat-desktop`
推送、以 `feat-desktop` 为来源或目标分支的 PR，以及在该工作流可见的分支上
手动触发。其他 PR 触发事件后不运行验证作业。
它不会调用 `.github/workflows/ci.yml` 的版本、签名、打包或发布任务；后者的受信
分支与标签门禁保持原样。

`workflow_dispatch` 已声明，但 GitHub 要求工作流文件先存在于仓库默认分支，才会
提供手动运行入口；只有 `feat-desktop` 分支新增文件时，不能保证远端手动入口可见。

Windows 2025、macOS 15 与 Ubuntu 24.04 各自使用既有 setup action 准备工具链，
运行 Cargo fmt/check/clippy/test 与 Flutter analyze/test，然后在本机宿主构建对应的
debug 桌面应用。macOS 使用构建目录下的临时配置关闭代码签名，不改沙箱权限；所有
job 仅有 `contents: read`，checkout 不持久化凭据，不访问签名 Environment 或 Secrets，
不通过 artifact 上传步骤归档应用产物或额外终端录制；GitHub Actions 仍保存步骤
日志和作业摘要，并沿用既有 setup 缓存。Ubuntu 上先运行既有 Python 辅助脚本回归和
actionlint。

工具、项目路径或依赖缺失，以及实际测试或构建失败，均以对应步骤失败呈现，不能
被当作无图形环境的跳过。当前流程**没有执行** Windows 原生 Flutter smoke、三平台
真实 PTY / System SSH 测试或 macOS 沙箱内运行；也没有探测 runner 是否可启动 GUI，
因此构建成功仅证明构建，不等于原生运行验收。后续阶段接入受控 fixture、真实测试
及清理核验后再更新覆盖范围。GitHub Actions 作业摘要标记这些尚未验证的运行面。
