# Android：安装、构建与排障

最低 Android 8.0/API 26，应用原生库为 arm64-v8a。当前源码版本为 **1.1.0**；截至 2026-10-10，公开 v1.0.0 Release 仅含桌面资产。1.1.0 移动未签名产物已通过对应 CI 构建，GitHub Release 尚未发布，仓库也不提供签名凭据。

## 安装与维护

### 1. 构建/安装

当前公开 Release 没有移动资产。先按下方[从源码构建](#从源码构建)准备工具链和 Debug APK；未签名 Release APK 需使用自己的正式密钥签名后才能安装。调试安装以开发机已有 ADB 和 arm64 真机为前提：

```sh
# 列出 USB 调试设备，手机确认授权后应显示 device；多设备时每条 adb 加 -s 实际序列号。
adb devices
# 覆盖安装同签名 APK并保留应用数据；不可将 Debug 当作正式包升级。
adb install -r clients/android/app/build/outputs/apk/debug/app-debug.apk
# 打开 Xcoc Camera 主界面。
adb shell am start -n org.sarmg.xcoc/.MainActivity
```

### 2. 配对、启动、查看和停止

按[移动端共同配对步骤](../mobile.md#使用)完成配对、授予摄像头权限并开始采集。界面和常驻通知显示状态；服务端应看到新快照并能播放视频。

点界面或通知的“停止摄像头”停止采集；再次启动使用已保存配对。Android 使用前台摄像头服务，不提供桌面 systemd/SCM 服务命令，不能假定重启手机后自动开机采集。

### 3. 重新配对、诊断、卸载

先停止摄像头，请管理员提供新授权码，再在相同服务端地址下点“配对”，成功后重新启动并验证画面。更换实例时核对目标实例，手机只保存一份配对。

```sh
# 查看安装版本、系统权限和应用服务运行信息；只读，不写入授权码。
adb shell dumpsys package org.sarmg.xcoc
adb shell dumpsys activity services org.sarmg.xcoc
# 获取应用进程 ID；进程未运行时输出为空，先打开应用再查日志。
adb shell pidof org.sarmg.xcoc
# 将 ACTUAL_PID 换成上条输出的实际 pid，只查看应用进程日志，Ctrl+C 结束。
adb logcat --pid=ACTUAL_PID
# 停止采集并完成 Server 退役后卸载；删除手机应用私有数据和本地配对。
adb uninstall org.sarmg.xcoc
```

也可在系统应用信息页卸载。先核对相机权限、通知/前台限制、网络、手机时间及 HTTPS/RTSPS 证书；没有跳过证书验证或导入私有 CA 的界面。卸载不删除服务端录像，也不代替撤销服务端实例。

## 从源码构建

在仓库根目录开始执行，按命令进入平台工程。

需要 Rust 1.99、JDK 17、Gradle 8.13、Android SDK 36、Build Tools 36.0.0、NDK r28 或更新版本和 cargo-ndk 4.1.2。将 `ANDROID_HOME` 设置为本机 Android SDK 的绝对路径；下例 NDK 路径也需替换。

以下命令使用 Bash。`rustup target add` 安装目标标准库，`cargo install` 安装固定版构建助手，仓库脚本生成 arm64 Rust/JNI 库；宿主构建供 JVM 测试使用，Gradle 运行测试并生成 Debug APK。
应用最低 Android 8.0/API 26，提供 arm64-v8a 原生库。Android/iOS 目标不编译或链接桌面 FFmpeg shim；宿主 JVM/JNI 测试针对桌面目标构建，需先按[桌面媒体运行时](../media-worker.md)准备宿主原生依赖。

```sh
rustup target add --toolchain 1.99.0 aarch64-linux-android
cargo +1.99.0 install cargo-ndk --version 4.1.2 --locked
export ANDROID_NDK_HOME=/absolute/path/to/android-sdk/ndk/28.2.13676358
bash scripts/build-android-rust.sh
cargo +1.99.0 build --locked -p xcoc-mobile-ffi --features jni-host-tests
gradle -p clients/android testDebugUnitTest assembleDebug
```

测试 APK 在 `clients/android/app/build/outputs/apk/debug/`。Release APK/AAB 的签名与构建见本页后续章节。
本仓库未放入签名密钥。arm64 原生库构建输出在 `clients/android/app/src/main/jniLibs/`，由 Git 忽略。

## Release 签名与安装

Android 的 APK 必须使用所有者稳定保管的 release key 签名后才能安装；AAB 不能直接安装，
需要签名后交由适用的分发流程生成 APK。不要用临时/debug key 冒充正式发行签名；更换签名会影响
后续更新能力。当前 CI 不创建或读取签名密钥，也不把 Debug APK 作为 Release 资产。


## 本地 Release 构建

先完成上面的对应平台原生库和工具链准备。Android 还需构建供 JVM 测试使用的宿主 JNI bridge，
并按桌面文档准备宿主媒体库。然后执行：

```sh
python3 packaging/mobile/release.py check
gradle -p clients/android testReleaseUnitTest assembleRelease bundleRelease lintRelease
python3 packaging/mobile/release.py android --aapt2 "$ANDROID_HOME/build-tools/36.0.0/aapt2"
```


产物在 `dist/`：`xcoc-android-arm64-v8a-unsigned.apk`、`xcoc-android-arm64-v8a-unsigned.aab` 和 `xcoc-android-release.json`。APK 需签名，AAB 不能直接安装。

构建输出的版本、源码 revision 和 SHA-256 校验规则见[移动端 Release 说明](../mobile.md#移动端-release-构建输出)。跨平台开发检查见[开发指南](../development.md)。

[共同配对与凭据说明](../mobile.md#使用) · [选择其他平台](../README.md#选择平台)
