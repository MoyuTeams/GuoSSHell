# Android 发布插件注册与 --no-pub

## 问题

Flutter 的 `--no-pub` 构建可能保留开发依赖的插件注册，却从 Android release classpath
排除对应插件；含 `integration_test` 的应用因此不能编译。上游问题为 flutter/flutter#169336。

## 现状

CI 的 Android release 构建由 Flutter 正常更新插件注册，不传 `--no-pub`。
构建前按锁文件获取依赖，构建后再次核对两个锁文件未变化；测试插件仍保留为开发依赖。

## 下一步

升级到已修复该问题的 Flutter 后，验证初始化依赖与 release 构建分步运行、插件注册表
不包含开发插件，再决定是否恢复 `--no-pub`。
