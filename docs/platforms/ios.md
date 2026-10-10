# iOS：安装、构建与排障

最低 iOS 16，实机和模拟器原生库均为 arm64；构建需要 Mac。当前源码版本为 **1.1.0**；截至 2026-10-10，公开 v1.0.0 Release 仅含桌面资产。1.1.0 移动未签名产物已通过对应 CI 构建，GitHub Release 尚未发布，仓库也不提供签名凭据。

## 安装与维护

### 1. 安装

最低 iOS 16，当前没有可直接安装的正式签名 IPA。按下方[从源码构建](#从源码构建)在 macOS 构建 Rust XCFramework，以 XcodeGen 生成 `XcocCamera.xcodeproj`，在 Xcode 选择自己的开发团队和已连接的真机，配置签名后运行。模拟器可验证构建/协议测试，实际摄像头采集必须在真机验收。

### 2. 配对、启动、查看和停止

按[移动端共同配对步骤](../mobile.md#使用)完成配对、授予摄像头权限并开始采集；状态在应用界面查看，到服务端验证画面和录像。点“停止摄像头”即可结束。**进入后台会停止采集，回到前台需手动再次启动**，没有 iOS 后台常驻摄像头服务。

### 3. 重新配对、诊断和卸载

先停摄像头，用服务端新授权码重新配对，再启动核验。权限拒绝时从系统应用设置恢复；采集失败/离线时检查设备网络、证书、相机占用和界面状态。实机日志通过 Xcode 的设备控制台查看，保留必要的脱敏错误信息；不要公开 token 或发布地址。

卸载前停止摄像头，在服务端撤销/退役实例；使用系统“删除 App”而非仅移除主屏幕图标。应用私有数据会被删除，Keychain 项可能由系统保留，不能把重装视为凭据已撤销；重新安装后按界面实际状态和服务端当前授权重新配对。服务端录像由服务端保留策略管理。

## 从源码构建

在仓库根目录开始执行，按命令进入平台工程。

需要 macOS、Xcode（含 iOS SDK）、XcodeGen 和 Rust 1.99。最低 iOS 16；设备与模拟器原生库都是
arm64。源码中的 XcodeGen 配置负责生成工程，Vendor 目录只保存生成产物。

```sh
rustup target add --toolchain 1.99.0 aarch64-apple-ios aarch64-apple-ios-sim
bash scripts/build-ios-rust.sh
cd clients/ios
xcodegen generate
xcrun simctl list devices available
```

从列表选择本机已有的 iPhone 模拟器，替换下面的 UUID；仍在 `clients/ios` 目录执行：

```sh
simulator_udid="REPLACE_WITH_AVAILABLE_IPHONE_SIMULATOR_UUID"
xcodebuild -project XcocCamera.xcodeproj -scheme XcocCamera -sdk iphonesimulator \
  -destination "platform=iOS Simulator,id=$simulator_udid" CODE_SIGNING_ALLOWED=NO test
```

实机安装时在 Xcode 选择自己的开发团队和目标设备。模拟器只能验证应用构建与协议转换单元测试，
真实视频采集必须使用实机。没有声明 iOS 摄像头后台常驻能力。

## Release 签名与安装

iOS 同时提供 `.xcarchive` 和由该归档中实机 app 打包的 `-unsigned.ipa`。未签名 IPA 也不能直接
装到 iPhone；它只提供标准 `Payload/XcocCamera.app` 布局，不代表已完成签名、provisioning 或
App Store 验证。所有者需要使用自己的 Apple 签名身份、开发团队和适用的 provisioning profile，
按实际分发方式签名/导出。Apple 签名身份、开发团队、协议和分发账号由应用所有者管理。
模拟器 app 可解压后用 `xcrun simctl install booted XcocCamera.app` 安装到已启动的 arm64 模拟器；
模拟器测试不证明真实摄像头采集或实机分发可用。

## 本地 Release 构建

生成 Xcode 工程后，回到仓库根目录，使用与 CI 相同的 Release 参数：

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


产物在 `dist/`：`xcoc-ios-arm64-unsigned.ipa`、`xcoc-ios-arm64-unsigned.xcarchive.zip`、`xcoc-ios-simulator-arm64.app.zip` 和 `xcoc-ios-release.json`。前两项供签名分发，模拟器 app 只用于 arm64 模拟器。

构建输出的版本、源码 revision 和 SHA-256 校验规则见[移动端 Release 说明](../mobile.md#移动端-release-构建输出)。跨平台开发检查见[开发指南](../development.md)。

[共同配对与凭据说明](../mobile.md#使用) · [选择其他平台](../README.md#选择平台)
