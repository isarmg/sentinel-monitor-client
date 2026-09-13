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
sudo sentinel-client setup --interactive
# 自动化仍可使用：sudo sentinel-client setup --input-stdin < bootstrap.json
sudo sentinel-client camera discover --timeout-seconds 3
sudo sentinel-client camera apply --input-stdin < camera.json
sudo sentinel-client run
```

交互式 Setup 会先用后台进程相同的 `PATH` 检查 FFmpeg/FFprobe，再配对、执行 ONVIF 发现（也可输入
手工 RTSP 主/子码流）、保护式读取摄像头密码、选择 `server`/`client` 录像位置，并在保存前用真实
FFprobe 探测视频流。配对已提交而摄像头探测失败时会明确返回失败并保留身份，之后从
`camera discover/apply` 继续，不会要求重新配对。

`run` 会在每个协调周期重新读取经过原子替换的配置。运行中执行 `camera apply/remove` 无需重启 Client；
被删除或改变的摄像头会立即停止对应发布和本地录像进程，并在下一份快照中更新 Server。删除最后一台
摄像头后 Client 仍保持配对心跳并发送空列表，之后可直接重新添加设备。

摄像头修改的结果分三层确认：命令成功只证明新配置已原子保存；本地 `run` 读取新 revision 后才证明旧
FFmpeg 发布/录像进程已停止或新进程已启动；Server 接收下一份快照后才证明远端摄像头集合已更新。Server
离线时，配置和本地进程仍按新 revision 收敛，本地录像继续工作，远端同步保持待确认并在恢复连接后完成。
无效配置在保存前拒绝，不改变现有 revision 或进程。

Server 管理页查看实例授权码。正式版本只接受可信 HTTPS Server；debug 构建额外允许真实 loopback HTTP。
Server 发布授权必须是 `rtsps://`，FFmpeg 会启用对端证书验证；证书链必须受 Client 主机信任且与发布域名匹配。
发布 URL 使用短期、单摄像头/单 profile/publish 限定的媒体 Token，不包含长期 Client API 凭据。本地摄像头输入仍可在隔离的摄像头网络使用 RTSP。
客户端依赖系统中的 `ffmpeg` 和同发行套件的 `ffprobe`；只有实际探测到视频流后才向 Server 报告设备在线并取得
发布授权。`storage_mode` 为 `server` 时 MediaMTX 在 Server 录像；为 `client` 时 Server
只接收实时流，Client 另存 15 分钟 MP4 分段到平台状态目录。客户端模式的历史录像不会上传 Server。

配置默认位于 Linux `/etc/isarmg/sentinel-client/config.json`、macOS
`/Library/Application Support/SentinelClient/config.json`、Windows `%ProgramData%\SentinelClient\config.json`。
配置包含摄像头和长期 Client 凭据，必须只允许服务账户读取。

当前源码在 Linux、Windows 和 macOS 构建；正式安装包与原生服务生命周期仍以对应 Release 的 CI 资产为准。
判断业务成功不能只看进程运行：至少应在 Client 观察到摄像头探测成功、在 Server 看到当前快照，并从管理页
实际播放主码流；`client` 存储模式还应确认本地 MP4 分段继续增长，`server` 模式则确认 Server 录像索引出现。

## 验证

```sh
cargo +1.98.0 fmt --all -- --check
cargo +1.98.0 clippy --locked --all-targets -- -D warnings
cargo +1.98.0 test --locked
```
