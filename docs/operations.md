# 运行维护与排障

服务安装和启停见[平台指南](platform-setup.md)，配对和摄像头 JSON 见[配置指南](configuration.md)。本文用于已经配置的客户端。

## 检查顺序

1. `xcoc --version`：确认正在使用对应源码版本的程序。
2. `xcoc media-worker --check`：桌面内置媒体库检查应成功。
3. `xcoc status` 和 `xcoc camera list`：核对配对实例与摄像头配置。
4. 查看实际运行服务及最近错误，再到 xcos 检查快照、画面和录像。

服务与日志命令按系统查阅：[Linux](platforms/linux.md#5-诊断)、[Windows](platforms/windows.md#5-诊断)、[macOS](platforms/macos.md#4-诊断和卸载)。
内置/USB 摄像头还需在服务的 PATH 中找到带对应采集后端和 `libx264` 的 `ffmpeg`。网络摄像头使用内置媒体库。

## 按症状排查

| 症状 | 检查 | 预期结果与下一步 |
|---|---|---|
| setup 在请求前失败 | `media-worker --check`、本地采集依赖 | 媒体检查通过；源码构建按[媒体指南](media-worker.md)核对原生链接 |
| `pairing_authorization_rejected` | xcos 实例和授权码 | 使用原实例的有效授权码；需要重配时由服务端生成新码 |
| `pairing_protocol_unsupported` | 两端版本与协议 | 两端均支持 `xcos-edge-v1` |
| ONVIF 发现为空 | 摄像头网段、路由、UDP 3702 | 使用可达网络，或在配置中直接填写设备服务 URL |
| 已配置但无画面 | 摄像头凭据、主码流、RTSPS 地址和证书 | 实际设备可读，发布证书可信且名称匹配，客户端可达发布端口 |
| 配置改变后显示旧状态 | 服务运行、配置路径、实例 UUID | 等待下一份快照；确认改的是该服务读取的配置 |
| 本地录像正常、远端离线 | 服务端连接和发布授权 | 恢复连接后核对新快照和画面；本地录像继续保存 |

## 状态文件错误

- `pairing_state_incompatible`：保留原文件，核对错误后用新的授权码执行 `setup --interactive --replace`；该命令先归档不兼容配对文件。
- `configuration_state_incompatible`：修正当前摄像头设置；账户替换流程会保留这类错误配置。
- `important_state_incompatible`：检查录像布局、文件类型与访问权限，保留现有录像后由管理员处理。
- 动作未确认或命令日志错误：核对设备状态和剩余磁盘空间，保留 `.commands.json`。修复存储后重新启动会继续上报已有结果。

摄像头修改使用 `camera apply/remove`，实例修改使用 `setup/unpair`；这些入口验证身份并原子保存。直接改写身份、格式或删除执行日志会破坏配对或动作确认依据。

## 运行数据

默认配置和录像目录见[平台指南](platform-setup.md#部署前准备)。配置、录像与命令日志只向管理员及运行账户开放。
日志使用 UTC、实例 UUID 和稳定错误码；反馈问题时提供版本、平台、症状与脱敏日志，保留 token、RTSP 地址、密码及私有文件。
媒体进程的凭据传递与配置权限细节见[状态参考](runtime-reference.md)和[媒体运行时](media-worker.md)。

## Windows 后台诊断

SCM 日志路径、权限、轮转和查询示例统一见 [Windows 后台日志与权限](platforms/windows.md#后台日志与权限)。
