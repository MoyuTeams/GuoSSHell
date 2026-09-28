# window_manager 的 Linux 初始化适配

来源：`leanflutter/window_manager`，pub.dev 发布版 `0.5.2`，MIT。
保留原始许可、Dart 接口、各桌面平台实现和测试。

局部适配仅修改 `linux/window_manager_plugin.cc`：自动注册只创建方法通道，
GTK 窗口事件及鼠标钩子在显式调用 `ensureInitialized` 时安装，重复调用不重复安装。
未使用窗口管理功能的平台保留原有关闭和输入行为；Windows 与 macOS 实现保持原样。

升级时核对 Linux 注册和初始化路径，并运行插件隔离检查、应用测试及桌面构建。
