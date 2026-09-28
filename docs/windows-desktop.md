# Windows 桌面界面

Windows 使用独立的 Fluent 深色工作区，窗口管理与亚克力只在 Windows 初始化。其他平台保留各自的导航与窗口行为，共用 [字体设置](fonts.md)。

## 布局与鼠标

- 自绘标题栏支持拖动、双击最大化、还原、最小化和关闭；窗口边缘可以调整尺寸。
- 左侧连接目录常驻，可拖动边界调整宽度，或折叠侧栏；拖动区透明，只显示一条工作区边界。
- 连接单击选中，双击或 Enter 打开；右键和省略号菜单提供连接、编辑、复制与删除。
- 标签支持拖动排序、中键关闭和溢出滚动。重排、切换标签与打开设置不销毁会话。
- 终端遵循各平台共用的[选区与复制粘贴规则](terminal-input-and-controls.md#选区与复制粘贴)：鼠标右键复制或粘贴，触摸时才显示触屏选区控件。
- 左右或上下分屏在当前会话旁打开同一连接的新会话；分隔线可拖动，活动窗格有高亮边框。
- 关闭仍在连接或已连接的窗格、标签及窗口时先确认断开。
- 连接编辑、私钥管理、身份验证与外观设置使用 Fluent 控件。主机密钥变更仍须核对新指纹后才能替换。

## 键盘

| 操作 | 快捷键 |
|---|---|
| 快速连接、新建标签 | Ctrl+Shift+T |
| 关闭活动窗格 | Ctrl+Shift+W |
| 左右分屏 | Ctrl+Shift+D |
| 上下分屏 | Ctrl+Shift+E |
| 下一个／上一个标签 | Ctrl+Tab／Ctrl+Shift+Tab |
| 下一个窗格 | Ctrl+Shift+P |
| 搜索连接 | Ctrl+Shift+F |
| 折叠／展开侧栏 | Ctrl+Shift+B |
| 设置 | Ctrl+, |
| 终端缩放／恢复 | Ctrl+加号、Ctrl+减号、Ctrl+0 |
| 编辑／删除选中的连接 | F2／Delete |

Ctrl+C、Ctrl+D、Ctrl+W 等控制键保留远端终端含义；文本输入框内不执行终端缩放。

## 材质与数据

设置中的「窗口外观」提供亚克力开关及 0–65% 背景透明度。拖动即时预览，松开后保存；文字、图标和终端显式背景色不随透明度降低。

Windows 使用接受色调 alpha 的原生 Accent 亚克力分支，并关闭 Windows 11 的固定色调系统背景，避免重复叠加不透明材质。统一通过 Flutter 背景遮罩控制透明度，不使用会让文字一起变淡的整窗透明度。系统材质不可用或启用高对比度时使用不透明背景。

窗口偏好由 Rust 保存，使用 `windows_acrylic`、`windows_opacity` 字段；缺省时使用默认值，不覆盖字体或键位条设置。

## 参考与边界

布局参考官方 rsHell 固定提交 `718d9b62a8f062af8f5787b5fc27f6c5bbb4f268` 中的 `crates/rshell-ui/src/main_window_shell.rs`、`main_window_layout.rs`、`connection_sidebar_widgets.rs`：常驻侧栏、标签工作区、分屏和明确的键盘焦点。只复用交互结构，不引入 GTK，也不暴露本地 shell。

控件使用 `fluent_ui`，窗口行为使用 [window_manager 的局部适配](../packages/window_manager/UPSTREAM.md)，材质使用 [flutter_acrylic 的局部适配](../packages/flutter_acrylic/UPSTREAM.md)。保留各平台实现与自动注册；Linux 的窗口事件接管和透明绘制仅在显式初始化后生效，应用仅由 Windows 入口调用这些接口。

## 验证

`test/windows_desktop_test.dart` 覆盖平台分流、控制键保留、窄窗口布局、目录查询关联、标签重排、标题栏生命周期和主机密钥确认。

`integration_test/windows_desktop_test.dart` 验证窗口行为、Fluent 编辑器、两类字体的加载与持久化、透明度设置。运行前通过 `GUOSH_DESKTOP_TEST_DATA` 指定隔离数据目录；测试自动结束，不作为交互版分发。

`scripts/ci/linux_plugin_check.py` 编译真实 GTK/Flutter 插件并检查自动注册不改变窗口外观、可见性或关闭处理；受信 Linux CI 在虚拟显示环境运行此项。

其他平台的原生构建与实体设备适配仍由现有 CI 矩阵及对应平台验收负责。
