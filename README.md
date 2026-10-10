# xcoc

## 项目简要介绍

xcos 的摄像头边缘客户端。在摄像头所在网络保存设备凭据、采集或探测视频，并向服务端发布码流。

## 项目功能

- RTSP/ONVIF 摄像头发现、品牌码流预设、主/子码流发布
- 电脑内置/USB 摄像头及手机前后摄像头采集
- 实例配对、状态上报、云台控制、客户端或服务端录像
- 桌面内置媒体工作进程；网络摄像头无需外部 `ffprobe`

## 适用平台

- 桌面：Linux x86_64、Windows x64、macOS Apple Silicon
- 移动：Android 8.0/API 26+ arm64、iOS 16+ arm64；iOS 切到后台会停止采集
- 内置/USB 摄像头额外需要带对应采集后端和 `libx264` 的 `ffmpeg`；手机使用系统采集和编码

## 如何快速部署

在 [下载页](https://github.com/isarmg/xcoc/releases) 选择平台和版本并核对同版 `SHA256SUMS`。当前 1.1.0 的内置媒体与移动端功能需使用对应版本产物；只有旧版资产时，先按下节编译，不能套用新版命令。

Linux 在解压目录或完成编译的源码根目录执行：

```sh
sudo sh packaging/linux/install.sh
sudo xcoc setup --interactive
sudo xcoc status
```

安装会立即启动 systemd 服务，开机自启提示回车默认 Yes。先在 xcos 创建实例，再输入 HTTPS 地址、授权码并配置摄像头；授权码会在交互终端明文显示，不要把凭据写入命令参数或日志。

Windows 使用 MSI 安装后运行 `xcoc setup --interactive`；macOS 手工安装 arm64 程序后运行 `xcoc setup --interactive` 和 `xcoc run`。移动应用需完成所有者签名才能实机安装：未签名 APK、AAB、IPA 或设备归档都不是可直接安装的正式包，模拟器 app 只适用于模拟器。

## 如何编译部署

桌面需要 Rust 1.99.0 和平台 C 工具链。Linux 还需 make、curl、pkg-config、Python 3、tar/xz、OpenSSL 开发静态库及 binutils；macOS 需 Xcode Command Line Tools、pkg-config、Python 3 和 xz。

```sh
bash packaging/native/build-unix.sh "$PWD/target/native-media"
export PKG_CONFIG_PATH="$PWD/target/native-media/lib/pkgconfig"
cargo +1.99.0 build --locked --release
python3 packaging/native/check-runtime.py ./target/release/xcoc
```

Linux 随后执行上面的安装命令；macOS 使用 `target/release/xcoc`。Windows 在装有 Visual Studio C++ Build Tools、Windows SDK、Python 3 和 Git 的 x64 Native Tools PowerShell 中执行：

```powershell
. .\packaging\native\build-windows.ps1 -WorkDirectory "$PWD\target\native-media"
cargo +1.99.0 build --locked --release
python packaging/native/check-runtime.py target/release/xcoc.exe
```

Android 使用 JDK 17、Gradle 8.13、SDK 36、NDK r28+ 和 cargo-ndk 4.1.2；先添加 `aarch64-linux-android` Rust target，运行 `scripts/build-android-rust.sh`，再执行 `gradle -p clients/android assembleRelease bundleRelease`。iOS 在 macOS/Xcode 上运行 `scripts/build-ios-rust.sh`，用 XcodeGen 生成工程后配置自己的签名团队。完整平台构建、安装与签名步骤见下方文档。

[详细文档](https://github.com/isarmg/xcoc/blob/main/docs/README.md)
