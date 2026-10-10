# 摄像头兼容与协议转换

摄像头接入与协议转换全部在客户端实现。网络摄像头继续使用 RTSP/ONVIF；电脑与手机摄像头在客户端
采集、编码为 H.264，再通过服务端已有的 RTSPS 发布授权接入 MediaMTX。转换后的快照
`adapter_kind` 为 `rtsp`，协议继续为 `xcos-edge-v1`，服务端不需要增加厂商或平台适配器。

## 常用网络摄像头

运行 `xcoc camera presets` 查看品牌、型号系列示例和官方文档链接；该命令无需配对。
`preset` 自动生成主/子码流路径，用户名和密码仍只留在受保护的客户端配置中。

| preset | 典型系列/型号 | 主码流路径（channel=1） | 子码流路径 |
|---|---|---|---|
| `hikvision` | DS-2CD、DS-2DE 的 RTSP 型号 | `/Streaming/Channels/101` | `/Streaming/Channels/102` |
| `dahua` | IPC-HFW、IPC-HDW 的 RTSP 型号 | `/cam/realmonitor?channel=1&subtype=0` | `subtype=1` |
| `uniview` | 宇视普通 IPC | `/media/video1` | `/media/video2` |
| `axis` | AXIS OS 网络摄像头 | `/axis-media/media.amp?camera=1&videocodec=h264` | 同端点，请求 `resolution=640x360` |
| `reolink` | 开启 RTSP 的 RLC、E1 型号 | `/Preview_01_main` | `/Preview_01_sub` |
| `tapo` | C100、C200、C210、C310 等有线供电 RTSP 型号 | `/stream1` | `/stream2` |

这些是厂商文档中的协议系列预设，不表示已逐台完成实机认证。实际型号必须开放 RTSP 服务，电池、
云端专用或部分多镜头机型可能使用其他路径或不开放该协议。Tapo 使用相机账户，区别于 TP-Link 云账户。
Reolink 需在设备设置开启对应服务。普通宇视 IPC、Tapo 预设只接受 channel=1；多镜头或 NVR 的特殊
通道使用原有 `rtsp` 配置明确填写地址。没有子码流时设置 `sub_stream: false`。Axis 子码流是请求较低
分辨率，仍需设备支持该分辨率。需要 PTZ 时使用原有 `onvif` 适配器，由设备的 ONVIF Profile 确认能力。

参考 `config/camera-preset.json.example`，`preset` 可替换为表中任意值。`host` 只填写主机名/IP，IPv6
使用 `[2001:db8::10]`；端口独立填写，凭据使用 `username`/`password`，不要混入 host。
应用方式与原有摄像头一致：

```sh
xcoc camera apply --instance-id INSTANCE_UUID --input-stdin < /protected/camera.json
```

桌面网络摄像头的探测、发布和录像使用链接的 FFmpeg 库，既不调用 `ffprobe`，也不把网络 URL 传给
外部 `ffmpeg` 命令。发布和录像按输入码流复制，保留目标容器/协议支持的视频与音频，包括 H.264、
HEVC 和兼容音频；不会为隐藏凭据强制改成仅 H.264 或丢弃音轨。不支持的码流组合明确失败，实际播放
能力仍取决于服务端和播放器。运行时依赖见[桌面媒体运行时](media-worker.md)。

## 电脑内置与 USB 摄像头

`xcoc camera devices` 枚举当前系统摄像头，无需配对。Linux 返回 `/dev/videoN`，Windows
返回 DirectShow 设备列表，macOS 返回 AVFoundation 视频设备索引。按结果修改对应模板：

- Linux：`config/camera-local-linux.json.example`，`backend: v4l2`。
- Windows：`config/camera-local-windows.json.example`，`backend: dshow`，device 填完整视频设备名。
- macOS：`config/camera-local-macos.json.example`，`backend: avfoundation`，device 填视频设备索引。

填写摄像头支持的宽度、高度和帧率，应用配置后运行 `xcoc run`。宽高必须为偶数，帧率为
1–60。FFmpeg 必须包含对应采集后端与 `libx264` 编码器。系统必须授予运行用户访问摄像头的权限。
Linux 服务用户需要设备权限；Windows 系统服务不能保证访问交互用户摄像头，内置摄像头应在已授权
的用户会话中运行客户端；macOS 应允许终端/运行程序访问摄像头。已经运行的后台客户端占有单实例锁时，
先停止该实例再在用户会话运行，避免两个客户端争用同一配置。

每台本地摄像头只有一个外部 `ffmpeg` 采集与编码进程，其参数只含设备标识、受控采集选项和
`pipe:1` 输出；视频通过匿名 stdout 管道进入客户端。私有随机路径的回环 HTTP 转发器将视频分发到
独立的内置媒体工作进程，固定缓冲预算，慢消费者会断开并由运行循环重建。relay 地址通过工作进程的
匿名 stdin 管道传递，不进入命令参数、环境变量、快照或日志；摄像头视频不在局域网监听。
仍须保护本机账户和进程内存访问。设置 `storage_mode: client` 即启用现有本地 MP4 分段录像；服务端
连接中断时录制继续。配置热更新、禁用、移除或退出客户端都会释放采集资源。

电脑摄像头当前提供一个 H.264 视频主码流，不附带麦克风、子码流、PTZ 或事件能力。

## Android / iOS 手机与平板

原生应用源码、构建与使用见 [移动端指南](mobile.md)。Android 使用 Camera2 + MediaCodec，iOS 使用
AVFoundation + VideoToolbox；共享 Rust 模块将原生 H.264 转换为 RTP，并通过 RTSPS 的 ANNOUNCE /
SETUP / RECORD 发布到已有服务端。无需 FFmpeg、额外网关或修改服务端配置协议。

移动端支持前后摄像头选择、实例配对、开始/停止、周期快照、断线后重建发布与有界帧队列。
Android 使用有常驻通知和停止按钮的摄像头前台服务；必须从可见界面取得权限并启动。
iOS 当前只在前台采集，进入后台立即停止；回来后可手动重新启动。移动端当前使用服务端录像，
不提供本地录像、音频、子码流、PTZ。能力快照明确报告 unsupported，而不会假装支持 ONVIF 控制。

## 依据与验收

路径预设依据官方资料：[海康](https://supportusa.hikvision.com/a/solutions/articles/17000129064)、
[大华](https://www.dahuasecurity.com/asset/upload/uploads/soft/20191107/4-Dahua-Network-Camera-Web-3.0-Operation-Manual_V2.0.11.pdf)、
[宇视](https://www.uniview.com/res/202310/26/20231026_1890310_How%20to%20Get%20a%20Uniview%20Camera%27s%20RTSP%20Stream_974039_168459_0.pdf)、
[Axis](https://developer.axis.com/video-streaming-and-recording/video-streaming/reference/rtsp-endpoints/)、
[Reolink](https://support.reolink.com/articles/900000630706-Introduction-to-RTSP/)、
[Tapo](https://www.tp-link.com/us/support/faq/2680/)。桌面采集选项依据 [FFmpeg 设备文档](https://ffmpeg.org/ffmpeg-devices.html)，
移动端 RTP 封包依据 [RFC 6184](https://datatracker.ietf.org/doc/html/rfc6184)。

接入后的完整验收：服务端显示 RTSP 设备和实际码流尺寸，浏览器播放有画面；按所选位置产生录像；
断开网络后重新接通能恢复发布；停止/禁用后系统摄像头占用结束。实机摄像头、系统权限和编码器能力
仍需分别在目标设备验收，模拟器构建通过不代表实机摄像头完成认证。
