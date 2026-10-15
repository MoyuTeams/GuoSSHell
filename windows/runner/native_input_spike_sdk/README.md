# Windows SDK bootstrap 适配接缝

本目录不接 HWND、TSF、WindowSource、keyboard/text/pointer owner，也不调用 open、key、
revoke、join 或 peer；core 唯一调用为 `inspect(nullptr, &snapshot)` 的无 owner 诊断。
共享头是 ABI 2、64 位布局；null-owner snapshot 的 hub marker 与 bootstrap Atomic
marker 可以不同，但必须落在同一个 MEM_IMAGE/module 地址范围。

## 父级最小 native hook

父级在受控 Windows 构建中编译 `bootstrap_host.cpp`，为独立 fixture 注册一个持久
`BootstrapHost` 的 `guosshell/native_input_spike_sdk` MethodChannel handler。
普通 runner 不调用 Preflight，显式开启为 false 时也不加载 DLL。
handler 在 platform thread 串行处理且只调用以下两个方法：

- `preflight`：参数 `path`（Dart compile define 的受控本地绝对路径）、
  `explicitOptIn`（bool）、`nonce`（精确 16 bytes）。长度／类型不正确直接返回失败。
  调用 `Preflight`，仅 status=0 时返回 `compiledLibPath=compiled_lib_path()`。
- `verify`：参数 `reply`（精确 48 bytes），调用 `Verify`。
- 两者返回 map：`status`、`coreAbi`、`pointerBits`、`owners`、`rinfExports`、
  `coreExports`（整数），`fixtureOnly`、`sameModule`（bool）。异常统一失败，不输出
  文件路径、地址、nonce、输入、配置或凭据；不把失败映射成 success。

Rust 隔离 main hook 必须先完成并审查，之后才能允许这里返回的路径交给 SDK。
当前模块 mode query 表示受控编译意图；main 分支实际进入由 nonce 回应另行证明。
没有 hook 的 DLL 不能因编译标记为 1 就启动。应用的 `native_input_spike_sdk_channel`
已接入这两个方法；仅精确进程参数 `--guosshell-native-input-spike-sdk` 注册通道，默认不注册。

## 同 module 与 lifetime

仅接受本地固定驱动器的 drive-absolute 路径，规范化为最终路径并拒绝
UNC/设备/ADS/reparse。此输入必须是父级受控构建文件，不承诺抵御文件系统并发替换。
预载和 SDK
`compiledLibPath` 使用同一 canonical path，不 copy 第二份 DLL，不链接静态 Rust。
预载本身不是 sameHub 证明；只有 SDK 公共 send 收到的实际回应 marker 经 native
`GetModuleHandleExW(FROM_ADDRESS)`、VirtualQuery 与映射范围核验后才为 sameModule。
Rinf 4 与 core 8 导出必须从该 module 的 GetProcAddress 解析，不接受 forwarder。
Windows Rinf 启动/停止/prepare 是全局 @Native；pre/post 都要求当前进程没有其他
module 携带同名 Rinf 导出。模块枚举最多 256，超容量失败，不在 GUI 上重试。

显式启动固定一个 nonce，一个 adapter，每进程一次 preflight，一次 verify。
模块用 FROM_ADDRESS|PIN 保持直到进程终止；本组件没有 FreeLibrary 或 close 卸载。
这保护 VM external typed-data finalizer 及 SDK 启动线程，不把 worker_done 当 join。
本组件不管理输入 worker；后续 owner 仍须遵守真实退休／join 与不可重绑的生命周期契约。

契约来源：Rinf 8.10.1 的公共 initializeRust 与 sendDartSignal，原始 load_os.dart
中的 override 与 lib.lookupFunction；Dart DynamicLibrary.open 官方约定同 path
在 VM 只加载一次；Windows LoadLibraryExW 绝对路径与 GetModuleHandleExW PIN 契约。

- <https://api.dart.dev/dart-ffi/DynamicLibrary/DynamicLibrary.open.html>
- <https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw>
- <https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulehandleexw>

## 独立 entry 的构建与启动

Rust 已接入隔离分支和 module，Windows 已接入默认关闭的 MethodChannel handler。
先在独立的受控构建 shell 设置 `RUSTUP_AUTO_INSTALL=0` 与 `GUOSSH_NATIVE_INPUT_SPIKE_FIXTURE=1`，
用现有精确 GNU toolchain 运行 `cargo build -p hub --features native-input-spike
--locked --offline --target-dir <受控构建目录>`。不设置用户／系统全局环境，不复制
或额外静态链接 hub；canonical path 指向这一个 DLL。普通 GUI 使用普通构建产物。

独立 Flutter 构建参数是 `flutter build windows --no-pub --target
lib/native_input_spike_main.dart --dart-define=GUOSSH_NATIVE_INPUT_SPIKE_SDK=true
--dart-define=GUOSSH_NATIVE_INPUT_SPIKE_DLL=<该受控 DLL 的本地绝对路径>`。
默认 bool define 为 false，默认 DLL define 为空，不会初始化 SDK。不存在运行时路径
编辑框或自动重试；点击“运行一次”才开始。文件路径不用于同 module 判定，不显示。
启动该 fixture executable 时必须额外传入 `--guosshell-native-input-spike-sdk`，
否则没有 native 通道，SDK 初始化前即失败关闭。真实集成验证入口为
`integration_test/native_input_spike_sdk_test.dart`；使用公共 SDK 实际回应、同 module
核验和空 core 诊断，不以 widget／loader 单测代替这条链路。

控制状态固定为一份 native host、16-byte nonce、48-byte borrowed reply、80-byte
null-owner snapshot、最多 256 个 module handle 的临时数组与有上限的路径缓冲。
Dart 固定一个 controller/callback/completer/reply timer；每次仅一个 preflight 和
一个 verify，等待有超时，失败没有 fallback/retry。MethodChannel 与 SDK 自身还有
封包、Future、VM finalizer 控制对象，不能把 payload 大小当成全部 heap 或 P1 预算。

## 测试边界

`build_test.ps1 -VcVars64 <现有 MSVC vcvars64.bat> -OutputDirectory <opencode 临时父目录下的全新子目录>` 编译并运行本目录的 loader 单元测试，拒绝覆盖，退出时仅清理本次确切产物。测试覆盖已核验映像借用的关闭与开放条件，不等于 live SDK 或输入来源验收。
资源固定在 `.test-build`，不自动删除，不安装工具、不改共享 CMake 或全局环境。
DLL 是明确的 C++ test double，未含 Rust/SDK，不能登记为 live SDK 或完整 Wave A。
实际 SDK 验证与普通 GUI、Windows 输入、其他平台的验收相互独立；不能用握手
结果声明物理输入、IME、pointer 或完整 Wave A 已通过。
