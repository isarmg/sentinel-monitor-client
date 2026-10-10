# Android / iOS 原生摄像头客户端

移动端属于 xcoc 仓库，复用现有 Server 的配对和快照 API、edge v1 三态能力与 RTSPS
发布授权。摄像头采集与协议转换均在 Client，Server 看见 `rtsp` 设备。当前支持一台设备的一份配对、
一个视频主码流，录像位置为 Server。应用版本仍随 Client 包版本。

Android/iOS 从安装、配对、重新配对到采集启停、诊断与卸载的逐步流程见[分平台部署指南](platform-setup.md)。本文保留移动媒体边界与构建细节。

## 使用

在 Server 创建一个摄像头实例，复制授权码。在应用中输入 Server 的 HTTPS 根地址、实例授权码和
摄像头名称，点击“配对”，授予系统摄像头权限，选择前置或后置摄像头，然后“启动摄像头”。
Android 连接状态见常驻通知，通知和界面均可停止。iOS 连接状态见界面，切入后台即停止摄像头。
回到 iOS 前台后需要点击启动。视频尺寸由实际采集结果上报。

授权码只用于配对；长期 token 和安装身份保存在 Android Keystore 加密的私有文件 / iOS Keychain
`WhenUnlockedThisDeviceOnly`。token 和发布 URL 不进入 UI 或日志。更换配对前停止采集，使用
Server 新授权码重新配对。摄像头权限被收回或编码失败会停止摄像头，恢复权限后重新启动。

移动端 HTTPS 与 RTSPS 使用相同的 WebPKI 公共 CA 信任集并校验证书主机名。不支持跳过证书验证，
也未提供导入私有 CA 的界面；私有部署需使用可信公共 CA 证书。

## Android 构建

需要 Rust 1.99、JDK 17、Gradle 8.13、Android SDK 36、NDK r28 或更新版本和 cargo-ndk 4.1.2。
应用最低 Android 8.0/API 26，提供 arm64-v8a 原生库。

```sh
rustup target add --toolchain 1.99.0 aarch64-linux-android
cargo +1.99.0 install cargo-ndk --version 4.1.2 --locked
export ANDROID_NDK_HOME=/absolute/path/to/android-sdk/ndk/28.2.13676358
bash scripts/build-android-rust.sh
cargo +1.99.0 build --locked -p xcoc-mobile-ffi --features jni-host-tests
gradle -p clients/android testDebugUnitTest assembleDebug
```

测试 APK 在 `clients/android/app/build/outputs/apk/debug/`。正式分发需自行配置 Android 签名；本仓库
未放入签名密钥。arm64 原生库构建输出在 `clients/android/app/src/main/jniLibs/`，由 Git 忽略。

## iOS 构建

需要 macOS、Xcode（含 iOS SDK）、XcodeGen 和 Rust 1.99。最低 iOS 16；设备与模拟器原生库都是
arm64。源码中的 XcodeGen 配置负责生成工程，Vendor 目录只保存生成产物。

```sh
rustup target add --toolchain 1.99.0 aarch64-apple-ios aarch64-apple-ios-sim
bash scripts/build-ios-rust.sh
cd clients/ios
xcodegen generate
xcodebuild -project XcocCamera.xcodeproj -scheme XcocCamera -sdk iphonesimulator \
  -destination 'platform=iOS Simulator,name=iPhone 16' CODE_SIGNING_ALLOWED=NO test
```

实机安装时在 Xcode 选择自己的开发团队和目标设备。模拟器只能验证应用构建与协议转换单元测试，
真实视频采集必须使用实机。没有声明 iOS 摄像头后台常驻能力。

## 开发检查

```sh
cargo +1.99.0 fmt --all -- --check
cargo +1.99.0 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.99.0 test --locked --workspace --all-targets --all-features
```

共享移动端 ABI 使用 xcsc revision 1：长度限定的输入、代际句柄、拥有所有权的结果和 panic
边界。C 调用方初始化结果后，读取并用 `xcsc_ffi_result_free_v1` 释放一次；编码帧在返回前复制，
不会保留平台缓冲指针。媒体队列固定 4 帧，每帧最多 4 MiB；超量会请求下一个 IDR 恢复，捕获线程
不等待网络写入。控制请求与媒体写入有超时，Server 响应、RTSP 头和 NAL 数量均受上限约束。

CI 包含 Android APK / Kotlin 单元测试和 iOS 模拟器构建 / Swift 单元测试。构建源码的本地检查与
实机能力认证必须分别记录，不能把 Linux Rust 测试当作 Android/iOS 应用实际验收。

媒体验收可以在 Linux x86_64 或 macOS ARM64 配合对应平台固定的 MediaMTX 1.20.0 执行：

```sh
XCOC_TEST_MEDIAMTX=/absolute/path/to/mediamtx bash scripts/test-media.sh
```

脚本核对 Server 已采用的 companion 哈希，生成临时证书和合成 H.264，在 loopback 验证单输入并行
探测/录像、原生 RTP 发布可由 RTSP 读出、生产入口拒绝不受信任证书；结束后清理临时进程和夹具。
