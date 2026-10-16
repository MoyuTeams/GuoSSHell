# 应用内 tailnet 接入：调研与设计（未实施）

- 状态：暂不实施，仅保留设计
- 日期：2026-09-28

## 目标

手机已开着别的 VPN 时无法再开系统 Tailscale，也就连不上只在 tailnet 里可达的 SSH 主机。
设想在 App 内自带一个 tailnet 用户态节点（不占系统 VPN / TUN），经它连接远端主机，
并在连接编辑页自动发现 tailnet 里的设备（主机名、100.x 地址、MagicDNS 名、在线状态）。

## 接入点

上游 rsHell 的 `NativeSshTransport` 固定用 `TcpStream::connect` 建连，且主机密钥按 TCP 对端
IP 记录。接入任何非直连的通道都需要上游先提供：

- 连接注入：`StreamConnector` trait（`connect(host, port) -> Box<dyn AsyncRead + AsyncWrite>`），
  `NativeSshTransport::with_connector` 用它代替直连 TCP；
- 经注入连接建立的会话，主机密钥按配置里的主机名与端口校验和记录（注入的流没有对端地址）。

russh 的 `connect_stream` 接受任意 `AsyncRead + AsyncWrite`，其余认证、通道逻辑不变。

## 实现路线

| 路线 | 结论 |
|---|---|
| Go tsnet + 自写 C 接口 | 功能最完整、最稳；需要 Go 工具链与 cgo 交叉编译，进程内多一套 Go 运行时 |
| libtailscale（官方 C 库） | 不支持 Windows（依赖 socketpair）；状态只能放目录；依赖的 tsnet 版本滞后 |
| tailscale-rs（官方 Rust 实现，0.6.x） | tokio 原生，连接可直接交给 russh；1.0 之前，稳定性不足（见下） |
| 自行用 Rust 重写 | 需要实现控制协议、WireGuard、DERP、打洞、用户态 TCP，工作量以月计且要长期跟进协议，不考虑 |

### Go tsnet 方案要点

- tsnet v1.102.5，按 `ts_omit_*` 构建标签裁掉用不到的功能（logtail、syspolicy、ssh、taildrop、
  drive、webclient、relayserver 等）；`envknob.SetNoLogsNoSupport()` 关闭日志上传。
  裁掉 syspolicy 也避开了 Windows 上系统策略覆盖 `ControlURL` 的问题。
- C 接口：启动（配置 JSON、节点状态、持久化回调、事件回调）、停止、状态与设备列表（JSON）、
  交互登录、退出登录、重新绑定网络、拨号、按连接编号阻塞读写、半关闭、关闭。
- 节点状态用自定义 `ipn.StateStore` 整份交给宿主保存（可放进系统钥匙串），不落盘。
- 连接不经本机 socket：拨通后按编号读写，Rust 侧用内存管道和两个阻塞线程接成 tokio 字节流。
- 登录：无预授权 key 时 tsnet 自动发起交互登录，经 IPN bus 的 `BrowseToURL` 取登录链接。
- 体积（strip 后）：Android arm64 约 19.7 MB（gzip 7.0 MB），Linux x86_64 约 21.6 MB，
  Windows x64 约 19.7 MB。
- 各平台的坑：
  - iOS：挂起后 socket 被回收且不自愈（tailscale#21353），回前台须重建或重新绑定；
  - Android：Android 11 起不能用 netlink，需 `netmon.RegisterInterfaceGetter` 注入网卡列表
    （tailscale#17311）；需 16KB 页对齐；
  - Windows：Go 的 c-archive 无法与 MSVC 链接，只能 c-shared；cgo 用 llvm-mingw；
  - 进程内只能有一份 Go 运行时；Go 动态库不能 dlclose。

### tailscale-rs 验证结果

在本地 Headscale（v0.29）上，用 tailscale-rs 与 GuoSSHell 同版本的 russh 验证：

- 预授权 key 注册、按名字查找设备、经 tailnet 完成 SSH 握手、认证、执行命令均可用；
  补一个列出全部设备的接口约 50 行；
- Android：网络监视模块补一处平台条件后可编译运行，模拟器上端到端可用；
- 新建 TCP 连接约 2.3～2.5 秒（同环境 Go tsnet 约 0.7 秒）：WireGuard 握手完成前的包被丢弃，
  依赖 TCP 重传；
- 稳定性：启动时连不上控制服务器会 panic 且 `Device::new` 不返回，DERP 客户端与关闭流程中也有
  panic；
- 控制服务器公钥只允许经有效证书的 HTTPS 获取；设备信息不含操作系统；不支持 MagicDNS；
- 体积增量约 15.6 MB（x86_64）。

结论：tailscale-rs 功能可用但尚不稳定，需等上游或自带补丁修复 panic 后再考虑。

## 选装插件设计（Go 方案）

### 各平台能否选装

| 平台 / 渠道 | 选装 | 机制与限制 |
|---|---|---|
| iOS、Mac App Store | 否 | 审核指南 2.5.2 / 2.4.5 禁止下载代码；iOS 只能加载系统库与 App 包内的库。只能打进包里 |
| macOS 直接分发 | 是 | 下载 dylib 后加载；需与主程序同一 Team ID 签名并公证，不关闭库验证 |
| Android 侧载 | 是 | 下载 .so 到私有目录、设为只读后按绝对路径加载（targetSdk 29+ 仍允许 dlopen） |
| Android（Google Play） | 是 | 只能用 Play Feature Delivery 按需模块；Flutter 的延迟组件不能承载 Go 的 .so |
| Windows | 是 | 下载 DLL 后加载；Windows 11 智能应用控制要求 DLL 有 Authenticode 签名 |
| Linux | 是 | 下载后加载，或另出 deb 包 |

### 架构

- 插件是只导出 C 接口的动态库（`libguosh_tailnet.so` / `guosh_tailnet.dll` /
  `libguosh_tailnet.dylib`），导出接口版本与插件信息，App 只加载接口版本一致的插件；
- Go 代码独立于 cargo 工作区，只有构建插件时需要 Go；iOS 用同一份代码编成静态库链进 App；
- hub 的加载器依次查找内置、安装包内与用户安装目录；安装包内的插件随 App 签名，
  下载安装的插件须校验签名清单（Ed25519，公钥编入 App）并在每次加载前校验哈希；
- 插件在进程内与 App 同权限（可访问钥匙串），不加载未经验证的文件；
- 卸载与更新须重启 App 生效（Go 动态库不能卸载）；
- 第一步可只做「含 / 不含插件」两种安装包，后续再做 App 内下载安装。

### 估计工作量

- tailnet 功能本身（hub 接入、登录、设备列表、界面）约 5 天；
- 插件边界、CI 构建与签名、App 内下载安装与界面状态约 5～7 天；
- iOS 静态集成约 1～2 天；上 Google Play 时的按需模块约 2 天。

## 参考

- tsnet：https://pkg.go.dev/tailscale.com/tsnet
- libtailscale：https://github.com/tailscale/libtailscale
- tailscale-rs：https://github.com/tailscale/tailscale-rs
- Headscale 自定义控制服务器：https://tailscale.com/docs/how-to/set-up-custom-control-server
- Apple 审核指南：https://developer.apple.com/app-store/review/guidelines/
- Google Play 设备和网络滥用政策：https://support.google.com/googleplay/android-developer/answer/9888379
- Play Feature Delivery 加载原生库：https://developer.android.com/guide/playcore/feature-delivery/on-demand
- Go 动态库不能卸载：https://github.com/golang/go/issues/11100
