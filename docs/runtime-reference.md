# xcoc 状态与动作参考

本文说明配置持久化、能力上报和动作确认。操作步骤见[配置指南](configuration.md)，症状检查见[运行维护](operations.md)。

## 配置文件安全与运行日志

Unix 配置读写使用 xcsc `ConfigurationDirectory`：文件必须是服务用户拥有、单链接的普通文件，权限为 `0600` 或受控 `0640`，父路径不能经过符号链接。读取在分配前检查 1 MiB 上限，替换先同步临时文件，再原子 rename 并同步目录。公开可读、硬链接、符号链接或过大文件会拒绝，错误不会反射被拒绝的摄像头凭据。已有合法 `0640` 配置保留其 group 与权限。Windows 继续使用受保护 ACL 和原子替换。

运行错误使用 xcsc 内部 `xcsc::log` 输出 UTC JSON 行到 stderr；摄像头事件带规范化的 UUID `instance_id`，失败使用稳定 `error_code`。不输出摄像头密码、RTSP URL、内部错误链或 媒体库原始日志或本地采集进程 stderr。日志写入失败返回明确错误，服务宿主可观察退出。


## 能力与工作预算

`xcos-edge-v1` 使用 `supported`、`unsupported`、`unknown` 三态能力。尚未探测、设备离线或缺少肯定证据时报告 `unknown`；PTZ 只在所选 ONVIF Profile 和控制服务确认支持后提供。旧布尔线上协议不在正常运行入口接受。真实设备厂商与型号仍需分别完成探测、录像和控制验收。

媒体探测在独立工作进程执行，输入、结果大小和执行时间均受预算限制；超限或超时会终止并回收工作进程，原始媒体库日志不进入错误输出。ONVIF 发现只接受对应本次 MessageID 的响应，最多 256 个唯一设备，每个设备最多 8 个不含凭据的 HTTP 服务地址和 32 个 Scope；超限返回 `DISCOVERY_LIMIT_EXCEEDED` 和结构事件，不报告部分发现为完整成功。

快照携带必填 `command_capacity`，服务端只下发该容量内的动作。客户端全局最多 256 个未完成动作、每相机最多 8 个；待确认结果、去重记录及正在发送的预留容量共用 4096 条记录预算。容量为 0 时仍发送状态和已有结果；超量动作在执行前明确拒绝，不静默丢弃 PTZ，也不通过无限积累消耗其他相机的处理空间。命令失败使用 xcsc 安全错误呈现与静态产品消息；保存配对后发生的故障明确保留已提交证据与继续操作说明。


## 动作确认与崩溃恢复

`command_results` 必须包含 `id`、`outcome`、`error_code`。成功为 `succeeded` 且无错误码；明确未执行或设备明确拒绝为 `failed`，使用 `invalid_command`、`unsupported_capability`、`expired_before_execution`、`device_unavailable`、`device_rejected`；可能已经执行而没有有效确认则为 `unknown` / `outcome_unknown`。PTZ 请求一旦发出，超时、断连和无效 SOAP 回应均属于未知，不能向用户表示为确定失败，也不会自动重试。

独立 `<配置文件名>.commands.json` 日志使用严格 format 1，最多 2048 条、1 MiB。它仅保存命令和相机 UUID、请求摘要、阶段、确认状态及结果，不保存地址、密码或动作正文。接受命令时先保存 queued；物理操作前原子保存并同步 pending；确认后保存 completed。启动时 pending 恢复为 unknown，queued 恢复为确定未执行的 device_unavailable。相同 UUID 和请求重发只回报持久结果；复用 UUID 改变请求会明确拒绝。

结果获服务端确认后仍保留至原到期时间后 120 秒；未确认的结果不因时间自动丢弃。日志达到上限时通过 command_capacity 停止接收新动作，继续发送已有结果。存储写入失败会停止执行；损坏或不兼容日志保留并明确报错。不要删除日志来重试 PTZ；先核对设备实际位置和服务端的未确认结果，由操作者决定新的动作。

运行权由 xcsc `SingleInstanceLock` 和独立私有 `.xcoc-runtime` 目录保护，拒绝不安全的锁路径。Linux 默认为 `/var/lib/isarmg/xcoc/.xcoc-runtime`；自定义配置在该配置的父目录下使用 `.xcoc-runtime`。配对文件与命令日志分别管理，凭据轮换不清除执行证据。
