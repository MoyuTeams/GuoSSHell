# rinf 插件不支持 Swift Package Manager（Flutter 预告将来报错）

- 状态：待办（非阻塞）
- 记录日期：2026-09-25

## 现状

Flutter 3.47 默认开启 Swift Package Manager 集成，工程已完成迁移
（`project.pbxproj` 里有 `FlutterGeneratedPluginSwiftPackage`）。但 `rinf`
（8.10.1）的 iOS 侧只提供 podspec，所以它仍经 CocoaPods 集成（`ios/Podfile`、
`ios/Podfile.lock`）。每次 iOS 构建都会提示：

```
The following plugins do not support Swift Package Manager for ios:
  - rinf
This will become an error in a future version of Flutter.
```

目前只是警告，模拟器与真机构建、运行都不受影响。

## 下一步

- 升级 Flutter 前，先看目标版本的发布说明是否已把这条警告改成错误。
- 关注 rinf 上游是否提供 `Package.swift`（它的 iOS 构建靠 podspec 里的
  Cargokit 脚本阶段驱动 cargo，迁移要由上游完成）；上游支持后升级 rinf，
  再移除 Podfile / CocoaPods 集成。
- 若 Flutter 先一步把它变成错误而 rinf 还没跟上：暂时固定 Flutter 版本，
  或按届时 Flutter 文档为本项目关闭 SwiftPM 集成。
