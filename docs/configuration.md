# xcoc 配置指南

首次部署或日常维护请先阅读[分平台全流程指南](platform-setup.md)：按本机平台完成安装、配对、重新配对、服务/后台任务查看与启停、诊断和卸载，命令旁均说明用途。本文详细说明配置字段和业务操作。

本文适用于 `xcoc` `1.0.0`，按实际 CLI 说明实例配对、RTSP/ONVIF 摄像头配置、更新和验证。

品牌码流预设、Windows/macOS/Linux 内置与 USB 摄像头见[摄像头兼容与协议转换](camera-support.md)；
Android/iOS 采集应用见[移动端指南](mobile.md)。这些输入统一转换成服务端已支持的 RTSP/RTSPS，
摄像头地址、平台采集选项与转换逻辑均由客户端管理。

## 命令用途与执行边界

| 命令 | 用途与影响 |
|---|---|
| `ffmpeg -version` / `ffprobe -version` | 核对采集/编码工具与码流探测工具已安装；后台也须能找到它们 |
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

客户端运行环境必须能找到同一发行套件中的 FFmpeg 和 FFprobe：

```sh
ffmpeg -version
ffprobe -version
xcoc --version
```

默认配置路径：

| 平台 | 配置文件 |
|---|---|
| Linux | `/etc/isarmg/xcoc/config.json` |
| macOS | `/Library/Application Support/XcocClient/config.json` |
| Windows | `%ProgramData%\XcocClient\config.json` |

Windows Release 同时提供 MSI。安装页可修改程序目录；全部客户端功能一起安装，`XcocClient` 系统服务会立即运行并等待配对。安装、升级和卸载均保留配置及 `%ProgramData%\XcocClient\recordings`。自定义安装路径可从 `HKLM\Software\sarmg\xcoc` 的 `InstallLocation` 读取。

Linux Release 解压后运行 `sudo ./packaging/linux/install.sh`。脚本安装全部功能并立即启动 `xcoc.service`；仅询问是否开机自启，直接回车默认 Yes。Windows 交互配对结束时也只询问是否开机自启，直接回车默认 Yes；`setup --input-stdin` 默认 Yes。选择 No 只改变下次开机行为，不停止当前运行。配对成功后服务会自动读取配置，无需再手动运行 `run`。

Linux 重新运行安装脚本升级时，脚本先备份旧程序和 systemd unit，再替换并启动；若安装或启动失败，会恢复旧文件、原来的开机自启设置和原先的运行状态。若回滚自身失败，错误输出会给出保留备份的位置，供管理员手动恢复。

其他路径可用全局参数指定：

```sh
xcoc --config /absolute/path/config.json status
```

配置包含长期客户端 token、摄像头 URL 和可选密码，只允许管理员及服务账号读取。不要直接编辑其身份、format 或实例 ID。
Windows 默认 `%ProgramData%\XcocClient` 目录和其中的配置、录影片段会收紧为仅 SYSTEM 和 Administrators 可读写内容。旧文件即使保留普通用户所有者，也会撤销所有者隐含的更改 ACL 权限。自定义 `--config` 路径不会改变其父目录权限；应预先创建仅服务账号及管理员可访问的专用目录，不要把配置放在普通用户可读的目录。
如果 Windows 配对已保存、但系统服务配置或启动失败，以管理员身份运行 `xcoc service` 继续启动；此前选择了不开机自启则运行 `xcoc service --no-boot-start`。这一步使用已保存的 token，无需重新申请授权码。
现有版本的 Windows MSI 升级会重建系统服务，可能把之前选择的不开机自启恢复为自动启动；升级后可重新运行 `xcoc service --no-boot-start`。

升级会区分账户配置和重要录像数据。不符合当前格式、损坏或无法识别的账户/配对文档返回
`pairing_state_incompatible`；创建新的服务端授权码后，显式运行
`xcoc setup --interactive --replace`，客户端会先原样归档该文档，再提交新配对。当前格式中的摄像头配置错误返回
`configuration_state_incompatible`，不会被 `--replace` 当作账户数据清除。本地录像目录如果包含无法识别的布局、链接、非 MP4 文件或不可读内容，返回
`important_state_incompatible` 并保留全部内容，必须由管理员核实并单独处理。

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

也可使用交互流程。它会检查 FFmpeg/FFprobe、完成配对，并继续引导发现或录入摄像头：

```sh
sudo xcoc setup --interactive
```

首次配对和 `setup --interactive --replace` 使用同一个 `Authorization code (visible)` 普通文本提示。输入或
粘贴的授权码会在终端中明文回显，不提供遮罩、隐藏切换或特殊显示流程；随后配置摄像头时，摄像头密码仍
使用隐藏输入。CLI 不会把授权码或摄像头密码写入自身日志、结果输出或启动参数。运行时 FFmpeg 子进程参数包含摄像头 RTSP 凭据，以及发布地址中的媒体 JWT；本机可查看进程参数的用户可能读到它们。Linux 部署应限制非特权用户读取其他进程的 `/proc`（例如 `hidepid=2`）并限制本机登录权限；客户端丢弃 FFmpeg 原始 stderr，避免它进入服务日志。

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

前台启动用于首次验证：

```sh
sudo xcoc run
```

正式运行由 Linux 的 `xcoc.service` 或 Windows 的 `XcocClient` 服务负责。Linux 可用 `systemctl status xcoc.service` 核验；Windows 可用 `Get-Service XcocClient` 核验。配置热更新时重新执行 `camera apply`，运行中的客户端会读取新 revision 并重建对应 FFmpeg 进程，无需重启整个客户端。

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

更多故障现象与核对顺序见[运维文档](operations.md)。

## 配置文件安全与运行日志

Unix 配置读写使用 xcsc `ConfigurationDirectory`：文件必须是服务用户拥有、单链接的普通文件，权限为 `0600` 或受控 `0640`，父路径不能经过符号链接。读取在分配前检查 1 MiB 上限，替换先同步临时文件，再原子 rename 并同步目录。公开可读、硬链接、符号链接或过大文件会拒绝，错误不会反射被拒绝的摄像头凭据。已有合法 `0640` 配置保留其 group 与权限。Windows 继续使用受保护 ACL 和原子替换。

运行错误使用 xcsc 内部 `xcsc::log` 输出 UTC JSON 行到 stderr；摄像头事件带规范化的 UUID `instance_id`，失败使用稳定 `error_code`。不输出摄像头密码、RTSP URL、内部错误链或 FFmpeg 原始 stderr。日志写入失败返回明确错误，服务宿主可观察退出。


## 能力与工作预算

`xcos-edge-v1` 使用 `supported`、`unsupported`、`unknown` 三态能力。尚未探测、设备离线或缺少肯定证据时报告 `unknown`；PTZ 只在所选 ONVIF Profile 和控制服务确认支持后提供。旧布尔线上协议不在正常运行入口接受。真实设备厂商与型号仍需分别完成探测、录像和控制验收。

FFprobe 每次最多等待 12 秒，stdout 上限 256 KiB、stderr 上限 64 KiB；超限或超时立即终止并回收子进程，原始 stderr 不进入错误输出。ONVIF 发现只接受对应本次 MessageID 的响应，最多 256 个唯一设备，每个设备最多 8 个不含凭据的 HTTP 服务地址和 32 个 Scope；超限返回 `DISCOVERY_LIMIT_EXCEEDED` 和结构事件，不报告部分发现为完整成功。

快照携带必填 `command_capacity`，服务端只下发该容量内的动作。客户端全局最多 256 个未完成动作、每相机最多 8 个；待确认结果、去重记录及正在发送的预留容量共用 4096 条记录预算。容量为 0 时仍发送状态和已有结果；超量动作在执行前明确拒绝，不静默丢弃 PTZ，也不通过无限积累消耗其他相机的处理空间。命令失败使用 xcsc 安全错误呈现与静态产品消息；保存配对后发生的故障明确保留已提交证据与继续操作说明。


## 动作确认与崩溃恢复

`command_results` 必须包含 `id`、`outcome`、`error_code`。成功为 `succeeded` 且无错误码；明确未执行或设备明确拒绝为 `failed`，使用 `invalid_command`、`unsupported_capability`、`expired_before_execution`、`device_unavailable`、`device_rejected`；可能已经执行而没有有效确认则为 `unknown` / `outcome_unknown`。PTZ 请求一旦发出，超时、断连和无效 SOAP 回应均属于未知，不能向用户表示为确定失败，也不会自动重试。

独立 `<配置文件名>.commands.json` 日志使用严格 format 1，最多 2048 条、1 MiB。它仅保存命令和相机 UUID、请求摘要、阶段、确认状态及结果，不保存地址、密码或动作正文。接受命令时先保存 queued；物理操作前原子保存并同步 pending；确认后保存 completed。启动时 pending 恢复为 unknown，queued 恢复为确定未执行的 device_unavailable。相同 UUID 和请求重发只回报持久结果；复用 UUID 改变请求会明确拒绝。

结果获服务端确认后仍保留至原到期时间后 120 秒；未确认的结果不因时间自动丢弃。日志达到上限时通过 command_capacity 停止接收新动作，继续发送已有结果。存储写入失败会停止执行；损坏或不兼容日志保留并明确报错。不要删除日志来重试 PTZ；先核对设备实际位置和服务端的未确认结果，由操作者决定新的动作。

运行权由 xcsc `SingleInstanceLock` 和独立私有 `.xcoc-runtime` 目录保护，拒绝不安全的锁路径。Linux 默认为 `/var/lib/isarmg/xcoc/.xcoc-runtime`；自定义配置在该配置的父目录下使用 `.xcoc-runtime`。配对文件与命令日志分别管理，凭据轮换不清除执行证据。
