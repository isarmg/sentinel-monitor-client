# macOS Apple Silicon：安装、构建与维护

适用于 Apple Silicon（arm64）Mac。默认配置为 `/Library/Application Support/XcocClient/config.json`，录像位于同级 `recordings`。客户端采用手工安装，可先前台运行，再选择配置 launchd。

当前源码版本为 **1.1.0**；截至 2026-10-10，公开 Release 仍是 v1.0.0。下列 1.1.0 安装命令用于取得同版构建产物之后；先核对[发行页](https://github.com/isarmg/xcoc/releases)，未发布时使用本页的源码构建。

安装前准备服务端 HTTPS 根地址和实例授权码，并核对[共同部署准备](../platform-setup.md#部署前准备)。配对字段和摄像头 JSON 统一见[配置指南](../configuration.md)，业务状态与录像排障见[运行维护](../operations.md)。

## 安装与维护

### 1. 手工安装与前台验证

下文通过 `sudo env PATH=...` 明确给管理命令提供与后台相同的媒体工具路径，避免 sudo 重置 PATH 后找不到 Homebrew 的 FFmpeg。

macOS 发行格式为 arm64 二进制压缩包，没有 PKG、安装/卸载助手或已登记的 LaunchDaemon。网络摄像头使用内置媒体运行时；内置/USB 摄像头另装支持 AVFoundation 与 `libx264` 的可信 FFmpeg 采集工具，使用 Homebrew 的机器可执行 `brew install ffmpeg`。手工安装步骤：

```sh
# 确认 arm64；仅内置/USB 摄像头另检查 ffmpeg -version。
uname -m
# 对比同版 SHA256SUMS 中的归档哈希，再解压到独立目录。
shasum -a 256 ./xcoc-macos-arm64.tar.gz
mkdir xcoc-1.1.0-macos
tar -xzf ./xcoc-macos-arm64.tar.gz -C xcoc-1.1.0-macos
# 创建程序目录，将归档中的实际二进制安装到固定路径。
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 xcoc-1.1.0-macos/target/release/xcoc /usr/local/bin/xcoc
# 核对安装版本，完成 Server 配对和摄像头探测。
/usr/local/bin/xcoc --version
/usr/local/bin/xcoc media-worker --check
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc setup --interactive
# 查看配对和摄像头摘要；前台启动发布/录像，Ctrl+C 优雅停止。
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc status
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc camera list
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc run
```

前台运行时在服务端验收画面与录像。用内置/USB 摄像头时还需在实际登录会话通过系统摄像头权限及采集验证；以下系统级后台示例适合网络 RTSP/ONVIF 摄像头，不能替代本机摄像头权限验收。

### 2. 可选：手工登记后台服务

下面的 `org.sarmg.xcoc` 是**本文手工部署示例定义的 label**，不是随包服务。先停止前台 `run`。在文本编辑器中将以下内容保存为当前目录的 `org.sarmg.xcoc.plist`：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>org.sarmg.xcoc</string>
  <key>ProgramArguments</key><array>
    <string>/usr/local/bin/xcoc</string>
    <string>--config</string><string>/Library/Application Support/XcocClient/config.json</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key><dict>
    <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>5</integer>
  <key>Umask</key><integer>63</integer>
  <key>StandardOutPath</key><string>/var/log/xcoc.log</string>
  <key>StandardErrorPath</key><string>/var/log/xcoc.log</string>
</dict></plist>
```

`ProgramArguments` 指向程序、配置和常驻 `run`；PATH 包含 Apple Silicon Homebrew 常用工具位置，实际工具在其他位置时先修改它。`RunAtLoad` 随装载启动，`KeepAlive` 在异常退出后恢复，`Umask=63` 即 0077。示例以系统默认 root 身份运行，并将日志写到固定受保护文件。

```sh
# 校验 plist 的 XML/属性列表语法，失败时先修正文件。
plutil -lint ./org.sarmg.xcoc.plist
# 安装为 root 拥有且普通用户不可写的系统 LaunchDaemon。
sudo install -o root -g wheel -m 0644 ./org.sarmg.xcoc.plist /Library/LaunchDaemons/org.sarmg.xcoc.plist
# 首次创建受保护日志文件；已有日志时不要执行此创建命令覆盖它。
sudo install -o root -g wheel -m 0600 /dev/null /var/log/xcoc.log
# 允许开机装载，然后注册并立即运行当前 job。
sudo launchctl enable system/org.sarmg.xcoc
sudo launchctl bootstrap system /Library/LaunchDaemons/org.sarmg.xcoc.plist
# 查看原生 job 状态、pid 和退出信息。
sudo launchctl print system/org.sarmg.xcoc
```

### 3. 重新配对与服务管理

已登记上述示例服务时，重新配对流程如下；仅前台运行时先 Ctrl+C 停止 `run`，省略 launchctl 操作。

```sh
# 卸载 job 并停止后台采集，避免同时写配置。
sudo launchctl bootout system/org.sarmg.xcoc
# 核对旧实例，用同一实例新授权码配对并核对摄像头配置。
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc status
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc setup --interactive
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc camera list
# 重新装载，立即启动，再到 Server 验收画面。
sudo launchctl bootstrap system /Library/LaunchDaemons/org.sarmg.xcoc.plist
```

状态不兼容使用 `setup --interactive --replace`；更换到新建实例先 `unpair OLD_INSTANCE_UUID`，然后 `setup`。均保留录像；不兼容配置/录像需另行诊断。

日常服务操作按需选择：

```sh
# 查看 job 当前状态；bootout 后找不到 job 表示已卸载。
sudo launchctl print system/org.sarmg.xcoc
# 停止并卸载当前 job，不改变开机策略。
sudo launchctl bootout system/org.sarmg.xcoc
# 开机策略允许且 job 未装载时：装载并启动。
sudo launchctl bootstrap system /Library/LaunchDaemons/org.sarmg.xcoc.plist
# 对已装载 job 强制重启，会中断当前媒体采集。
sudo launchctl kickstart -k system/org.sarmg.xcoc
# 分别允许/禁用以后自动装载；disable 不负责停止当前进程。
sudo launchctl enable system/org.sarmg.xcoc
sudo launchctl disable system/org.sarmg.xcoc
# 查看已持久保存的禁用策略。
sudo launchctl print-disabled system
```

禁用自启后如要再次 `bootstrap`，先 `enable`；完全暂停需先 `bootout` 再 `disable`。

### 4. 诊断和卸载

```sh
# 核对内置媒体库；仅内置/USB 摄像头额外检查 plist PATH 中的采集命令。
/usr/local/bin/xcoc media-worker --check
env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin ffmpeg -version
# 查看本机配对、摄像头摘要和 ONVIF 发现。
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc status
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc camera list
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc camera discover --timeout-seconds 3
# 手工服务的最近日志；-f 跟踪，Ctrl+C 只停止查看。
sudo tail -n 100 /var/log/xcoc.log
sudo tail -f /var/log/xcoc.log
```

前台模式直接查看终端输出。升级先停止前台/后台进程，校验新压缩包后重复安装二进制，再启动验收；不覆盖配置或录像。

卸载手工后台部署时：

```sh
# 对已装载 job 先停服，再禁用后续自动装载；若已 bootout，省略第一条。
sudo launchctl bootout system/org.sarmg.xcoc
sudo launchctl disable system/org.sarmg.xcoc
# 确认 print 已找不到 job 后，仅删除本示例创建的程序和 plist。
sudo launchctl print system/org.sarmg.xcoc
sudo rm /usr/local/bin/xcoc /Library/LaunchDaemons/org.sarmg.xcoc.plist
```

仅前台部署只需在停止后删除 `/usr/local/bin/xcoc`。配置、录像、命令记录和日志保留；没有包收据需要清理。正式退役还需服务端撤销实例。

## 从源码构建

在仓库根目录执行。共享的媒体版本、静态链接要求与许可证见[媒体运行时](../media-worker.md#原生构建依赖)。

需要 Apple Silicon、Xcode Command Line Tools、Rust 1.99.0、Python 3、pkg-config 与 xz。
已使用 Homebrew 的构建机可安装这些构建工具：

```sh
brew install pkg-config python xz
bash packaging/native/build-unix.sh "$PWD/target/native-media"
export PKG_CONFIG_PATH="$PWD/target/native-media/lib/pkgconfig"
cargo +1.99.0 build --locked --release
python3 packaging/native/check-runtime.py ./target/release/xcoc
```

脚本使用系统 SecureTransport，不链接 Homebrew FFmpeg/OpenSSL dylib。`brew install ffmpeg`
只用于外部本地设备采集，不是这个构建过程的必需步骤。部署仍为手工安装 arm64 tar 包；必须在
支持的 macOS 目标上检查实际系统框架和最低系统版本，不以 Linux 的构建结果代替。

### 安装本机源码构建结果

在仓库根目录将本机生成的二进制安装到固定位置；这一步替代前面的下载、解压与复制归档文件：

```sh
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 target/release/xcoc /usr/local/bin/xcoc
/usr/local/bin/xcoc --version
/usr/local/bin/xcoc media-worker --check
```

随后继续[前台配对与验证](#1-手工安装与前台验证)中的 `setup --interactive`，需要后台运行时再登记 launchd。对外分发还需完成[发行校验与依赖材料](../media-worker.md#发行与验证边界)。

## 下一步

[日常使用](../usage.md) · [共同排障](../operations.md) · [选择其他平台](../README.md#选择平台)
