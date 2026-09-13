# Sentinel Monitor Client

`sentinel-client` 是多品牌摄像头边缘管理客户端。品牌和设备差异由 Client 的适配器吸收，Server 只接收统一的
设备身份、能力、主/子码流、健康状态和命令结果。摄像头 RTSP/ONVIF 凭据仅保存在客户端受保护配置中；客户端与
Sentinel Server 配对后，把实时主/子码流发布到 Server 的 MediaMTX，因此两种录像策略都能在管理页查看实时画面。

当前提供两种适配器：`rtsp` 适用于已知主/子码流地址的设备；`onvif` 自动读取设备厂商、型号、媒体配置与
RTSP URI，并在设备声明 PTZ 能力时执行 Server 下发的统一 PTZ 命令。后续厂商私有 SDK 应实现同一个
`DeviceAdapter` 边界，不得把厂商密码、私有 DTO 或局域网地址上传 Server。

## 统一配对

Host、Sunshine 与 Sentinel Client 的自动化都只通过 stdin 提交 `{server,authorization_code,...}`，不把秘密放在
命令参数或日志中。每个客户端实例拥有一个长期授权码，由 Server 加密保存并可查看。Server 更换实例授权码时会
撤销现有客户端凭据；把新码写入 bootstrap JSON 后运行 `sentinel-client setup --input-stdin --replace` 重新配对。

```sh
sudo sentinel-client setup --input-stdin < bootstrap.json
sudo sentinel-client camera discover --timeout-seconds 3
sudo sentinel-client camera apply --input-stdin < camera.json
sudo sentinel-client run
```

Server 管理页查看实例授权码。正式版本只接受可信 HTTPS Server；debug 构建额外允许真实 loopback HTTP。
客户端依赖系统中的 `ffmpeg` 和同发行套件的 `ffprobe`；只有实际探测到视频流后才向 Server 报告设备在线并取得
发布授权。`storage_mode` 为 `server` 时 MediaMTX 在 Server 录像；为 `client` 时 Server
只接收实时流，Client 另存 15 分钟 MP4 分段到平台状态目录。客户端模式的历史录像不会上传 Server。

配置默认位于 Linux `/etc/isarmg/sentinel-client/config.json`、macOS
`/Library/Application Support/SentinelClient/config.json`、Windows `%ProgramData%\SentinelClient\config.json`。
配置包含摄像头和长期 Client 凭据，必须只允许服务账户读取。

## 验证

```sh
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --locked --all-targets -- -D warnings
cargo +1.98.0 test --locked
```
