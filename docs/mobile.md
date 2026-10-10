# Android / iOS 原生摄像头客户端

移动端属于 xcoc 仓库，复用现有服务端的配对和快照 API、edge v1 三态能力与 RTSPS
发布授权。摄像头采集与协议转换均在客户端，服务端看见 `rtsp` 设备。当前支持一台设备的一份配对、
一个视频主码流，录像位置为服务端。应用版本仍随客户端包版本。

按设备选择 [Android 安装与构建](https://github.com/isarmg/xcoc/blob/main/docs/platforms/android.md)或 [iOS 安装与构建](https://github.com/isarmg/xcoc/blob/main/docs/platforms/ios.md)。本文保存两端共用的配对、凭据、ABI 与发布校验说明。

## 使用

在服务端创建一个摄像头实例，复制授权码。在应用中输入服务端的 HTTPS 根地址、实例授权码和
摄像头名称，点击“配对”，授予系统摄像头权限，选择前置或后置摄像头，然后“启动摄像头”。
Android 连接状态见常驻通知，通知和界面均可停止。iOS 连接状态见界面，切入后台即停止摄像头。
回到 iOS 前台后需要点击启动。视频尺寸由实际采集结果上报。

授权码只用于配对；长期 token 和安装身份保存在 Android Keystore 加密的私有文件 / iOS Keychain
`WhenUnlockedThisDeviceOnly`。token 和发布 URL 不进入 UI 或日志。更换配对前停止采集，使用
服务端新授权码重新配对。摄像头权限被收回或编码失败会停止摄像头，恢复权限后重新启动。

移动端 HTTPS 与 RTSPS 使用相同的 WebPKI 公共 CA 信任集并校验证书主机名。不支持跳过证书验证，
也未提供导入私有 CA 的界面；私有部署需使用可信公共 CA 证书。

## Android 构建

工具链、Debug/Release 命令、签名、ADB 安装和诊断见 [Android 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/android.md)。

## iOS 构建

Mac 工具链、XCFramework、模拟器、实机签名和诊断见 [iOS 指南](https://github.com/isarmg/xcoc/blob/main/docs/platforms/ios.md)。

## 开发检查

跨平台工作区检查统一见[开发指南](https://github.com/isarmg/xcoc/blob/main/docs/development.md#开发验证)。宿主检查需准备桌面媒体库；手机目标使用系统采集和编码器。

共享移动端 ABI 使用 xcsc revision 1：长度限定的输入、代际句柄、拥有所有权的结果和 panic
边界。C 调用方初始化结果后，读取并用 `xcsc_ffi_result_free_v1` 释放一次；编码帧在返回前复制，
不会保留平台缓冲指针。媒体队列固定 4 帧，每帧最多 4 MiB；超量会请求下一个 IDR 恢复，捕获线程
不等待网络写入。控制请求与媒体写入有超时，服务端响应、RTSP 头和 NAL 数量均受上限约束。

CI 包含 Android Debug/Release APK、Release AAB、Kotlin 单元测试与 lint，
iOS Debug/Release 模拟器构建、Swift 单元测试及未签名实机 Release archive。构建源码的本地检查与
实机能力认证必须分别记录，不能把 Linux Rust 测试当作 Android/iOS 应用实际验收。

共享媒体回环验收见[开发指南](https://github.com/isarmg/xcoc/blob/main/docs/development.md#媒体验收)。

## 移动端 Release 构建输出

以下文件由当前源码的完整 CI 或手动构建生成。公开 v1.0.0 Release 不包含移动资产；1.1.0 标签和 Release 发布完成后，才可从对应下载页取得同版文件。

主分支和 PR 的完整 CI 定义了 Android/iOS Release 构建，并上传以下 Actions artifacts；
也可在 `xcoc Mobile Release builds` workflow 选择明确源码 ref 手动构建。手动执行只保留构建产物，
不创建 Release、不移动标签，也不上传应用商店。Android/iOS Release 失败会使对应 CI 检查失败。

- `xcoc-android-arm64-v8a-unsigned.apk`：Android 8.0/API 26+、arm64-v8a 的未签名 Release APK。
- `xcoc-android-arm64-v8a-unsigned.aab`：相同应用的未签名 Android App Bundle。
- `xcoc-ios-arm64-unsigned.ipa`：从实机归档生成的未签名应用包，包含 `Payload/XcocCamera.app`；需签名后才能装到 iPhone。
- `xcoc-ios-arm64-unsigned.xcarchive.zip`：iOS 16+、arm64 的未签名实机 Xcode archive。
- `xcoc-ios-simulator-arm64.app.zip`：arm64 iOS Simulator 的 Release app；只在模拟器使用。
- `xcoc-android-release.json`、`xcoc-ios-release.json`：应用标识、版本、实际源码提交、签名状态、
  每个产物的 SHA-256 和大小。应用标识均为 `org.sarmg.xcoc`。

发布 workflow 仍只接受与 Cargo 版本和发行说明一致的、不可覆盖的 annotated `vX.Y.Z` 标签。
桌面和移动构建全部成功后，由同一个 GitHub Release 发布步骤收集产物并生成统一的 `SHA256SUMS`。
现有标签和已有 Release 不会被覆盖；不得把新源码产物附到旧版本标签。移动端原生库不会打包桌面
FFmpeg shim 或桌面媒体依赖。

### 签名后安装或分发

选择 [Android 签名与安装](https://github.com/isarmg/xcoc/blob/main/docs/platforms/android.md#release-签名与安装)或 [iOS 签名与安装](https://github.com/isarmg/xcoc/blob/main/docs/platforms/ios.md#release-签名与安装)。未签名文件不能直接安装到真实设备；各平台指南说明 APK/AAB、IPA/archive 与模拟器 app 的用途。

### 本地 Release 构建

按 [Android Release 构建](https://github.com/isarmg/xcoc/blob/main/docs/platforms/android.md#本地-release-构建)或 [iOS Release 构建](https://github.com/isarmg/xcoc/blob/main/docs/platforms/ios.md#本地-release-构建)执行。

产物输出在 `dist/`。验证脚本拒绝版本漂移、Debug APK、错误 app ID/目标平台、缺少或非 arm64 的
原生库，以及混入签名的“unsigned”产物。IPA 另核对 Payload 布局、实机 app 标识/版本、arm64 Mach-O
可执行文件及执行权限，保留应用内容和包内相对符号链接，拒绝链接到包外的文件。IPA 与其他产物
一起写入版本元数据和 SHA-256 清单；元数据中的源码 revision 来自当前 checkout。CI 会另用独立
DerivedData 在 Release 下启用 testability 跑 Swift 测试，最终模拟器产物重新以 testability 关闭构建。
版本升级时同时更新 Cargo、移动 FFI、Android versionName/versionCode 和 iOS MARKETING_VERSION/
CURRENT_PROJECT_VERSION。Android versionCode 使用 `major*1000000 + minor*1000 + patch`；iOS
使用 `major.minor.patch`，并由共享校验器检查平台允许的数字范围。

参考 [Android 应用签名](https://developer.android.com/studio/publish/app-signing)和 [Apple 分发与归档说明](https://developer.apple.com/documentation/xcode/distributing-your-app-for-beta-testing-and-releases)。
