# Sentinel Monitor Client 运维

## 1. 前置条件与状态文件

当前版本为 `0.3.6`，需要 `ffmpeg` 与同套发行中的 `ffprobe` 均可从后台服务的 `PATH` 找到。默认配置路径为：

- Linux：`/etc/isarmg/sentinel-client/config.json`
- macOS：`/Library/Application Support/SentinelClient/config.json`
- Windows：`%ProgramData%\SentinelClient\config.json`

可用全局 `--config PATH` 指向其他文件。配置包含长期 Client token、摄像头地址和可选账号密码，必须限制为服务
账户可读；不要把配置、stdin bootstrap JSON 或带凭据的 RTSP URL 写入日志、命令参数或工单。

## 2. 配对与摄像头配置

交互式流程会先检查媒体工具，再完成配对、发现/录入摄像头并用真实 `ffprobe` 验证码流：

```sh
sudo sentinel-client setup --interactive
```

自动化配对从 stdin 读取严格 JSON，不接受未知字段：

```json
{
  "server": "https://sentinel.example.com",
  "authorization_code": "64 位小写十六进制授权码",
  "name": "camera-edge-01"
}
```

```sh
sudo sentinel-client setup --input-stdin < bootstrap.json
```

同一 Server 实例已存在时，只有管理员先更换授权码后，才能用 `setup --input-stdin --replace` 替换该实例的访问
凭据；已保存的摄像头配置会保留。配对已提交但摄像头配置失败时，不要重新创建实例，使用输出的实例 ID 继续：

```sh
sudo sentinel-client camera discover --timeout-seconds 3
sudo sentinel-client camera apply --instance-id INSTANCE_UUID --input-stdin < camera.json
```

手工 RTSP 配置示例：

```json
{
  "name": "东门",
  "location": "一层",
  "enabled": true,
  "storage_mode": "server",
  "adapter": {
    "kind": "rtsp",
    "streams": [
      {"profile": "main", "url": "rtsp://camera.example/main"},
      {"profile": "sub", "url": "rtsp://camera.example/sub"}
    ],
    "username": "operator",
    "password": "REDACTED"
  }
}
```

`id` 可省略；`camera apply` 会强制使用 `--instance-id`，避免一份配置越权绑定到另一实例。ONVIF 配置使用
`kind: "onvif"`、`device_service_url`，并可提供 `username`、`password`、`main_profile_token` 和
`sub_profile_token`。

## 3. 运行与核验

```sh
sudo sentinel-client status
sudo sentinel-client camera list
sudo sentinel-client run
```

`status` 只输出安装 ID、Server、实例 ID、名称和是否已配置，不输出 token 或摄像头凭据。`run` 每两秒重新读取
配置：`camera apply` 或 `camera remove` 后无需重启，受影响的 FFmpeg 子进程会停止并按新配置重建。

应分别核验：

1. Client 能探测主码流，Server 能看到最新快照；
2. 管理页可播放实时画面；
3. `storage_mode: "client"` 时本地 15 分钟 MP4 分段持续生成；
4. `storage_mode: "server"` 时 Server 录像索引持续出现。

Server 暂时离线不应阻断本地录像；发布授权和快照会在连接恢复后重新获取。生产构建只接受可信 HTTPS Server，
Server 返回的发布地址必须是证书受系统信任且主机名匹配的 `rtsps://` URL。

## 4. 常见故障

| 现象 | 核对项 |
|---|---|
| Setup 在请求前失败 | 后台服务 `PATH` 中的 `ffmpeg -version`、`ffprobe -version` |
| `pairing_authorization_rejected` | 实例授权码是否有效、是否已配对、是否应先在 Server 更换授权码 |
| `pairing_protocol_unsupported` | Client 与 Server 是否都支持 `sentinel-edge-v3` |
| ONVIF 发现为空 | Client 是否与摄像头同一可达网段，UDP 3702 组播是否被网络策略拦截 |
| 配置已保存但画面离线 | 摄像头 URL/凭据、FFprobe 探测、Server RTSPS 证书和 Client 到发布端口的连通性 |
| 修改后 Server 尚未更新 | `run` 是否仍在运行；等待下一次快照并检查该实例错误输出 |
| `pairing_state_incompatible` | 旧账户文件会保留；创建新授权码后运行 `setup --interactive --replace` 归档旧文件并重新配对 |
| `configuration_state_incompatible` | 当前摄像头配置不合法；文件已保留，不会被账户恢复流程清除 |
| `important_state_incompatible` | 本地录像布局、文件类型或可读性不兼容；录像已保留，禁止直接覆盖或自动迁移 |

不要通过手改本地 JSON 的 `format`、`installation_id` 或实例 ID 修复状态；运行时检测到安装身份变化会要求重启，
错误绑定可能使实例无法重新配对。修改摄像头应使用 `camera apply/remove` 的原子写入路径。
