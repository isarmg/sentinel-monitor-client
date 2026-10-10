# xcoc 运维

完整的逐平台安装、配对、服务启停、升级与卸载见[分平台部署指南](platform-setup.md)。本文补充运行数据和故障边界。

## 1. 前置条件与状态文件

当前版本为 `1.0.0`。先用 `xcoc media-worker --check` 检查内置媒体运行时；网络摄像头无需 `ffmpeg` 或 `ffprobe` 命令，内置/USB 摄像头另需后台服务能找到支持该设备后端与 `libx264` 的 `ffmpeg`。构建/库依赖见[桌面媒体运行时](media-worker.md)。默认配置路径为：

- Linux：`/etc/isarmg/xcoc/config.json`
- macOS：`/Library/Application Support/XcocClient/config.json`
- Windows：`%ProgramData%\XcocClient\config.json`

Linux 安装包解压后运行 `sudo ./packaging/linux/install.sh`，Windows 使用 MSI。安装全部功能并立即运行后台服务；Linux 的开机自启提示回车默认为 Yes，Windows 交互配对时的同名提示回车也默认为 Yes。非交互配对默认 Yes。服务会在尚未配对时等待配置。

可用全局 `--config PATH` 指向其他文件。配置包含长期客户端 token、摄像头地址和可选账号密码，必须限制为服务
账户可读；不要把配置、stdin bootstrap JSON 或带凭据的 RTSP URL 手工写入日志、CLI 启动参数或工单。摄像头凭据、发布 JWT 和私有回环地址通过有界匿名 stdin 管道传给媒体工作进程，不放入进程参数、环境变量或临时凭据文件；媒体库原始日志被抑制，本地采集进程 stderr 被丢弃。管理员及具备进程调试/内存读取权限的本机账户仍可能访问运行中的秘密，必须限制服务账户权限和本机登录。
Windows 默认 `%ProgramData%\XcocClient` 的配置和录像由客户端收紧为仅 SYSTEM 和 Administrators 可读写内容；旧普通用户所有者无法重写 ACL。自定义 `--config` 的父目录不会自动调整权限，必须预先使用受保护的专用目录。Windows 配对已保存但服务未启动时，以管理员身份运行 `xcoc service`；若选择不开机自启，使用 `xcoc service --no-boot-start`，无需再次使用授权码。Windows MSI 升级可能将先前的不开机自启设置恢复为自动启动，升级后可重设。

## 2. 配对与摄像头配置

交互式流程会先检查内置媒体运行时，再完成配对、发现/录入摄像头并通过媒体库实际验证码流：

```sh
sudo xcoc setup --interactive
```

自动化配对从 stdin 读取严格 JSON，不接受未知字段：

```json
{
  "server": "https://xcos.example.com",
  "authorization_code": "36 位小写英文字母数字授权码",
  "name": "camera-edge-01"
}
```

```sh
sudo xcoc setup --input-stdin < bootstrap.json
```

同一服务端实例轮换授权码后，运行 `setup --input-stdin` 会保存新访问凭据并保留摄像头配置；`--replace` 用于归档损坏或不兼容的旧配对文件。配对已提交但摄像头配置失败时，不要重新创建实例，使用输出的实例 ID 继续：

```sh
sudo xcoc camera discover --timeout-seconds 3
sudo xcoc camera apply --instance-id INSTANCE_UUID --input-stdin < camera.json
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
sudo xcoc status
sudo xcoc camera list
systemctl status xcoc.service
```

Windows 用 `Get-Service XcocClient` 查看服务状态。`status` 只输出安装 ID、服务端、实例 ID、名称和是否已配置，不输出 token 或摄像头凭据。后台运行每两秒重新读取
配置：`camera apply` 或 `camera remove` 后无需重启，受影响的媒体工作进程会停止并按新配置重建。

应分别核验：

1. 客户端能探测主码流，服务端能看到最新快照；
2. 管理页可播放实时画面；
3. `storage_mode: "client"` 时本地 15 分钟 MP4 分段持续生成；
4. `storage_mode: "server"` 时服务端录像索引持续出现。

服务端暂时离线不应阻断本地录像；发布授权和快照会在连接恢复后重新获取。生产构建只接受可信 HTTPS 服务端，
服务端返回的发布地址必须是证书受系统信任且主机名匹配的 `rtsps://` URL。

## 4. 常见故障

| 现象 | 核对项 |
|---|---|
| Setup 在请求前失败 | `xcoc media-worker --check`；源码构建检查固定原生库与链接依赖；本地摄像头另检查服务 PATH 中的 `ffmpeg -version` |
| `pairing_authorization_rejected` | 实例授权码是否有效、是否已配对、是否应先在服务端更换授权码 |
| `pairing_protocol_unsupported` | 客户端与服务端是否都支持 `xcos-edge-v1` |
| ONVIF 发现为空 | 客户端是否与摄像头同一可达网段，UDP 3702 组播是否被网络策略拦截 |
| 配置已保存但画面离线 | 摄像头 URL/凭据、内置媒体探测、服务端 RTSPS 证书和客户端到发布端口的连通性 |
| 修改后服务端尚未更新 | `run` 是否仍在运行；等待下一次快照并检查该实例错误输出 |
| `pairing_state_incompatible` | 不兼容的账户文件会保留；创建新授权码后运行 `setup --interactive --replace` 归档该文件并重新配对 |
| `configuration_state_incompatible` | 当前摄像头配置不合法；文件已保留，不会被账户恢复流程清除 |
| `important_state_incompatible` | 本地录像布局、文件类型或可读性不兼容；录像已保留，禁止直接覆盖或自动迁移 |

服务端删除旧实例并创建新实例时，先运行 `sudo xcoc unpair OLD_INSTANCE_UUID` 移除本地旧配对，再使用新授权码执行 `setup`。`unpair` 只移除指定实例的本地配对和该实例摄像头配置，保留其他实例；最后一个实例移除后配置文件会被删除。它不撤销服务端授权，也不删除已保存的录影片段。摄像头配置单独用 `camera remove INSTANCE_UUID` 删除；客户端会向支持空快照的服务端清除该实例的摄像头。

不要通过手改本地 JSON 的 `format`、`installation_id` 或实例 ID 修复状态；运行时检测到安装身份变化会要求重启，
错误绑定可能使实例无法重新配对。修改摄像头应使用 `camera apply/remove` 的原子写入路径。


动作显示“未确认”时，先检查设备实际状态，再核对 command_results 的 outcome/error_code。不要将 unknown 当作失败重复同一动作。客户端命令日志已将意图持久化，崩溃后不重放可能已执行的 PTZ；配置旁的 `.commands.json` 是恢复证据，不能删除或覆盖。存储或日志写入失败会使服务退出，修复权限、空间后重新启动，原有记录继续回报。容量为零时仍回报状态和已有结果。

## Windows 后台诊断

SCM 在私有状态校验后创建 `%ProgramData%/XcocClient/logs`，使用共享类型化的 sink 写入 `xcoc.jsonl`。最多保留活动文件和四份归档，每份 8 MiB，总上限 40 MiB。服务账户首次创建此目录；管理员查询不会先替服务建立日志目录。ACL 拒绝普通用户，已有不安全对象不修复。服务启动失败且日志输出器尚不可用时，可同时查看 Windows SCM 的服务退出代码。

0.5.3 的公共查询按日志输出器的实际物理布局读取：活动文件为 `xcoc.jsonl`，四份归档为 `xcoc.jsonl.1` 至 `.4`。此前查询错误使用 `.01` 等归档名称；更新后直接读取现有文件，日志数据无需迁移或手动重命名。Windows 服务仍使用 LocalSystem，目录保持 SYSTEM 所有且只授予 SYSTEM/Administrators 访问；不改变摄像头适配或配置、命令事实的身份。

```powershell
xcoc logs --tail 100 --format json
xcoc logs --since 2026-10-07T00:00:00Z --level warn --format json
xcoc logs --follow --format ndjson --timeout 60s
```

可用 `--instance-id`、`--event`、`--request-id` 和 `--task-id` 精确筛选。后台整体启动事件使用 `scope=client`；属于某个已知实例的事件使用 `scope=instance` 并携带 `instance_id`。日志源损坏、超限或持续跟踪游标已从保留窗口移除时明确失败，避免把丢失记录显示为空成功。
