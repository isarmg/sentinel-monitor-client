# xcoc 配置指南

首次部署或日常维护请先阅读[分平台全流程指南](platform-setup.md)：按本机平台完成安装、配对、重新配对、服务/后台任务查看与启停、诊断和卸载，命令旁均说明用途。本文详细说明配置字段和业务操作。

本文适用于 `xcoc` `1.1.0`，按实际 CLI 说明实例配对、RTSP/ONVIF 摄像头配置、更新和验证。

品牌码流预设、Windows/macOS/Linux 内置与 USB 摄像头见[摄像头兼容与协议转换](camera-support.md)；
Android/iOS 采集应用见[移动端指南](mobile.md)。这些输入统一转换成服务端已支持的 RTSP/RTSPS，
摄像头地址、平台采集选项与转换逻辑均由客户端管理。

## 命令用途与执行边界

| 命令 | 用途与影响 |
|---|---|
| `xcoc media-worker --check` | 无凭据检查内置媒体库是否可用；不连接摄像头或发布服务 |
| `ffmpeg -version` | 仅内置/USB 摄像头使用的采集/编码命令；后台采集也须能找到它 |
| `xcoc --version` | 查看本机客户端软件版本 |
| `setup --interactive` | 交互配对，继续摄像头发现/录入和真实媒体验证；Windows 还配置并启动服务 |
| `setup --input-stdin` | 从受保护 stdin JSON 配对；同实例新 token 可保留已有摄像头设置 |
| `setup --interactive --replace` | 仅在配对状态不兼容时归档旧账户并重新配对，不绕过配置或录像异常 |
| `status` | 查看本机安装/Server/实例和配置摘要，不输出 token |
| `camera presets` | 列出支持的品牌/型号预设 |
| `camera devices` | 枚举本机 USB/内置摄像头，不联网发现 ONVIF |
| `camera discover --timeout-seconds 3` | 最多等待 3 秒发现可达 ONVIF 设备，发现结果不等于码流验证成功 |
| `camera apply --instance-id UUID --input-stdin` | 从受保护 JSON 添加/替换指定实例的摄像头；常驻进程随后热更新 |
| `camera list` | 查看不含摄像头 URL/密码的本机摘要 |
| `camera remove UUID` | 移除指定实例的摄像头配置，配对保留；运行后提交空快照 |
| `unpair UUID` | 移除指定本机配对及其摄像头配置，保留录像/其他实例，不撤销服务端授权 |
| `run` | 常驻发布、录像和心跳；前台 Ctrl+C 停止，不能与同配置后台实例重复运行 |
| `service [--no-boot-start]`（Windows） | 用已保存配置设置并启动系统服务；该选项只把开机策略改为手动 |
| `logs --tail 100`（Windows） | 只读查询后台持久日志；Linux/macOS 使用对应系统日志入口 |

`--config` 选择配置文件。`UUID` 使用真实实例 ID；`--input-stdin` 是标准输入通道，不是把 JSON 放进命令参数。`sudo install -m 0600` 创建仅管理员可读写的输入文件，`sudoedit` 编辑它，`sudo sh -c 'exec ... < 文件'` 让提权后的进程打开受保护文件并读 stdin。`shred -u` 覆写并删除临时文件，在 SSD/快照文件系统上不保证介质安全擦除。常驻进程每两秒重读配置，修改后的摄像头需要核对服务端新快照。


## 1. 前置条件

桌面客户端使用链接的 FFmpeg 库完成探测、转发和分段录像，无需 `ffprobe` 命令。
先检查内置媒体运行时；内置/USB 摄像头另需 `ffmpeg` 命令及对应采集后端和 `libx264`：

```sh
xcoc --version
xcoc media-worker --check
# 仅内置/USB 摄像头：
ffmpeg -version
```

库版本、静态构建与平台要求见[桌面媒体运行时](media-worker.md)。`--check` 只检查运行时能力，不证明摄像头、证书或网络配置正确。

默认配置路径：

| 平台 | 配置文件 |
|---|---|
| Linux | `/etc/isarmg/xcoc/config.json` |
| macOS | `/Library/Application Support/XcocClient/config.json` |
| Windows | `%ProgramData%\XcocClient\config.json` |

使用全局 `--config /absolute/path/config.json` 选择其他位置。配置包含长期 token 和摄像头凭据，应只允许管理员与运行账户访问；自定义位置需预先创建受保护目录。具体安装、服务账户和权限见[平台指南](platform-setup.md)，持久状态校验见[状态参考](runtime-reference.md)。

## 2. 创建实例并配对

先在 xcos 管理页创建客户端实例并复制 36 位小写英文字母数字授权码。Bootstrap 文件只接受以下字段：

```json
{
  "server": "https://xcos.example.com",
  "authorization_code": "REPLACE_WITH_INSTANCE_AUTHORIZATION_CODE",
  "name": "camera-edge-01"
}
```

`server` 必须是 HTTPS 根地址，可以包含端口；路径（如 `/admin`、`/api/v1`）、URL 内的用户名/密码、查询参数和片段都会被拒绝。Debug 构建还允许 `localhost`、`127.0.0.1` 和 `[::1]` 的 HTTP 根地址用于本机验证。

下面所有复制 `config/*.json.example` 的命令仅在源码仓库根目录执行；公开 Release 归档不含 `config/`。从发行包安装时，将相应命令改为 `sudo install -m 0600 /dev/null 目标文件`，再用 `sudoedit` 把本文对应的完整 JSON 写入该文件并填写实际值。不要把空文件直接提交给 CLI。

Linux 上创建受保护的配对文件并通过 stdin 提交：

```sh
sudo install -m 0600 config/bootstrap.json.example /root/xcoc-bootstrap.json
sudoedit /root/xcoc-bootstrap.json
sudo sh -c 'exec xcoc setup --input-stdin < /root/xcoc-bootstrap.json'
sudo xcoc status
sudo shred -u /root/xcoc-bootstrap.json
```

也可使用交互流程。它会检查内置媒体运行时、完成配对，并继续引导发现或录入摄像头：

```sh
sudo xcoc setup --interactive
```

首次配对和 `setup --interactive --replace` 使用同一个 `Authorization code (visible)` 普通文本提示。输入或
粘贴的授权码会在终端中明文回显，不提供遮罩、隐藏切换或特殊显示流程；随后配置摄像头时，摄像头密码仍
使用隐藏输入。媒体工作进程只通过有界匿名 stdin 管道接收摄像头 RTSP 凭据、发布 URL/JWT 和私有回环地址；这些数据不进入进程参数、环境变量或临时凭据文件。客户端只报告安全的媒体错误，不输出底层库的原始日志。管理员或拥有服务进程调试/内存读取权限的账户仍可能读取运行中的秘密，必须限制本机账户、调试和配置访问权限。

保存输出中的实例 UUID。一次安装可以保存多个实例，但每个实例只对应一台摄像机。

## 3. RTSP 摄像头

复制模板并填写主/子码流。密码只放在受保护文件中：

```sh
sudo install -m 0600 config/camera.json.example /root/xcoc-camera.json
sudoedit /root/xcoc-camera.json
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
sudo sh -c 'exec xcoc camera apply --instance-id INSTANCE_UUID --input-stdin < /root/xcoc-camera.json'
sudo xcoc camera list
sudo shred -u /root/xcoc-camera.json
```

`storage_mode` 为 `server` 时由服务端录像；设为 `client` 时，客户端在本机状态目录写入 15 分钟 MP4 分段，历史录像不会自动上传服务端。

## 4. ONVIF 摄像头

先发现同一网络中的 ONVIF 设备：

```sh
sudo xcoc camera discover --timeout-seconds 3
```

复制并编辑 ONVIF 模板：

```sh
sudo install -m 0600 config/camera-onvif.json.example /root/xcoc-camera-onvif.json
sudoedit /root/xcoc-camera-onvif.json
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
sudo sh -c 'exec xcoc camera apply --instance-id INSTANCE_UUID --input-stdin < /root/xcoc-camera-onvif.json'
sudo xcoc camera list
sudo shred -u /root/xcoc-camera-onvif.json
```

profile token 为空时由适配器选择媒体 ONVIF Profile。发现为空时检查客户端与摄像头间的路由、防火墙和 UDP 3702 组播；也可以直接填写设备服务 URL。

## 5. 启动与完整验证

在未运行同一配置的后台服务时，可前台验证：

```sh
sudo xcoc run
```

正式运行由 Linux 的 `xcoc.service` 或 Windows 的 `XcocClient` 服务负责。Linux 可用 `systemctl status xcoc.service` 核验；Windows 可用 `Get-Service XcocClient` 核验。配置热更新时重新执行 `camera apply`，运行中的客户端会读取新 revision 并重建对应媒体工作进程，无需重启整个客户端。

完整成功需要同时确认：

1. `xcoc status` 显示实例已配对且摄像头已配置；
2. `xcoc camera list` 显示正确的实例、名称、适配器和录像位置；
3. 服务端管理页收到新快照，并能播放主码流；
4. `client` 模式下本地 MP4 分段持续增长，或 `server` 模式下服务端录像索引持续出现。

## 6. 修改、删除与轮换授权码

修改摄像头时编辑新的受保护 JSON，并对同一实例再次执行 `camera apply`。删除本地摄像头配置：

```sh
sudo xcoc camera remove INSTANCE_UUID
sudo xcoc camera list
```

删除只移除该实例的本地摄像头配置，不代表删除服务端实例或历史录像。

服务端管理员轮换实例授权码后，创建新的 Bootstrap 文件并更新配对凭据：

```sh
sudo sh -c 'exec xcoc setup --input-stdin < /root/xcoc-bootstrap.json'
sudo xcoc status
```

更新会保留该实例已保存的摄像头配置。`--replace` 只用于归档损坏或不兼容的旧配对文件；不要通过删除整个配置文件轮换凭据。

如果服务端删除了旧实例并创建了新实例，先运行 `sudo xcoc unpair OLD_INSTANCE_UUID`，再用新实例授权码执行 `setup`。`unpair` 只移除本地旧槽和摄像头配置，不撤销服务端授权且保留已有录像。服务端旧实例也应由管理员删除或撤销，避免界面残留。

## 7. 安全与排障

- 生产客户端只接受可信 HTTPS 服务端；服务端返回的发布 URL 必须是证书受系统信任且名称匹配的 `rtsps://` 地址。
- 摄像头 RTSP/ONVIF 地址和凭据只留在客户端，不写入日志、工单或服务端配置。
- 配置已保存只证明本地原子提交成功；还需等待 `run` 应用新 revision，并等待服务端收到下一份快照。
- 服务端暂时离线不会停止 `client` 模式本地录像；恢复连接后再核对快照和发布状态。
- 配对已成功但摄像头探测失败时，使用 `camera discover` 和 `camera apply` 继续，不要重新创建实例。

更多故障现象与核对顺序见[运行维护](operations.md)。配置权限、动作预算与未确认结果见[状态参考](runtime-reference.md)。
