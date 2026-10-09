# xcoc

`xcoc` `1.0.0` 是 xcos 的摄像头边缘客户端。它在摄像头所在网络中保存 RTSP/ONVIF 凭据、探测视频流、向 Server 发布主/子码流，并按配置在 Client 或 Server 侧录像。

`1.0.0` 提供桌面 CLI、移动端摄像头采集和配对服务，统一使用公共基础库 1.0.0、摄像头 edge v1 和 ABI v1。安装、重新配对及运维步骤见平台部署文档。

Client 提供 RTSP/ONVIF、海康/大华/宇视/Axis/Reolink/Tapo 码流预设，以及 Linux/Windows/macOS
内置与 USB 摄像头采集。Android/iOS 原生应用支持前后摄像头，将原生 H.264 转换成 Server 已支持的
RTSP/RTSPS 发布输入；所有适配与转换均在 Client，Server 协议保持现有 edge v1。
桌面运行环境必须提供 `ffmpeg` 与 `ffprobe`，本地摄像头还需要对应采集 backend 和 `libx264`；
手机应用使用系统采集和编码，无需 FFmpeg。详见[摄像头兼容与协议转换](docs/camera-support.md)和[移动端指南](docs/mobile.md)。

Linux Release 压缩包解压后运行 `sudo ./packaging/linux/install.sh`：安装全部功能并立即启动 systemd 服务。唯一提示是是否开机自启，直接回车默认 Yes；即使选择 No，本次安装仍会立即启动服务。Windows MSI 安装全部功能并立即启动 `XcocClient` 系统服务；运行 `xcoc setup --interactive` 时，开机自启提示直接回车也默认 Yes。非交互配对默认启用开机自启。安装和卸载都会保留现有配对配置与录像。

## 配置概览

先检查媒体工具并通过受保护的 stdin 完成实例配对：

```sh
ffmpeg -version
ffprobe -version
sudo install -m 0600 /dev/null /root/xcoc-bootstrap.json
sudoedit /root/xcoc-bootstrap.json
sudo sh -c 'exec xcoc setup --input-stdin < /root/xcoc-bootstrap.json'
sudo xcoc status
```

再发现摄像头，使用配对结果中的实例 ID 写入摄像头配置并核对：

```sh
sudo xcoc camera discover --timeout-seconds 3
sudo install -m 0600 config/camera.json.example /root/xcoc-camera.json
sudoedit /root/xcoc-camera.json
sudo sh -c 'exec xcoc camera apply --instance-id INSTANCE_UUID --input-stdin < /root/xcoc-camera.json'
sudo xcoc camera list
systemctl status xcoc.service
```

Bootstrap JSON、RTSP/ONVIF 样例、授权码轮换、热更新和故障定位见[完整配置指南](docs/configuration.md)。不要把摄像头密码、长期授权码或 Client token 放进命令参数和日志。

使用 `setup --interactive` 首次或替换配对时，实例授权码按普通文本输入并在终端中明文显示，不提供遮罩或
隐藏切换；摄像头密码仍使用隐藏输入。CLI 不会把授权码或摄像头密码写入自身日志、结果输出或启动参数。FFmpeg 子进程命令行包含摄像头 RTSP 凭据和发布地址中的媒体 JWT，本机可查看进程参数的用户可能读到它们；服务端不会收到摄像头 URL。

升级时会分别识别配对账户、摄像头配置和本地录像：不兼容账户要求显式重新配对并归档原文件；配置错误和无法识别的录像数据会明确报错且保持原样。

## 开发验证

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
- [摄像头兼容与协议转换](docs/camera-support.md)
- [Android/iOS 原生客户端](docs/mobile.md)
- [发行记录](docs/releases/)

代码采用 [Apache License 2.0](LICENSE-APACHE)。

## 仓库布局

根 Rust 包提供桌面 CLI 与共享摄像头/移动媒体库，`crates/mobile-ffi` 提供 Foundation v1 原生桥接，
`clients/android` 和 `clients/ios` 保存原生摄像头应用。`Cargo.lock` 固定 workspace 编译输入。
`src/main/tests.rs` 和 `src/onvif/tests.rs` 验证 Client 生命周期与 ONVIF；根 `tests/` 保存独立验收。
`protocol/` 保存固定 Server Source 的受控契约，`config/` 保存无凭据样例，`packaging/` 保存桌面安装器，
`scripts/` 提供移动库构建，`docs/` 描述配置与运行。

当前发布版本：**1.0.0**。参见 [1.0.0 发布说明](docs/releases/1.0.0.md)和[项目命名](docs/naming.md)。
