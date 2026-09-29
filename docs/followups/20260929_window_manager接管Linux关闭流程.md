# window_manager 接管 Linux 窗口关闭流程

- 状态：待办（非阻塞）
- 记录日期：2026-09-29
- Issue：[MoyuTeams/GuoSSHell#5](https://github.com/MoyuTeams/GuoSSHell/issues/5)

## 问题

`window_manager` 0.5.2 在 Linux 上随插件自动注册即接管窗口：移除 Flutter 引擎自身的
`delete-event` 处理，改由插件处理关闭，并安装窗口状态监听与鼠标按下钩子。应用只在
Windows 入口调用该插件，Linux 上同样生效。

关闭窗口时 GTK 直接销毁窗口，不经过 Flutter 的退出请求，`AppLifecycleListener.onExitRequested`
等退出回调在 Linux 上不会触发。引擎随后关闭，标准错误输出
`'FlutterEngineRemoveView' returned 'kInvalidArguments'` 及若干条
`Attempted to set message handler on an FlBinaryMessenger without an engine` 告警。

## 现状

- 应用在 Linux 上不使用退出回调，关闭后进程立即正常退出、无残留，用户不可见。
- 启动绘制、鼠标点击、表单键入、Tab 与退格与接管前的行为一致。
- 按 AGENTS.md「上游依赖」规则，当前属于「上游能力不好用」，不修改上游。

## 下一步

1. Linux 需要关闭确认（例如仍有连接时询问是否断开）或其他退出回调时，此问题即构成
   「上游 bug 挡路」：在 `MoyuTeams/window_manager` 做最小补丁，使 Linux 仅在显式调用
   `ensureInitialized` 后接管窗口，并记入根 `UPSTREAM.md`；
2. 或在 Linux 上改用 `windowManager.setPreventClose` 与 `onWindowClose` 实现关闭确认，
   届时比较两种方式并选定；
3. 升级 `window_manager` 时复查其 Linux 注册行为是否已改为按需接管。
