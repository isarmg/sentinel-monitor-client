# 日常使用

先按[平台指南](platform-setup.md)安装，再完成[配对和摄像头配置](configuration.md)。以下 Linux 示例以默认系统配置为准；Windows 使用管理员终端，macOS 按平台指南选择同一个运行用户。

## 确认摄像头可用

```sh
sudo xcoc status
sudo xcoc camera list
```

核对实例 UUID、摄像头名称、适配器和录像位置。随后在 xcos 管理页确认最新快照、播放主码流，并查看新录像。
`status` 是本地配置摘要；它与服务运行状态、远端画面是三项独立检查。

- `storage_mode: "server"`：在 xcos 查看录像索引。
- `storage_mode: "client"`：在客户端查看持续生成的 15 分钟 MP4 分段；历史片段保存在本机。
- 手机应用：录像位于服务端；Android 可从通知停止采集，iOS 切到后台会停止，回到前台后手动启动。

## 修改摄像头

按[配置指南](configuration.md#3-rtsp-摄像头)准备受保护 JSON，对同一实例执行 `camera apply`。
常驻进程每两秒重新读取配置，重建受影响的媒体工作进程。等待下一份快照后检查新的名称、适配器和画面。

摄像头配置已提交但探测失败时，使用原实例 UUID 继续修改即可；无需再创建服务端实例。

## 更换授权码

1. 在 xcos 的原实例中更换授权码。
2. 用新授权码准备配对 JSON，执行 `setup --input-stdin`。
3. 核对 `status`、摄像头配置和服务端画面。

同一实例重配会保留已有摄像头设置。授权码会在桌面交互终端明文显示；在私密终端操作，避免录屏或共享日志。摄像头密码使用隐藏输入。

## 移除摄像头或配对

```sh
sudo xcoc camera remove INSTANCE_UUID
sudo xcoc camera list
```

将 `INSTANCE_UUID` 替换为 `status` 中的真实 UUID。该操作移除摄像头配置，保留本地配对和已存录像。
需要移除整个本地配对时运行 `sudo xcoc unpair INSTANCE_UUID`：它同时移除该实例的摄像头配置，保留其他实例和录像；服务端授权需在 xcos 中另行撤销。

## 云台结果未确认

先核对摄像头实际位置和 xcos 中的动作结果，再决定新的操作。超时可能发生在设备已经执行后；xcoc 会保留执行证据并上报 `unknown`，不会自动重放。配置旁的 `.commands.json` 用于继续确认，保留此文件。
