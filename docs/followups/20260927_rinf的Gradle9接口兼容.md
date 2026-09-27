# rinf 的 Gradle 9 接口兼容

## 问题

当前 rinf 依赖的 Cargokit 构建任务使用 `Project.exec`，该接口在 Gradle 9 中已移除。

## 现状

Android 工程固定使用 Gradle 8.14.5、Android Gradle Plugin 8.13.2 和 Kotlin 2.3.21。
构建不修改 Pub 缓存内的第三方代码；升级到 Gradle 9 需要先有兼容的 Cargokit 接口。

## 下一步

采用已改用 `ExecOperations` 的 rinf / Cargokit 版本后，联合升级 Android 构建工具链，
验证三个 ABI 的 Rust 库、APK 签名及 Android Keystore 初始化。
