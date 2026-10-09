# xcoc 文档

本文档集描述当前 `1.0.0` 实现。命令行帮助、`src/main.rs` 的严格输入结构和
`src/device.rs` 的适配器模型是行为事实源；Release 说明只记录对应历史版本的变化。

| 文档 | 内容 |
|---|---|
| [../README.md](../README.md) | GitHub 首页简介、最短配置路径和开发验证 |
| [platform-setup.md](platform-setup.md) | Linux、Windows、macOS、Android、iOS 安装、配对/重配、服务或采集启停、诊断与卸载，含命令解释 |
| [configuration.md](configuration.md) | 配对、RTSP/ONVIF 配置、热更新、授权码轮换和完整验证 |
| [operations.md](operations.md) | 运行边界、录像位置、安全要求和故障定位 |
| [camera-support.md](camera-support.md) | 品牌预设、电脑摄像头和 Client RTSP 协议转换 |
| [mobile.md](mobile.md) | Android/iOS 原生采集、构建和操作 |
| [camera-validation.md](camera-validation.md) | 本次摄像头扩展的验证结果与实机待验范围 |
| [releases/](releases/) | 各已发布版本的变更记录 |
| [unsafe-audit.md](unsafe-audit.md) | 当前依赖选择、unsafe 边界和验证 |

客户端管理摄像头侧状态，线协议固定为
`xcos-edge-v1`；一份本地安装可以保存多个已配对实例，但每个实例只对应一台摄像机。
