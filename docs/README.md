# xcoc 文档

xcoc 在摄像头所在网络采集视频，并向 xcos 发布码流。先选择平台，再完成配对和画面验证。

当前源码准备的是 **1.1.0**。截至 2026-10-10，公开 GitHub Release 仍为 **v1.0.0，仅含桌面资产**；内置媒体工作进程和移动 Release 产物应从对应源码构建。下方 1.1.0 产物名称说明构建输出，不表示这些文件已经发布。

## 开始使用

1. [选择平台并安装](platform-setup.md)：Linux、Windows、macOS、Android 和 iOS。
2. [配对与摄像头配置](configuration.md)：交互配对、RTSP/ONVIF、录像位置和设置更新。
3. [日常使用](usage.md)：确认画面、管理实例、改配置与轮换授权码。
4. [运行维护与排障](operations.md)：状态、日志、录像和常见错误。

## 开发与参考

- [支持的摄像头](camera-support.md)：品牌预设、USB/内置摄像头与手机采集。
- [桌面媒体运行时与构建](media-worker.md)：静态媒体库、平台依赖、许可证。
- [Android/iOS 构建与签名](mobile.md)：原生采集、Release 输出、实机安装要求。
- [开发与验证](development.md)、[状态与动作参考](runtime-reference.md)。
- [摄像头验证记录](camera-validation.md)、[公共支撑](common-support.md)、[安全审查](unsafe-audit.md)。
- [1.1.0 发布准备说明](releases/1.1.0.md)、[1.0.0 发行记录](releases/1.0.0.md)、[项目首页](../README.md)。

一份桌面安装可保存多个配对实例，每个实例对应一台摄像机。移动应用管理一份配对和一个主码流。设备协议为 `xcos-edge-v1`。
