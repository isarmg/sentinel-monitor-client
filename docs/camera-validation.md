# 摄像头扩展验证记录

2026-10-08，本次工作区在 Linux 完成以下检查：

| 检查 | 结果 |
|---|---|
| Rust 1.99 fmt、workspace/all-targets/all-features 严格 Clippy | 通过 |
| 完整 Rust 回归 | 69 项通过；2 项媒体用例另行执行 |
| Linux 系统 FFmpeg 7.1.5：同一采集向探测器和录像器分发，结束后释放监听端口 | 通过 |
| 固定 MediaMTX 1.20.0：原生 H.264/RTP 经 RTSPS 发布，可通过 RTSP 探测真实尺寸 | 通过 |
| 同一媒体测试中生产 TLS 入口拒绝不受信任的临时证书 | 通过 |
| Android 源码：Kotlin 2.2.21、JVM target 17，使用本机 Android API 37 jar 编译 | 通过 |
| Kotlin H.264 转换与真实宿主 JVM/JNI 句柄、错误、帧输入测试 | 3 项通过 |
| Linux 安装器升级/失败恢复回归 | 11 项通过 |
| Windows WiX 安装器定义、构建脚本语法、配置 JSON 和 Android Manifest XML | 通过 |

这里的 Android 编译是直接检查原生 Kotlin 源码和宿主 JNI，不是已构建 APK；项目正式 Gradle
compileSdk 为 36。当前环境没有 Android NDK 和 Apple Xcode/iOS SDK，Android 目标原生库/APK
与 iOS Rust/Swift 应用构建未在本地完成，已经增加各自平台 CI 工作流。

没有连接实机摄像头。本次验证不代表六个品牌的所有型号、Windows/macOS 摄像头权限或 Android/iOS
实机完成认证。摄像头支持边界、操作和构建见 [camera-support.md](camera-support.md) 与 [mobile.md](mobile.md)。

## 2026-10-08 macOS 运行排查

本次在 macOS ARM64、Rust 1.99.0、Xcode 26.6 和 FFmpeg/FFprobe 9.0.2 环境重新验证。

修复了以下问题：

- macOS 的默认临时目录经 `/var` 符号链接访问，导致配置与命令日志测试触发生产安全校验。
  测试夹具现在在临时根目录的真实路径中创建；生产存储继续拒绝符号链接祖先。
- 配对前只检查目录可写，未检查安全路径。带符号链接的目录会在远端配对完成后才保存失败。
  Unix 现在在发出配对请求前通过 xcsc `ConfigurationDirectory` 验证目录；新增回归覆盖此路径。
- 共享媒体库无条件引入桌面 `xcsc::cli`，其服务管理代码无法为 iOS/Android 编译。
  该依赖现在仅在桌面目标中引入，保持现有 xcsc 版本和固定 revision。
- 媒体脚本依赖 macOS 默认没有的 `sha256sum`，且只接受 Linux companion 哈希。
  现在使用 Python 分块校验，分别固定 Linux x86_64 与 macOS ARM64 的 1.20.0 二进制哈希。

CI 的 macOS 原生构建任务现在同时执行完整 Rust 回归，以覆盖默认临时目录下的保护存储行为。

| 检查 | 结果 |
|---|---|
| Rust fmt、workspace/all-targets/all-features 严格 Clippy | 通过 |
| 完整 Rust 回归（macOS 默认 TMPDIR） | 70 项通过；2 项媒体用例单独验证 |
| CLI 与本机模拟服务端 | 通过 stdin 配对、0600 保存、状态脱敏、心跳、单实例锁、摄像头配置热更新、SIGTERM 退出和立即重启、最终解除配对 |
| 配对安全路径 | 符号链接祖先在请求前被拒绝，模拟服务端未收到配对请求 |
| FFmpeg 合成视频采集分发 | 同时探测与录像通过，释放采集后监听端口关闭 |
| 固定 MediaMTX 1.20.0 macOS ARM64 | 真实 H.264/RTP 经 RTSPS 发布后由 RTSP 探测出 H.264 和 320 像素宽度；生产 TLS 入口拒绝不受信任临时证书 |
| 媒体脚本完整性校验 | 通过；版本相同但哈希不符的二进制在创建夹具前被拒绝 |
| iOS 设备及模拟器 Rust 库 / XCFramework | 构建通过 |
| iOS 应用 / Swift H.264 单元测试 | Xcode 构建通过，1 项测试通过；已在 iPhone 17 Pro 模拟器启动并检查配对界面 |
| Android ARM64 Rust 原生库 | NDK 28.2.13676358、API 26 release 构建通过 |
| Linux 安装器回归 / Windows WiX 安装器定义 | 11 项安装器回归通过；安装器定义检查通过 |

CLI 验证使用 debug 构建允许的回环 HTTP 模拟服务端，没有连接真实服务端或摄像头。
iOS 模拟器启动不代表实机摄像头采集验收；本次未构建 Android APK，未运行 Windows/Linux 原生发行物。

## 1.0.0 xcsc 升级复验

2026-10-08，将共享客户端固定到正式 1.0.0 / 2644ff01f8a7e0fcc9d413b3964fe1e9ca9974b3，
中立日志固定到正式 1.0.0 / 020b40b1185a604ced7a01230781a3eadffe64c2。升级只改变 xcsc
来源及产品发行号，协议、配置、命令日志与录像身份保持。

本地重新执行 fmt、完整工作区严格 Clippy、70 项常规 Rust 测试及 2 项真实合成媒体测试，
CLI 模拟服务端生命周期、iOS 设备/模拟器库和 Swift 应用测试、Android ARM64 release 库，
以及 11 项安装器回归和 Windows WiX 安装器定义。上述检查均通过；本地未构建 Android APK，
未运行 Windows/Linux 原生安装器，正式发行另以最终提交的原生 CI 和实际 MSI/SCM 生命周期为准。

## 当前单体公共支撑来源

2026-10-10 当前客户端只消费一个 xcsc 1.0.0 包，固定 `c45e48e93e360542c2e1db6c6441a9e29b344b03`；中立日志已经物理归入 `xcsc::log`。上述 2026-10-08 来源行是其当时的记录，不代表当前锁文件。单体迁移的角色门禁、原生测试及受控合成媒体证据按最终源码分别记录，不能代替实体摄像头和手机采集验收。


## 2026-10-10 xcsc 1.0.1 输入更新

当前依赖已更新为官方 `xcsc =1.0.1`，固定 Git 修订 `d3e9b8db84e4ead70ec0bf8a596dbad697f7db24`，并同步产品能力清单和 `Cargo.lock`。上文关于 xcsc 1.0.0 的来源与验证描述保留为当时记录，不代表当前输入。新修订的客户端来源校验和 `cargo metadata --locked --all-features` 已通过；这两项不替代本产品最终源码的原生平台、实体设备或业务路径验收。
