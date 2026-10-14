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
- 连接编辑、私钥管理、身份验证与设置使用 Fluent 控件。主机密钥变更仍须核对新指纹后才能替换。

## 键盘

连接编辑器可直接导入私钥或登记 OpenPGP 卡，成功后选中新凭据并保留连接草稿。编辑与密钥面板不替换工作区或会话；返回后点击终端可继续输入。

密钥页面逐项显示本机／同步存储位置。迁移请求、未确认结果与操作锁由常驻壳持有，切换面板不会解除；删除确认重新核对引用及存储位置，同步私钥明确提示删除的跨设备影响。

| 操作 | 快捷键 |
|---|---|
| 快速连接、新建标签 | Ctrl+Shift+T |
| 打开本地终端（终端持焦也可用） | Ctrl+Shift+L |
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

设置页（Ctrl+,）分应用、配置文件、外观与字体、键位条、关于五页，三个桌面平台共用；终端字号与滚回在「配置文件」页，应用配色在「外观与字体」页。Windows 的「外观与字体」页另提供亚克力开关及 0–65% 背景透明度。拖动即时预览，松开后保存；文字、图标和终端显式背景色不随透明度降低。

材质由 Windows 宿主的 `windows/runner/window_material.cpp` 实现，经方法通道 `guosshell/window_material` 切换：使用接受色调 alpha 的原生 Accent 亚克力，并关闭 Windows 11 的固定色调系统背景，避免重复叠加不透明材质。统一通过 Flutter 背景遮罩控制透明度，不使用会让文字一起变淡的整窗透明度。系统材质不可用或启用高对比度时使用不透明背景。

窗口偏好由 Rust 保存，使用 `windows_acrylic`、`windows_opacity` 字段；缺省时使用默认值，不覆盖字体或键位条设置。

## 参考与边界

布局参考 rsHell 的 `crates/rshell-ui/src/main_window_shell.rs`、`main_window_layout.rs`、`connection_sidebar_widgets.rs`：常驻侧栏、标签工作区、分屏和明确的键盘焦点。依赖固定版本见根 `UPSTREAM.md`。只复用交互结构，不引入 GTK。本地终端与 System OpenSSH 在本地终端标签中运行（见[桌面扩展](desktop-expansion.md)）。

控件使用 `fluent_ui`，窗口行为使用 pub.dev 发布版 `window_manager`，应用仅由 Windows 入口调用其接口。材质不依赖第三方插件。

## 验证

`test/windows_desktop_test.dart` 覆盖平台分流、控制键保留、窄窗口布局、目录查询关联、标签重排、标题栏生命周期和主机密钥确认。

`integration_test/windows_desktop_test.dart` 验证窗口行为、Fluent 编辑器、两类字体的加载与持久化、透明度设置。`integration_test/desktop_settings_e2e_test.dart` 验证设置页中 profile、默认 profile、应用配色与键位条开关的保存与读回。这两项运行前通过 `GUOSH_DESKTOP_TEST_DATA` 指定隔离数据目录；测试自动结束，不作为交互版分发。

`flutter test integration_test/desktop_local_shell_smoke_test.dart -d windows --no-pub` 验证本地终端标签的打开、输入输出与退出处理。

`scripts/ci/windows_input_check.py` 启动隔离的环回 SSH 服务，运行 `integration_test/windows_terminal_input_test.dart`。验收向测试窗口发送原生键盘和已提交字符消息，核对英文数字、中文提交、回车及方向键到达对端的字节，并覆盖密码弹窗、设置返回、标签切换和鼠标清选区。此项不替代实体中文输入法的候选窗验收。

`flutter test integration_test/desktop_catalog_smoke_test.dart -d windows --no-pub` 验证真实 Windows 壳中的目录 CRUD、私钥文件错误及独占存储清理。

`pwsh -NoProfile -File scripts/ci/windows_auth_check.ps1` 验证隔离 Native SSH 的 Fluent 密码与首次主机指纹确认、存活会话管理往返、原生字符输入的对端字节与展示回显，以及断开后返回目录。`-VerifyKeygenAbort` 单独验证首次主机密钥生成期间父进程退出后的 Job 清理，不计作 SSH 成功链。runner 不预写应用信任库，服务与宿主执行前纳入各自 Job，测试及清理错误均返回失败。`-Target profile` 验证 profile 的 TERM、answerback、绑定与左右 Alt；`-Target system` 验证 System OpenSSH，经仅 debug 构建读取的 `GUOSH_SYSTEM_SSH_CONFIG` 指定隔离的 `-F` 配置，不使用用户自己的 ssh 配置。

这些验收不覆盖生产启动时的双字体等待、profile 字体渲染、实体 IME 或其他平台 GUI。

其他平台的原生构建与实体设备适配仍由现有 CI 矩阵及对应平台验收负责。
