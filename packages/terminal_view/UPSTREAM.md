# terminal_view 的本地适配

来源为 `Anthony-Hoo/terminal_view` 固定提交 `f0a1e0d9a717311ab622581508bdbacdf65f19de`，版本 0.2.1，保留原始 `LICENSE`、`NOTICE`、实现和测试。该 fork 基于 `Termphin/terminal_view`，提供本项目使用的帧渲染接口。

局部适配仅在 `lib/src/terminal_view.dart` 增加默认开启的 `showSelectionHandles` 参数。GuoSSHell 在所有平台按实际输入设备控制手柄：触摸或触控笔显示，鼠标、触控板及键盘隐藏。终端手势、鼠标上报和帧渲染逻辑保持来源版本的实现。

采用本地快照使打包包含此 API；不修改 Pub 缓存，也不依赖未提交的覆盖文件。升级时对照来源提交合并该参数，并运行库测试和应用的选区、鼠标上报回归。
