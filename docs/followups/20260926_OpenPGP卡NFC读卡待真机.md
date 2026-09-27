# OpenPGP 卡的 NFC 读卡待真机验证

- 状态：待验证（非阻塞）
- 记录日期：2026-09-26

## 现状

添加 OpenPGP 卡与用卡登录都支持 NFC：设备能用 NFC 读卡（iOS 26 起的 CryptoTokenKit NFC
卡槽）时，「添加 OpenPGP 卡」页有「通过 NFC 读取」，连接时卡没插着就先问 PIN、再弹系统的
NFC 界面等卡靠近。Info.plist 已声明 OpenPGP 应用标识（`D27600012401`）与 NFC 用途说明。

这条路径没有在真机上跑过：模拟器没有 NFC，也没有签名构建。

## 下一步

1. 签名配置里开启 NFC 读取能力（Near Field Communication Tag Reading），装到 iOS 26 设备；
2. 用支持 OpenPGP-over-NFC 的卡（如 CanoKey）走一遍：添加卡 → 连接 → 输 PIN → 靠卡；
3. 若系统要求的 entitlement 或 Info.plist 键与现有声明不同，按实际补齐。
