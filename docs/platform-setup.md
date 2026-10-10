# xcoc 平台安装入口

当前源码版本为 **1.1.0**。截至 2026-10-10，公开 GitHub Release 仍为 v1.0.0，只有桌面资产；1.1.0 移动未签名产物已有 CI 构建记录，尚未发布 GitHub Release。先选择设备平台，再按对应页面完成安装、构建和排障。

| 设备平台 | 安装、构建与维护 |
|---|---|
| Linux x86_64 / systemd | [Linux x86_64 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/linux.md) |
| Windows x64 / MSI | [Windows x64 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/windows.md) |
| macOS Apple Silicon / 手工安装 | [macOS Apple Silicon 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/macos.md) |
| Android 8.0+ / arm64-v8a | [Android 8.0+ 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/android.md) |
| iOS 16+ / arm64 | [iOS 16+ 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/ios.md) |

平台页集中保存该系统的安装、源码构建、日志、更新和卸载步骤。摄像头字段与共享流程统一见[配置](https://github.com/isarmg/xcoc/blob/main/docs/configuration.md)、[日常使用](https://github.com/isarmg/xcoc/blob/main/docs/usage.md)和[排障](https://github.com/isarmg/xcoc/blob/main/docs/operations.md)。

## 部署前准备

1. 先核对[客户端 Releases](https://github.com/isarmg/xcoc/releases)上的实际版本和资产。对应版本发布后，下载本机平台资产和 `SHA256SUMS`；使用源码时，先完成同版构建。Linux/macOS 资产名不包含版本号，必须确认所在 Release 的版本。Windows 正式安装用 MSI，ZIP 仅用于手工运行。
2. 请服务端管理员创建摄像头实例，提供 HTTPS 根地址、实例授权码。桌面一次安装可管理多个实例，每个实例对应一台摄像机；移动端只管理一份配对和一个主码流。
3. 桌面使用内置媒体工作进程，以 `xcoc media-worker --check` 检查链接的 FFmpeg 库。网络摄像头不需要 `ffmpeg`/`ffprobe` 命令；内置/USB 摄像头另需带对应采集后端和 `libx264` 的 `ffmpeg`，且实际采集账户的 PATH 必须能找到它。见[媒体运行时](https://github.com/isarmg/xcoc/blob/main/docs/media-worker.md)与[兼容说明](https://github.com/isarmg/xcoc/blob/main/docs/camera-support.md)。手机使用系统摄像头/编码器，无需安装 FFmpeg。
4. 服务端 HTTPS 与发布用 RTSPS 证书都必须可信且名称匹配；摄像头 RTSP 可在本地网络内使用。授权码在桌面交互提示中明文回显，摄像头密码隐藏输入。
5. 平台页的注释解释命令用途。`INSTANCE_UUID`、`OLD_INSTANCE_UUID` 替换为实际实例 UUID；不要将授权码、摄像头密码或媒体 token 放在命令参数中。

| 平台 | 发行安装方式 | 服务与默认配置 | 本地录像 |
|---|---|---|---|
| Linux x86_64 | tar.gz 内 systemd 安装脚本 | `xcoc.service`；`/etc/isarmg/xcoc/config.json` | `/var/lib/isarmg/xcoc/recordings` |
| Windows x64 | MSI | SCM `XcocClient`（LocalSystem）；`C:\ProgramData\XcocClient\config.json` | `C:\ProgramData\XcocClient\recordings` |
| macOS Apple Silicon | tar.gz，手工部署 | 无随包安装器；默认配置 `/Library/Application Support/XcocClient/config.json` | `/Library/Application Support/XcocClient/recordings` |
| Android / iOS | 原生应用，按移动指南构建/签名安装 | 应用私有存储及系统安全存储 | 当前移动端在服务端录像 |

`status` 和 `camera list` 是本机只读视图；服务运行和本机已配对不能代替服务端的画面/录像验收。服务启停使用对应平台页的 systemd、SCM 或 launchd 命令。

## Linux x86_64（systemd）

打开 [Linux x86_64（systemd） 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/linux.md)。

## Windows x64

打开 [Windows x64 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/windows.md)。

## macOS Apple Silicon

打开 [macOS Apple Silicon 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/macos.md)。

## Android

打开 [Android 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/android.md)，完成构建、签名和设备验证。

## iOS

打开 [iOS 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/ios.md)，完成构建、签名和设备验证。

本入口使用在线文档链接，单独随安装包复制或移到其他目录后仍可访问完整指南。
