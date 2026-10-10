# xcoc

`xcoc` `1.0.0` 是 xcos 的摄像头边缘客户端。它在摄像头所在网络中保存 RTSP/ONVIF 凭据、探测视频流、向服务端发布主/子码流，并按配置在客户端或服务端侧录像。

`1.0.0` 提供桌面 CLI、移动端摄像头采集和配对服务，统一使用公共基础库 1.0.0、摄像头 edge v1 和 ABI v1。安装、重新配对及运维步骤见平台部署文档。

客户端提供 RTSP/ONVIF、海康/大华/宇视/Axis/Reolink/Tapo 码流预设，以及 Linux/Windows/macOS
内置与 USB 摄像头采集。Android/iOS 原生应用支持前后摄像头，将原生 H.264 转换成服务端已支持的
RTSP/RTSPS 发布输入；所有适配与转换均在客户端，服务端协议保持现有 edge v1。
桌面媒体探测、发布与录像通过 `xcoc` 内置工作进程调用 FFmpeg 库，不依赖 `ffprobe` 命令。
内置/USB 摄像头另需提供带对应采集后端和 `libx264` 的 `ffmpeg` 命令；手机应用使用系统采集和编码，无需 FFmpeg。
构建与运行依赖见[桌面媒体运行时](docs/media-worker.md)，接入边界见[摄像头兼容与协议转换](docs/camera-support.md)和[移动端指南](docs/mobile.md)。

Linux Release 压缩包解压后运行 `sudo ./packaging/linux/install.sh`：安装全部功能并立即启动 systemd 服务。唯一提示是是否开机自启，直接回车默认 Yes；即使选择 No，本次安装仍会立即启动服务。Windows MSI 安装全部功能并立即启动 `XcocClient` 系统服务；运行 `xcoc setup --interactive` 时，开机自启提示直接回车也默认 Yes。非交互配对默认启用开机自启。安装和卸载都会保留现有配对配置与录像。

## 配置概览

先检查内置媒体运行时，再通过受保护的 stdin 完成实例配对：

```sh
xcoc media-worker --check
# 仅内置/USB 摄像头需要外部采集工具：
ffmpeg -version
sudo install -m 0600 /dev/null /root/xcoc-bootstrap.json
sudoedit /root/xcoc-bootstrap.json
sudo sh -c 'exec xcoc setup --input-stdin < /root/xcoc-bootstrap.json'
sudo xcoc status
```

再发现摄像头，使用配对结果中的实例 ID 写入摄像头配置并核对。下方复制模板的命令仅适用于源码仓库根目录；公开 Release 压缩包不含 `config/`。从发行包安装时，先用 `sudo install -m 0600 /dev/null /root/xcoc-camera.json` 创建受保护文件，再按[配置指南中的 RTSP JSON](docs/configuration.md#3-rtsp-摄像头)填写，随后执行同一 `camera apply` 命令：

```sh
sudo xcoc camera discover --timeout-seconds 3
sudo install -m 0600 config/camera.json.example /root/xcoc-camera.json
sudoedit /root/xcoc-camera.json
sudo sh -c 'exec xcoc camera apply --instance-id INSTANCE_UUID --input-stdin < /root/xcoc-camera.json'
sudo xcoc camera list
systemctl status xcoc.service
```

Bootstrap JSON、RTSP/ONVIF 样例、授权码轮换、热更新和故障定位见[完整配置指南](docs/configuration.md)。不要把摄像头密码、长期授权码或客户端 token 放进命令参数和日志。

使用 `setup --interactive` 首次或替换配对时，实例授权码按普通文本输入并在终端中明文显示，不提供遮罩或
隐藏切换；摄像头密码仍使用隐藏输入。摄像头 RTSP 凭据、发布地址中的媒体 JWT 与私有回环地址通过有界匿名管道传给内置媒体工作进程，不进入子进程命令参数、环境变量或临时凭据文件；原始媒体库日志不进入服务日志。服务端不会收到摄像头 URL。该设计不隔离拥有管理员权限或进程调试/内存读取权限的本机用户，仍须保护服务账户与配置。

升级时会分别识别配对账户、摄像头配置和本地录像：不兼容账户要求显式重新配对并归档原文件；配置错误和无法识别的录像数据会明确报错且保持原样。

## 开发验证

先按[桌面媒体运行时构建说明](docs/media-worker.md)准备原生 FFmpeg 库、C 编译器与链接依赖；Android/iOS 目标不链接这些桌面库。

```sh
cargo +1.99.0 fmt --all -- --check
cargo +1.99.0 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.99.0 test --locked --workspace --all-targets --all-features
```

## 文档

- [文档总览](docs/README.md)
- [分平台部署、重新配对、启停与卸载](docs/platform-setup.md)
- [完整配置指南](docs/configuration.md)
- [运行与故障定位](docs/operations.md)
- [桌面媒体运行时与构建依赖](docs/media-worker.md)
- [摄像头兼容与协议转换](docs/camera-support.md)
- [Android/iOS 原生客户端](docs/mobile.md)
- [发行记录](docs/releases/)

代码采用 [Apache License 2.0](LICENSE-APACHE)。

## 仓库布局

根 Rust 包提供桌面 CLI 与共享摄像头/移动媒体库，`crates/mobile-ffi` 提供 xcsc v1 原生桥接，
`clients/android` 和 `clients/ios` 保存原生摄像头应用。`Cargo.lock` 固定工作区编译输入。
`src/main/tests.rs` 和 `src/onvif/tests.rs` 验证客户端生命周期与 ONVIF；根 `tests/` 保存独立验收。
`protocol/` 保存固定服务端源码的受控契约，`config/` 保存无凭据样例，`packaging/` 保存桌面安装器，
`scripts/` 提供移动库构建，`docs/` 描述配置与运行。

当前发布版本：**1.0.0**。参见 [1.0.0 发布说明](docs/releases/1.0.0.md)。

公共支撑的职责、单体依赖、平台边界与验证方法见[公共支撑说明](docs/common-support.md)。
