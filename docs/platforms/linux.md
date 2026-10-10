# Linux x86_64（systemd）：安装、构建与维护

适用于 Linux x86_64 的 systemd 部署。默认配置为 `/etc/isarmg/xcoc/config.json`，录像为 `/var/lib/isarmg/xcoc/recordings`。

当前源码版本为 **1.1.0**；截至 2026-10-10，公开 Release 仍是 v1.0.0。下列 1.1.0 安装命令用于取得同版构建产物之后；先核对[发行页](https://github.com/isarmg/xcoc/releases)，未发布时使用本页的源码构建。

安装前准备服务端 HTTPS 根地址和实例授权码，并核对[共同部署准备](../platform-setup.md#部署前准备)。配对字段和摄像头 JSON 统一见[配置指南](../configuration.md)，业务状态与录像排障见[运行维护](../operations.md)。

## 安装与维护

### 1. 安装媒体工具和客户端

网络摄像头只需要客户端内置媒体运行时。内置/USB 摄像头另装外部采集命令，例如 Debian/Ubuntu
使用 `sudo apt update && sudo apt install ffmpeg`，并核对 `ffmpeg -version`。这套本地采集工具不替代
源码构建所需的链接库，二者的依赖边界见[媒体运行时](../media-worker.md)。

```sh
# 核对架构；本机应为 x86_64。
uname -m
# 计算压缩包哈希，人工对比同版 SHA256SUMS 中该文件的行。
sha256sum ./xcoc-linux-x86_64.tar.gz
# 创建本次解压目录，保留发行包中的 target/release 与 packaging 相对布局。
mkdir xcoc-1.1.0-linux
tar -xzf ./xcoc-linux-x86_64.tar.gz -C xcoc-1.1.0-linux
cd xcoc-1.1.0-linux
# 安装程序到 /usr/local/bin 和 unit 到 /etc/systemd/system，登记并立即启动服务。
sudo sh packaging/linux/install.sh
# 确认 Client 版本、内置媒体运行时及当前服务状态。
xcoc --version
xcoc media-worker --check
systemctl status xcoc.service
```

安装脚本只询问是否开机自启，按 Enter 默认为 Yes；选择 No 仍立即启动本次服务。首次未配对时服务等待配置。脚本先对待安装二进制执行无凭据的媒体运行时检查。内置/USB 摄像头使用的 `ffmpeg` 须位于 `/usr/local/bin:/usr/bin:/bin`；网络摄像头不要求该命令。当前 unit 未指定独立 User，按 systemd 默认使用 root。

### 2. 首次配对、摄像头配置与验收

```sh
# 停止服务，避免与配对/配置写入同时运行。
sudo systemctl stop xcoc.service
# 交互输入 Server、授权码和名称，然后发现/录入摄像头并用内置媒体库实际验证。
sudo xcoc setup --interactive
# 查看配对实例 UUID，以及不含摄像头凭据的配置摘要。
sudo xcoc status
sudo xcoc camera list
# 启动 systemd 服务并核对运行状态。
sudo systemctl start xcoc.service
systemctl status xcoc.service
# 核对是否启用开机自启；与当前运行状态是两项检查。
systemctl is-enabled xcoc.service
```

如果配对已保存但摄像头验证失败，保留输出的 UUID；按配置指南准备受保护 JSON，再 `camera apply --instance-id INSTANCE_UUID --input-stdin`，不必重新配对。Linux 归档不包含 `config/camera.json.example`，可从配置指南复制 JSON 字段，不要引用压缩包内不存在的模板。

在服务端检查该实例在线、快照更新、实时画面可播放；服务端录像模式还需有新录像索引，本地录像模式需看到 15 分钟 MP4 分段持续生成。单看 service active 不足以判断摄像头可用。

### 3. 重新配对

同一实例轮换授权码后，`setup --input-stdin` 可更新 token 并保留已有摄像头；`setup --interactive` 会继续摄像头配置步骤，完成后应核对原设置。

```sh
# 停服并读取当前实例。
sudo systemctl stop xcoc.service
sudo xcoc status
# 使用同一实例的新授权码重新配对，并核对摄像头。
sudo xcoc setup --interactive
sudo xcoc camera list
# 保存成功后启动，去 Server 验收画面。
sudo systemctl start xcoc.service
```

仅在 `pairing_state_incompatible` 时使用 `sudo xcoc setup --interactive --replace`，它验证重要录像状态后归档不兼容账户文件。`configuration_state_incompatible` 或 `important_state_incompatible` 不会被此选项绕过。

服务端删除旧实例后创建新实例时，停服后先 `sudo xcoc unpair OLD_INSTANCE_UUID`，再运行新实例的 `setup`。`unpair` 移除指定本机配对及该实例摄像头配置，保留其他实例和已存录像；不会撤销服务端授权。删除最后一份配对时本机 config 文件会被移除。

### 4. 服务查看、启停与自启

下面是独立操作，按需选择：

```sh
# 查看运行状态和服务定义，q 退出 status 分页器。
systemctl status xcoc.service
systemctl cat xcoc.service
# 立即启动、停止、重启，不修改自启策略。
sudo systemctl start xcoc.service
sudo systemctl stop xcoc.service
sudo systemctl restart xcoc.service
# 启用或取消开机自启，不改变当前进程状态。
sudo systemctl enable xcoc.service
sudo systemctl disable xcoc.service
# 同时取消自启并停止服务。
sudo systemctl disable --now xcoc.service
```

### 5. 诊断

```sh
# 核对安装后的本机配对与摄像头摘要。
sudo xcoc status
sudo xcoc camera list
# 核对内置媒体库；仅内置/USB 摄像头额外检查 unit PATH 中的采集命令。
/usr/local/bin/xcoc media-worker --check
env PATH=/usr/local/bin:/usr/bin:/bin ffmpeg -version
# 发现可达网段的 ONVIF 设备，最长等待 3 秒；为空不代表 RTSP 不可用。
sudo xcoc camera discover --timeout-seconds 3
# 查看最近 100 条服务日志；实时跟踪用 -f，Ctrl+C 只停止查看。
sudo journalctl -u xcoc.service -n 100 --no-pager
sudo journalctl -u xcoc.service -f
```

发现失败核对网段和 UDP 3702；有摄像头配置却无画面时，核对本机码流验证、摄像头凭据、RTSPS 证书及发布端口。日志中不公开原始 RTSP URL、token 或配置文件。

### 6. 替换程序与卸载

升级重新解压已校验的新版本并运行安装脚本。脚本备份旧程序/unit，失败时尝试回滚；本次自启选择会生效，升级后再次核对。没有 DEB/RPM，不使用 `apt remove xcoc` 卸载。

```sh
# 先停止服务并取消开机自启，确认 inactive 后才移除固定安装文件。
sudo systemctl disable --now xcoc.service
systemctl is-active xcoc.service
# 仅在前一项确认服务不再运行后执行；移除程序/unit，保留配对配置、录像和命令恢复证据。
sudo rm /usr/local/bin/xcoc /etc/systemd/system/xcoc.service
# 重载 systemd，使已删除的 unit 定义退出登记。
sudo systemctl daemon-reload
# 验收 LoadState 不再 loaded；保留状态目录是预期行为。
systemctl show xcoc.service --property=LoadState,ActiveState
```

正式退役还需在服务端取消/退役相应实例。本产品没有自动清除全部录像与命令记录的 purge 命令；确认保留/归档策略后再单独管理数据，普通卸载不删除它们。

## 从源码构建

在仓库根目录执行。共享的媒体版本、静态链接要求与许可证见[媒体运行时](../media-worker.md#原生构建依赖)。

需要 Rust 1.99.0、C 编译器、make、curl、pkg-config、Python 3、tar/xz 和 OpenSSL 开发包及
静态库。以 Debian/Ubuntu 构建机为例：

```sh
sudo apt update
sudo apt install build-essential curl pkg-config python3 xz-utils libssl-dev binutils
bash packaging/native/build-unix.sh "$PWD/target/native-media"
export PKG_CONFIG_PATH="$PWD/target/native-media/lib/pkgconfig"
cargo +1.99.0 build --locked --release
python3 packaging/native/check-runtime.py ./target/release/xcoc
```

脚本固定并校验 FFmpeg 源码，禁用外部自动发现、程序、GPL/nonfree 组件和不需要的库，静态链接
OpenSSL。发行审计须确认不存在额外 `libav*` 或 OpenSSL 动态依赖；安装构建机的旧版 `ffmpeg`
命令不能替代此步骤。构建机的系统 C 库基线仍约束发行物能运行的 Linux 版本。

### 安装本机源码构建结果

上面的构建成功后，在同一仓库根目录直接运行安装脚本，无需先制作 tar 包：

```sh
sudo sh packaging/linux/install.sh
xcoc --version
xcoc media-worker --check
systemctl status xcoc.service
```

随后继续本页[首次配对](#2-首次配对摄像头配置与验收)。对外分发还需完成[发行校验与依赖材料](../media-worker.md#发行与验证边界)。

## 下一步

[日常使用](../usage.md) · [共同排障](../operations.md) · [选择其他平台](../README.md#选择平台)
