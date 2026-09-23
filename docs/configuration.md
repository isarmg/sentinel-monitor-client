# Sentinel Monitor Client 配置指南

本文适用于 `sentinel-client` `0.3.8`，按实际 CLI 说明实例配对、RTSP/ONVIF 摄像头配置、更新和验证。

## 1. 前置条件

Client 运行环境必须能找到同一发行套件中的 FFmpeg 和 FFprobe：

```sh
ffmpeg -version
ffprobe -version
sentinel-client --version
```

默认配置路径：

| 平台 | 配置文件 |
|---|---|
| Linux | `/etc/isarmg/sentinel-client/config.json` |
| macOS | `/Library/Application Support/SentinelClient/config.json` |
| Windows | `%ProgramData%\SentinelClient\config.json` |

Windows Release 同时提供 MSI。交互安装的自定义页允许修改程序目录，并分别选择是否保留上述配置/配对信息、是否保留
`%ProgramData%\SentinelClient\recordings`；两项默认保留，取消选择会在安全检查后永久清理对应类别。安装结束会停留在明确的完成页或失败页。自定义安装路径可从
`HKLM\Software\sarmg\Sentinel Client` 的 `InstallLocation` 读取。

其他路径可用全局参数指定：

```sh
sentinel-client --config /absolute/path/config.json status
```

配置包含长期 Client token、摄像头 URL 和可选密码，只允许管理员及服务账号读取。不要直接编辑其身份、format 或实例 ID。

升级会区分账户配置和重要录像数据。旧版、损坏或未知的账户/配对文档返回
`pairing_state_incompatible`；创建新的 Server 授权码后，显式运行
`sentinel-client setup --interactive --replace`，Client 会先原样归档旧文档，再提交新配对。当前格式中的摄像头配置错误返回
`configuration_state_incompatible`，不会被 `--replace` 当作账户数据清除。本地录像目录如果包含旧布局、链接、非 MP4 文件或不可读内容，返回
`important_state_incompatible` 并保留全部内容，必须先用兼容版本导出或由管理员核实后处理。

## 2. 创建实例并配对

先在 Sentinel Server 管理页创建 Client 实例并复制 36 位小写英文字母数字授权码。Bootstrap 文件只接受以下字段：

```json
{
  "server": "https://sentinel.example.com",
  "authorization_code": "REPLACE_WITH_INSTANCE_AUTHORIZATION_CODE",
  "name": "camera-edge-01"
}
```

`server` 必须是 HTTPS 根地址，可以包含端口；路径（如 `/admin`、`/api/v2`）、URL 内的用户名/密码、查询参数和片段都会被拒绝。Debug 构建还允许 `localhost`、`127.0.0.1` 和 `[::1]` 的 HTTP 根地址用于本机验证。

Linux 上创建受保护文件并通过 stdin 提交：

```sh
sudo install -m 0600 config/bootstrap.json.example /root/sentinel-bootstrap.json
sudoedit /root/sentinel-bootstrap.json
sudo sh -c 'exec sentinel-client setup --input-stdin < /root/sentinel-bootstrap.json'
sudo sentinel-client status
sudo shred -u /root/sentinel-bootstrap.json
```

也可使用交互流程。它会检查 FFmpeg/FFprobe、完成配对，并继续引导发现或录入摄像头：

```sh
sudo sentinel-client setup --interactive
```

首次配对和 `setup --interactive --replace` 使用同一个 `Authorization code (visible)` 普通文本提示。输入或
粘贴的授权码会在终端中明文回显，不提供遮罩、隐藏切换或特殊显示流程；随后配置摄像头时，摄像头密码仍
使用隐藏输入。CLI 不会把授权码或摄像头密码写入日志、结果输出或命令参数。

保存输出中的实例 UUID。一次安装可以保存多个实例，但每个实例只对应一台摄像机。

## 3. RTSP 摄像头

复制模板并填写主/子码流。密码只放在受保护文件中：

```sh
sudo install -m 0600 config/camera.json.example /root/sentinel-camera.json
sudoedit /root/sentinel-camera.json
```

格式如下：

```json
{
  "name": "front-door",
  "location": "entrance",
  "manufacturer": "Generic",
  "model": "RTSP camera",
  "enabled": true,
  "storage_mode": "server",
  "adapter": {
    "kind": "rtsp",
    "streams": [
      {"profile": "main", "url": "rtsp://192.0.2.10/main"},
      {"profile": "sub", "url": "rtsp://192.0.2.10/sub"}
    ],
    "username": "camera-user",
    "password": "REPLACE_WITH_CAMERA_PASSWORD"
  }
}
```

应用并核对配置：

```sh
sudo sh -c 'exec sentinel-client camera apply --instance-id INSTANCE_UUID --input-stdin < /root/sentinel-camera.json'
sudo sentinel-client camera list
sudo shred -u /root/sentinel-camera.json
```

`storage_mode` 为 `server` 时由 Server 录像；设为 `client` 时，Client 在本机状态目录写入 15 分钟 MP4 分段，历史录像不会自动上传 Server。

## 4. ONVIF 摄像头

先发现同一网络中的 ONVIF 设备：

```sh
sudo sentinel-client camera discover --timeout-seconds 3
```

复制并编辑 ONVIF 模板：

```sh
sudo install -m 0600 config/camera-onvif.json.example /root/sentinel-camera-onvif.json
sudoedit /root/sentinel-camera-onvif.json
```

```json
{
  "name": "warehouse-ptz",
  "location": "warehouse",
  "enabled": true,
  "storage_mode": "client",
  "adapter": {
    "kind": "onvif",
    "device_service_url": "http://192.0.2.20/onvif/device_service",
    "username": "camera-user",
    "password": "REPLACE_WITH_CAMERA_PASSWORD",
    "main_profile_token": null,
    "sub_profile_token": null
  }
}
```

```sh
sudo sh -c 'exec sentinel-client camera apply --instance-id INSTANCE_UUID --input-stdin < /root/sentinel-camera-onvif.json'
sudo sentinel-client camera list
sudo shred -u /root/sentinel-camera-onvif.json
```

profile token 为空时由适配器选择媒体 Profile。发现为空时检查 Client 与摄像头间的路由、防火墙和 UDP 3702 组播；也可以直接填写设备服务 URL。

## 5. 启动与完整验证

前台启动用于首次验证：

```sh
sudo sentinel-client run
```

正式运行请使用 Release 为当前平台安装的原生服务，不要自行假设服务名。配置热更新时重新执行 `camera apply`，运行中的 Client 会读取新 revision 并重建对应 FFmpeg 进程，无需重启整个 Client。

完整成功需要同时确认：

1. `sentinel-client status` 显示实例已配对且摄像头已配置；
2. `sentinel-client camera list` 显示正确的实例、名称、适配器和录像位置；
3. Server 管理页收到新快照，并能播放主码流；
4. `client` 模式下本地 MP4 分段持续增长，或 `server` 模式下 Server 录像索引持续出现。

## 6. 修改、删除与轮换授权码

修改摄像头时编辑新的受保护 JSON，并对同一实例再次执行 `camera apply`。删除本地摄像头配置：

```sh
sudo sentinel-client camera remove INSTANCE_UUID
sudo sentinel-client camera list
```

删除只移除该实例的本地摄像头配置，不代表删除 Server 实例或历史录像。

Server 管理员轮换实例授权码后，创建新的 Bootstrap 文件并显式替换配对凭据：

```sh
sudo sh -c 'exec sentinel-client setup --input-stdin --replace < /root/sentinel-bootstrap.json'
sudo sentinel-client status
```

替换会保留该实例已保存的摄像头配置。不要通过删除整个配置文件轮换凭据。

## 7. 安全与排障

- 生产 Client 只接受可信 HTTPS Server；Server 返回的发布 URL 必须是证书受系统信任且名称匹配的 `rtsps://` 地址。
- 摄像头 RTSP/ONVIF 地址和凭据只留在 Client，不写入日志、工单或 Server 配置。
- 配置已保存只证明本地原子提交成功；还需等待 `run` 应用新 revision，并等待 Server 收到下一份快照。
- Server 暂时离线不会停止 `client` 模式本地录像；恢复连接后再核对快照和发布状态。
- 配对已成功但摄像头探测失败时，使用 `camera discover` 和 `camera apply` 继续，不要重新创建实例。

更多故障现象与核对顺序见[运维文档](operations.md)。
