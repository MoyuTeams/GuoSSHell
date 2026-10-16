# 持续集成与 GitHub Release

项目上游为 `MoyuTeams/GuoSSHell`。工作流位于 `.github/workflows/ci.yml`，
个人 fork 使用同一份配置；构建工具链与目标矩阵固定在 `scripts/ci/config.json`。

## 触发与门禁

- PR 和普通分支推送、普通分支手动运行：只做静态检查，不运行单元测试、不打包、不签名、不上传应用产物。
- `main`、`dev` 分支推送或在这两条分支手动运行：静态检查、单元测试、SSH 环回验证及完整应用构建。
- 受信日常构建产物保存为 Actions Artifacts，保留 14 天；同分支新推送会取消过时的运行。
- `vX.Y.Z` tag：提交必须已包含在当前仓库的 `main` 或 `dev` 历史中，全部检查与所有架构构建成功后发布正式 GitHub Release。
- `vX.Y.Z-rc.1` 等 SemVer 预发布 tag：发布标记为 prerelease 的 GitHub Release。
- tag 来源不可信、版本格式不合法、缺少目标、文件校验失败或产物来自不同提交时，发版失败。
- 发布先创建草稿、上传全部资产，再公开；重试可接续草稿，不覆盖已经公开的版本。

分支保护可以把稳定名称 `CI 全部通过` 设为必需检查。正式发布仅由 tag push 触发，
手动运行和 PR 不会发布，也不会把 PR 代码交给带发布写权限的任务执行。
判断使用 `github.event_name` 与完整 `github.ref`；fork PR 的来源分支即使叫 `main` 或 `dev`，
也不能打包。工作流不使用 `pull_request_target`。构建、签名及发布入口会核对实际检出提交与
GitHub 事件 SHA；版本标签另外核对受信分支历史。`CI 全部通过` 对静态检查流程要求打包与
签名任务均跳过，对受信构建流程要求全部目标成功。

## 检查范围

macOS、Linux、Windows 三类 runner 执行 Rust 格式检查、Clippy 和 Flutter analyze。
工作流语法由固定版本且校验 SHA-256 的 actionlint 检查，同时检查 Python 语法。
只有受信构建执行 Rust 与 Flutter 单元测试、生产 SSH 环回示例，以及版本解析、来源策略和
发布门禁的 Python 回归。Clippy 需要编译分析依赖，但不生成可分发应用包。

Flutter 与 rinf 绑定在干净环境生成，依赖必须满足已提交的锁文件。Actions 自身固定到
完整提交 SHA，普通任务只有仓库读取权限，只有最后的发布任务获得 `contents: write`。
Linux / Windows arm64 使用固定 Flutter 源码和原生 Dart SDK，拒绝静默退回 x64 工具链。
Apple 平台使用 macOS 26 runner（x86_64 iOS 模拟器使用 Intel 镜像），并切换到 `config.json`
固定的 Xcode 版本，实际版本不符时构建失败。
Android 使用兼容 rinf 的 Gradle 8 工具链，升级边界见
`docs/followups/20260927_rinf的Gradle9接口兼容.md`。

## 构建与下载

| 平台 | 架构 | 产物与签名状态 |
|---|---|---|
| iPhone / iPad | arm64 | release 配置、未签名 IPA，安装前需重新签名 |
| iOS 模拟器 | arm64、x86_64 | 各架构的 debug App 压缩包 |
| macOS | arm64 + x86_64 | 通用 App 压缩包，ad-hoc 签名，未公证 |
| Windows | x86_64、arm64 | 完整便携目录压缩包，未做 Authenticode 签名 |
| Linux | x86_64、arm64 | 完整目录压缩包和 deb 安装包 |
| Android | armeabi-v7a、arm64-v8a、x86_64 | 按架构拆分的 APK |

桌面包包含 Flutter 引擎、Rust 库与应用资源。打包脚本核对主程序及独立 Rust 库架构，
APK 必须包含对应 ABI 的 `libhub.so`。`SHA256SUMS.txt` 和每个目标的 manifest 记录
文件校验值、版本、构建号、提交、构建配置及签名方式。

GitHub Release 中的 iOS 与 macOS 产物不等于 App Store / TestFlight 发布；当前流程
不导入 Apple 分发证书，也不自动进行公证。iOS 模拟器的 Flutter 构建使用 debug 配置。

## Android 签名

仓库需要创建 `android-signing` **Environment**，启用 **Selected branches and tags**，
分别添加 `main`、`dev` 两条 Branch 规则和 `v*` 一条 Tag 规则。版本标签还必须通过工作流的
提交归属校验；环境规则本身不检查 Git 历史。分支与版本标签的写入权限仅授予受信维护者。

以下四项必须配置为该 Environment 的 **Secrets**，不能保留同名仓库级或组织级 Secrets，
也不能使用公开 Variables 保存口令：

| 名称 | 内容 |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | PKCS#12 发布 keystore 的 Base64 |
| `ANDROID_KEY_ALIAS` | keystore 内的私钥别名 |
| `ANDROID_STORE_PASSWORD` | keystore 口令 |
| `ANDROID_KEY_PASSWORD` | 私钥口令，与 PKCS#12 存储口令一致 |

个人仓库和组织仓库使用同一把发布密钥，保证两处构建的 APK 签名一致。已有发布密钥不能
随意替换，否则既有用户无法直接升级。应独立备份 keystore 和口令；GitHub Secrets 不提供
读回备份的能力。

Android 签名在独立任务中声明该 Environment，其余平台的构建不引用签名 Secrets。
PR 和普通分支不启动此任务；环境的 ref 限制同时阻止这些来源访问发布密钥。
受信构建缺少发布密钥时直接失败，禁止回退为开发签名 APK。临时 keystore 放在构建目录，
任务结束后清理，产物清单也拒绝混入任何未登记文件。

## 版本命令

```sh
# 在发布仓库的 main 或 dev 已包含的提交上创建版本标签，再推送标签。
git tag -s v1.0.0 -m "发布 1.0.0"
git push origin v1.0.0

# 预发布示例。
git tag -s v1.1.0-rc.1 -m "发布 1.1.0 候选版本"
git push origin v1.1.0-rc.1
```

tag 的版本覆盖 Flutter 的 build-name；预发布标识保留在发布名称与文件名中，Apple 包内
使用兼容的三段版本。所有目标共享构建号，两个仓库使用相同的时间基准，避免各自运行编号
造成 Android 更新版本倒退。日常构建以 pubspec 版本和提交短 SHA 命名。

## 平台运行条件

- Android：应用启动时注册 Android 上下文，私钥和口令交给 Keystore 加密的系统存储；
  备份关闭，避免将无法跨设备解密的凭证文件恢复到其他设备。
- Windows：长私钥和口令使用当前用户的 DPAPI 加密文件，SSH 登录密码仍使用 Windows
  凭证管理器。密文绑定条目种类与 ID，不能直接交换或复制给其他用户使用。
- Linux：以 Ubuntu 24.04 为构建基线，需要相应运行库、图形桌面、D-Bus 会话和可用的
  Secret Service（如 GNOME Keyring）。deb 会声明
  这些依赖；压缩包用户需要自行提供运行环境。没有安全存储时不回退到明文。
- iCloud 同步只在支持的 Apple 构建中可用。其他平台的 OpenPGP 卡和 FIDO2 系统接入仍见
  `docs/followups/20260927_新增平台的硬件认证与真机验收.md`。

CI 的编译、单元测试与系统存储回归不能替代各平台实体键盘、移动设备生命周期和硬件密钥验收。
