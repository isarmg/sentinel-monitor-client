# Android / iOS 原生摄像头客户端

移动端属于 xcoc 仓库，复用现有服务端的配对和快照 API、edge v1 三态能力与 RTSPS
发布授权。摄像头采集与协议转换均在客户端，服务端看见 `rtsp` 设备。当前支持一台设备的一份配对、
一个视频主码流，录像位置为服务端。应用版本仍随客户端包版本。

Android/iOS 从安装、配对、重新配对到采集启停、诊断与卸载的逐步流程见[分平台部署指南](platform-setup.md)。本文保留移动媒体边界与构建细节。

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

需要 Rust 1.99、JDK 17、Gradle 8.13、Android SDK 36、NDK r28 或更新版本和 cargo-ndk 4.1.2。
应用最低 Android 8.0/API 26，提供 arm64-v8a 原生库。Android/iOS 目标不编译或链接桌面 FFmpeg shim；宿主 JVM/JNI 测试针对桌面目标构建，需先按[桌面媒体运行时](media-worker.md)准备宿主原生依赖。

```sh
rustup target add --toolchain 1.99.0 aarch64-linux-android
cargo +1.99.0 install cargo-ndk --version 4.1.2 --locked
export ANDROID_NDK_HOME=/absolute/path/to/android-sdk/ndk/28.2.13676358
bash scripts/build-android-rust.sh
cargo +1.99.0 build --locked -p xcoc-mobile-ffi --features jni-host-tests
gradle -p clients/android testDebugUnitTest assembleDebug
```

测试 APK 在 `clients/android/app/build/outputs/apk/debug/`。Release APK/AAB 构建与签名边界见下文。
本仓库未放入签名密钥。arm64 原生库构建输出在 `clients/android/app/src/main/jniLibs/`，由 Git 忽略。

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

以下工作区检查在宿主桌面目标执行，需要桌面媒体库依赖；这不表示手机应用依赖 FFmpeg。

```sh
cargo +1.99.0 fmt --all -- --check
cargo +1.99.0 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.99.0 test --locked --workspace --all-targets --all-features
```

共享移动端 ABI 使用 xcsc revision 1：长度限定的输入、代际句柄、拥有所有权的结果和 panic
边界。C 调用方初始化结果后，读取并用 `xcsc_ffi_result_free_v1` 释放一次；编码帧在返回前复制，
不会保留平台缓冲指针。媒体队列固定 4 帧，每帧最多 4 MiB；超量会请求下一个 IDR 恢复，捕获线程
不等待网络写入。控制请求与媒体写入有超时，服务端响应、RTSP 头和 NAL 数量均受上限约束。

CI 包含 Android Debug/Release APK、Release AAB、Kotlin 单元测试与 lint，
iOS Debug/Release 模拟器构建、Swift 单元测试及未签名实机 Release archive。构建源码的本地检查与
实机能力认证必须分别记录，不能把 Linux Rust 测试当作 Android/iOS 应用实际验收。

媒体验收可以在 Linux x86_64 或 macOS ARM64 配合对应平台固定的 MediaMTX 1.20.0 执行：

```sh
XCOC_TEST_MEDIAMTX=/absolute/path/to/mediamtx bash scripts/test-media.sh
```

脚本核对服务端已采用的 companion 哈希，生成临时证书和合成 H.264，在回环验证单输入并行
探测/录像、原生 RTP 发布可由 RTSP 读出、生产入口拒绝不受信任证书；结束后清理临时进程和夹具。

## 移动端 Release 产物

主分支和 PR 的完整 CI 会实际构建 Android/iOS Release，并上传以下 Actions artifacts；
也可在 `xcoc Mobile Release builds` workflow 选择明确源码 ref 手动构建。手动执行只保留构建产物，
不创建 Release、不移动标签，也不上传应用商店。Android/iOS Release 失败会使对应 CI 检查失败。

- `xcoc-android-arm64-v8a-unsigned.apk`：Android 8.0/API 26+、arm64-v8a 的未签名 Release APK。
- `xcoc-android-arm64-v8a-unsigned.aab`：相同应用的未签名 Android App Bundle。
- `xcoc-ios-arm64-unsigned.xcarchive.zip`：iOS 16+、arm64 的未签名实机 Xcode archive。
- `xcoc-ios-simulator-arm64.app.zip`：arm64 iOS Simulator 的 Release app；只在模拟器使用。
- `xcoc-android-release.json`、`xcoc-ios-release.json`：应用标识、版本、实际源码提交、签名状态、
  每个产物的 SHA-256 和大小。应用标识均为 `org.sarmg.xcoc`。

发布 workflow 仍只接受与 Cargo 版本和发行说明一致的、不可覆盖的 annotated `vX.Y.Z` 标签。
桌面和移动构建全部成功后，由同一个 GitHub Release 发布步骤收集产物并生成统一的 `SHA256SUMS`。
现有标签和已有 Release 不会被覆盖；不得把新源码产物附到旧版本标签。移动端原生库不会打包桌面
FFmpeg shim 或桌面媒体依赖。

### 未签名不等于可安装发行版

Android 的 APK 必须使用所有者稳定保管的 release key 签名后才能安装；AAB 不能直接安装，
需要签名后交由适用的分发流程生成 APK。不要用临时/debug key 冒充正式发行签名；更换签名会影响
后续更新能力。当前 CI 不创建或读取签名密钥，也不把 Debug APK 作为 Release 资产。

iOS 的 `.xcarchive` 不是 IPA，不能直接装到 iPhone。所有者需要在 Xcode 中使用自己的 Apple
签名身份、开发团队和适用的 provisioning profile，按实际分发方式构建/签名并导出。当前 workflow
不导出或伪造“可安装”的未签名 IPA，不创建 Apple 证书，不接受开发者协议，不上传 TestFlight/App Store。
模拟器 app 可解压后用 `xcrun simctl install booted XcocCamera.app` 安装到已启动的 arm64 模拟器；
模拟器测试不证明真实摄像头采集或实机分发可用。

### 本地 Release 构建

先完成上面的对应平台原生库和工具链准备。Android 还需构建供 JVM 测试使用的宿主 JNI bridge，
并按桌面文档准备宿主媒体库。然后执行：

```sh
python3 packaging/mobile/release.py check
gradle -p clients/android testReleaseUnitTest assembleRelease bundleRelease lintRelease
python3 packaging/mobile/release.py android --aapt2 "$ANDROID_HOME/build-tools/36.0.0/aapt2"
```

macOS 上生成 Xcode 工程后，使用与 CI 相同的 Release 参数：

```sh
xcodebuild -project clients/ios/XcocCamera.xcodeproj -scheme XcocCamera \
  -configuration Release -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath clients/ios/ReleaseBuild/simulator \
  CODE_SIGNING_ALLOWED=NO ENABLE_TESTABILITY=NO ARCHS=arm64 ONLY_ACTIVE_ARCH=NO build
xcodebuild -project clients/ios/XcocCamera.xcodeproj -scheme XcocCamera \
  -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
  -archivePath clients/ios/ReleaseBuild/XcocCamera.xcarchive \
  -derivedDataPath clients/ios/ReleaseBuild/device \
  CODE_SIGNING_ALLOWED=NO ARCHS=arm64 ONLY_ACTIVE_ARCH=NO archive
python3 packaging/mobile/release.py ios
```

产物输出在 `dist/`。验证脚本拒绝版本漂移、Debug APK、错误 app ID/目标平台、缺少或非 arm64 的
原生库，以及混入签名的“unsigned”产物；元数据中的源码 revision 来自当前 checkout。CI 会另用独立
DerivedData 在 Release 下启用 testability 跑 Swift 测试，最终模拟器产物重新以 testability 关闭构建。
版本升级时同时更新 Cargo、移动 FFI、Android versionName/versionCode 和 iOS MARKETING_VERSION/
CURRENT_PROJECT_VERSION。Android versionCode 使用 `major*1000000 + minor*1000 + patch`；iOS
使用 `major.minor.patch`，并由共享校验器检查平台允许的数字范围。

参考 [Android 命令行构建与签名](https://developer.android.com/build/building-cmdline) 和
[Apple 分发与归档说明](https://developer.apple.com/documentation/xcode/distributing-your-app-for-beta-testing-and-releases)。
