# 依赖和 unsafe 审查

Rust 固定 1.99.0；本轮选择 rand 0.10、base64 0.23、SHA-1/SHA-2 0.11，保留适用的 reqwest 0.13、Tokio 1、windows-service 0.8.1 和 windows-sys 0.61.2。协议和持久状态的版本由产品权威合同确定，与软件发行号分离。Cargo.lock 无 rusqlite；此客户端不持有 SQLite 数据库。

| 位置 | 保留原因与安全边界 | 安全替代或验证 |
|---|---|---|
| windows_config_acl::PrivateAcl | 管理员首次限制公开配置/录像目录，需要原生构造 protected DACL。descriptor 和 DACL 生命周期相同；失败和 Drop 通过 LocalFree 回收唯一原生分配。普通私有文件 opener 不具备这种行政修复权限。 | 固定 SDDL、有效输出参数，限制为 SYSTEM、Administrators 和仅 ReadControl 的 OwnerRights。 |
| windows_config_acl::secure_path | 获取原生 link count 后对同一已打开文件调用 SetSecurityInfo。Windows 稳定 Rust 尚未提供安全的 link count API；不能删除 hardlink 校验。 | 用安全 OpenOptions/File 替换原始创建和所有权转移；保持祖先和最终对象无 DELETE sharing，拒绝 reparse 和多链接文件。 |
| Windows ACL 测试 | 从 SDK 返回的 security descriptor 读取并释放 SDDL 字符串，检查实际权限而非模拟值。 | live 分配和显式返回长度；真实 hardlink 与祖先 junction 拒绝测试，确认原对象 ACL 和字节不变。 |
| main::private_child_umask | std::process 的 Unix pre_exec 本身是 unsafe；子进程在 exec 前只调用 async-signal-safe umask，让 ffmpeg 新录像具有私有模式。 | 不分配、不获取锁、不修改父进程；普通运行期配置、日志和命令 ledger 使用安全共享文件 API。 |
| mobile-ffi::xcoc_call_v1 / xcoc_frame_v1 | 平台调用者提供 live 长度限定缓冲和已初始化结果；Foundation v1 检查输入预算、隔离 panic、管理代际句柄与结果所有权。帧在返回前复制到有界队列。 | 不保留原生缓冲指针；检查错误、过期句柄、结果释放及 JNI bridge 编译。 |

`MetadataExt::number_of_links` 在 Rust 1.99 仍属于 nightly API，故保留小范围 Win32 元数据调用，不引入 nightly 工具链。[Rust 官方 MetadataExt](https://doc.rust-lang.org/stable/std/os/windows/fs/trait.MetadataExt.html#tymethod.number_of_links)。`OpenOptionsExt::share_mode` 可安全控制删除/重命名共享，因此已用于祖先及最终句柄。[Rust 官方 OpenOptionsExt](https://doc.rust-lang.org/stable/std/os/windows/fs/trait.OpenOptionsExt.html#tymethod.share_mode)。

所有保留 unsafe 块均说明调用和所有权条件。网络、PTZ、命令恢复、日志过滤和进程输出预算的业务逻辑使用安全 Rust。原生 CI 必须针对实际最终 Source；Linux 结果不代替 Windows 权限证明。

当前共享 Client 输入为 0.10.5 / a3c827b7f0f69d84ff69f72be1533a7171ab63d6，来自正式 v0.10.5 标签；产品按自身最终 Source 独立执行平台验证。
中立日志输入为 0.11.5 / d58b9ef0822984ee0d29fb8b8139cfd2787374fb，来自正式 v0.11.5 标签，精确 Git 来源与 Cargo.lock 一致。

0.5.3 更新共享 Client 锁守卫的正式源码输入：Unix single-instance guard 在销毁前显式解锁，持久锁 inode 不删除；同时保留已修复的日志查询：使用 sink 的 `.jsonl` 活动文件及 `.jsonl.1`–`.jsonl.4` 四份归档，查询和跨轮转 follow 读取已有文件，不引入产品 adapter 或状态迁移。Windows 继续运行 LocalSystem，普通 PrivateDirectory 已接受 SYSTEM 所有和 SYSTEM/Administrators 私有 ACL；无需扩展权限 API，没有新增产品 unsafe。最终 Source 仍独立执行各平台 CI，Mac 验证不代表 Windows SCM 或真实摄像头验收。

本补丁在 Mac 实际通过 53 项业务测试及 2 项权威协议测试、`cargo clippy --locked --all-targets -- -D warnings`、fmt、官方消费策略（11 个源文件）、WiX authoring 与 11 个 Linux 安装/失败恢复夹具。无链接路径测试使用规范化的 `TMPDIR=/private/tmp`，既有网络夹具仅访问本地 loopback。原生 Windows MSI/SCM/ACL 仍以本版最终 Source 的平台 CI 为准。

## 当前工程约束

摄像头扩展把 Client 组织为桌面包与 `crates/mobile-ffi` workspace，增加 Android/iOS 原生采集应用。
上述 0.5.3 历史发行验收不代替新增移动端验证。移动 FFI 仅在 ABI 入口保留两个小范围 unsafe 块；
TLS、RTSP RECORD、RTP 分片和采集分发使用安全 Rust。Linux 可以验证共享 Rust/JNI 编译及合成媒体，
Android APK、iOS Swift/Xcode 和实机摄像头分别以目标平台检查为准。

正式状态以 Git tag、最终 Source 工作流和 Release 产物为准。Rust 1.99.0 是截至 2026-10-07 的当前正式版；Tokio 选择稳定的 ~1.53.2，兼容补丁由根 Cargo.lock 锁定。unsafe function 内的原始解引用和 foreign 调用必须放进显式 unsafe 块（unsafe_op_in_unsafe_fn = deny）。这项约束检查操作边界，不替代原生 ABI、权限与生命周期验证。正式输入和用户数据身份分开记录，不通过发行号推导持久状态。

## 统一规范验收边界

| 适用条款 | 当前实现与本轮验收 | 真实限制 |
|---|---|---|
| 2–5、19：职责、目录与身份 | 单Rust包保留 src 的摄像头适配、命令ledger、CLI及平台服务职责；config、protocol、packaging、tests、docs各有统一用途。edge v1定义来自固定Server契约及hash校验，与1.0.0软件号和本地format分开。 | Client不含Web或Server运行库；中性日志是编译leaf。 |
| 6–9、11–14、17：安全、命令和摄像头 | 秘密由stdin/受保护配置进入，授权与实际PTZ/RTSP/ONVIF能力分别核验；不做授权外的探测副作用。异步命令先持久intent；未确认PTZ报告unknown，重连/重复交付不第二次驱动摄像头。ffmpeg/ffprobe有超时/输出/录像预算。 | 外部设备适配由实际协议能力决定；厂商实机、真实RTSP与ONVIF环境需另外验收。 |
| 15：日志 | typed产品事件携带instance/task身份；过滤与轮转消费公共实现，不把摄像头凭据写入日志。 | ffmpeg外部进程参数包含RTSP凭据，既有实际边界在README公开；不虚称外部进程参数已隐藏。 |
| 20–23：验证与发行 | Mac 70个常规Rust用例、2个真实合成媒体用例与strict Clippy通过；Linux安装器11个成功/失败/恢复夹具和Windows WiX authoring静态检查通过。最终包按Source、hash、产品版本验收。 | shell/WiX模拟不代替Linux系统服务、Windows实际MSI/SCM/ACL及摄像头设备路径。 |

## 1.0.0 更新

消费 Foundation Client 0.10.5 的有界子进程回收修复：继承屏蔽的 SIGCHLD 不再令已经退出的
FFmpeg/FFprobe 耗尽捕获期限。Foundation Server 0.11.5 的中立日志保持 0.11.4 的 Windows
日志存储策略；Xcoc 的 LocalSystem 服务模式、日志名称和权限身份保持原有合同。

配对提交前通过共享 ConfigurationDirectory 校验 Unix 父路径，安全路径被拒绝时尚未发送远端请求。
测试仅规范化临时根目录，不规范化用户提供的配置路径或放宽 no-follow 检查。移动目标不再引入桌面
CLI，媒体脚本分别校验 Linux x86_64 与 macOS ARM64 的固定 companion。上述产品改动不增加 unsafe。

本地重新验证记录见 [摄像头验证记录](camera-validation.md)；正式发布以 1.0.0 最终提交的各平台 CI、
实际 MSI/SCM 生命周期及发行附件回下载结果为准，不把模拟设备结果视为真实摄像头认证。
