# Sentinel Monitor Client

`sentinel-client` `0.3.9` 是 Sentinel Monitor 的摄像头边缘客户端。它在摄像头所在网络中保存 RTSP/ONVIF 凭据、探测视频流、向 Server 发布主/子码流，并按配置在 Client 或 Server 侧录像。

当前提供 `rtsp` 与 `onvif` 两种适配器，支持 Linux、Windows 和 macOS 构建。运行环境必须提供同一发行套件中的 `ffmpeg` 与 `ffprobe`；原生服务和正式安装包以对应 Release 为准。

## 配置概览

先检查媒体工具并通过受保护的 stdin 完成实例配对：

```sh
ffmpeg -version
ffprobe -version
sudo install -m 0600 /dev/null /root/sentinel-bootstrap.json
sudoedit /root/sentinel-bootstrap.json
sudo sh -c 'exec sentinel-client setup --input-stdin < /root/sentinel-bootstrap.json'
sudo sentinel-client status
```

再发现摄像头，使用配对结果中的实例 ID 写入摄像头配置并核对：

```sh
sudo sentinel-client camera discover --timeout-seconds 3
sudo install -m 0600 config/camera.json.example /root/sentinel-camera.json
sudoedit /root/sentinel-camera.json
sudo sh -c 'exec sentinel-client camera apply --instance-id INSTANCE_UUID --input-stdin < /root/sentinel-camera.json'
sudo sentinel-client camera list
sudo sentinel-client run
```

Bootstrap JSON、RTSP/ONVIF 样例、授权码轮换、热更新和故障定位见[完整配置指南](docs/configuration.md)。不要把摄像头密码、长期授权码或 Client token 放进命令参数和日志。

使用 `setup --interactive` 首次或替换配对时，实例授权码按普通文本输入并在终端中明文显示，不提供遮罩或
隐藏切换；摄像头密码仍使用隐藏输入。两者都不会写入日志或命令参数。

升级时会分别识别配对账户、摄像头配置和本地录像：不兼容账户要求显式重新配对并归档原文件；配置错误和无法识别的录像数据会明确报错且保持原样。

## 开发验证

```sh
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --locked --all-targets -- -D warnings
cargo +1.98.0 test --locked
```

## 文档

- [文档总览](docs/README.md)
- [完整配置指南](docs/configuration.md)
- [运行与故障定位](docs/operations.md)
- [发行记录](docs/releases/)

代码采用 [Apache License 2.0](LICENSE-APACHE)。
