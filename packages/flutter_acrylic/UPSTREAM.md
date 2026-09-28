# flutter_acrylic 的 Windows 色调适配

来源：`alexmercerind/flutter_acrylic`，pub.dev 发布版 `1.1.4`，MIT。保留上游 `LICENSE` 及 Dart、Windows、Linux 的实现；macOS 仍由上游声明的 `macos_window_utils` 提供。

Windows 增加默认关闭的 `customAcrylic` 选项。开启时关闭固定色调的系统背景，使用上游已有的 Accent API 应用接受 alpha 的亚克力；原生调用失败会向 Dart 返回错误。未开启选项时沿用原行为，macOS 和 Linux 不接收该选项。

Linux 自动注册仅建立方法通道；GTK 透明绘制、绘制回调与窗口显示延后到显式 `Initialize`，避免未使用亚克力的平台改变窗口行为。

升级时比较 `lib/window.dart`、`windows/flutter_acrylic_plugin.cpp` 和 `linux/flutter_acrylic_plugin.cc`，并复测材质开关、透明度两端、窗口移动、中文界面及 Linux 注册隔离。
