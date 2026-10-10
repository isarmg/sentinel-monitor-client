# xcoc 开发与仓库结构

桌面构建先准备[媒体运行时与原生依赖](media-worker.md)，移动端构建和签名按[移动端指南](mobile.md)。
Android/iOS 不链接桌面媒体库；宿主 JVM/JNI 测试仍需宿主桌面依赖。

## 开发验证

先按[桌面媒体运行时构建说明](media-worker.md)准备原生 FFmpeg 库、C 编译器与链接依赖；Android/iOS 目标不链接这些桌面库。

```sh
cargo +1.99.0 fmt --all -- --check
cargo +1.99.0 clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo +1.99.0 test --locked --workspace --all-targets --all-features
```

## 仓库布局

根 Rust 包提供桌面 CLI 与共享摄像头/移动媒体库，`crates/mobile-ffi` 提供 xcsc v1 原生桥接，
`clients/android` 和 `clients/ios` 保存原生摄像头应用。`Cargo.lock` 固定工作区编译输入。
`src/main/tests.rs` 和 `src/onvif/tests.rs` 验证客户端生命周期与 ONVIF；根 `tests/` 保存独立验收。
`protocol/` 保存固定服务端源码的受控契约，`config/` 保存无凭据样例，`packaging/` 保存桌面安装器，
`scripts/` 提供移动库构建，`docs/` 描述配置与运行。

代码采用 [Apache License 2.0](../LICENSE-APACHE)。当前 1.1.0 的软件版本与公共支撑 1.0.0、edge v1、
ABI v1 分别管理。实际分发能力以对应源码的成功构建、目标平台验收和已发布资产为准。
