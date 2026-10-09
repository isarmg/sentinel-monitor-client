# xcoc 分平台部署与维护

适用于 `xcoc 1.0.0`。按平台完成安装、配对、摄像头配置、验收，再按需要重新配对、管理服务或卸载。摄像头 JSON 与协议说明见[配置指南](configuration.md)，录像与故障边界见[运维文档](operations.md)。

## 部署前准备

1. 在 [Client Releases](https://github.com/isarmg/xcoc/releases) 下载目标版本的本机平台资产和 `SHA256SUMS`。Linux/macOS 资产名不包含版本号，必须确认所在 Release 的版本。Windows 正式安装用 MSI，ZIP 仅用于手工运行。
2. 请 Server 管理员创建摄像头实例，提供 HTTPS 根地址、实例授权码。桌面一次安装可管理多个实例，每个实例对应一台摄像机；移动端只管理一份配对和一个主码流。
3. 桌面安装 FFmpeg 和同套 FFprobe。后台服务的 PATH 中必须也能找到它们；仅在当前用户终端可用还不够。本机摄像头还需对应采集 backend 和 libx264，见[兼容说明](camera-support.md)。手机使用系统摄像头/编码器，无需安装 FFmpeg。
4. Server HTTPS 与发布用 RTSPS 证书都必须可信且名称匹配；摄像头 RTSP 可在本地网络内使用。授权码在桌面交互提示中明文回显，摄像头密码隐藏输入。
5. 下文注释解释命令用途。`INSTANCE_UUID`、`OLD_INSTANCE_UUID` 替换为实际实例 UUID；不要将授权码、摄像头密码或媒体 token 放在命令参数中。

| 平台 | 发行安装方式 | 服务与默认配置 | 本地录像 |
|---|---|---|---|
| Linux x86_64 | tar.gz 内 systemd 安装脚本 | `xcoc.service`；`/etc/isarmg/xcoc/config.json` | `/var/lib/isarmg/xcoc/recordings` |
| Windows x64 | MSI | SCM `XcocClient`（LocalSystem）；`C:\ProgramData\XcocClient\config.json` | `C:\ProgramData\XcocClient\recordings` |
| macOS Apple Silicon | tar.gz，手工部署 | 无随包安装器；默认配置 `/Library/Application Support/XcocClient/config.json` | `/Library/Application Support/XcocClient/recordings` |
| Android / iOS | 原生应用，按移动指南构建/签名安装 | 应用私有存储及系统安全存储 | 当前移动端在 Server 录像 |

`status` 和 `camera list` 是本机只读视图；服务运行和本机已配对不能代替 Server 的画面/录像验收。Xcoc 没有 Host/Sunshine 的 `pair recover`、`doctor` 或通用 `service start/stop` 子命令，不应复制其他产品的命令。

## Linux x86_64（systemd）

### 1. 安装媒体工具和 Client

以下安装媒体工具以 Debian/Ubuntu 为例；其他发行版用本机包管理器安装 `ffmpeg`，再核对工具路径。

```sh
# 更新 APT 包索引，并安装提供 ffmpeg/ffprobe 的媒体工具套件。
sudo apt update
sudo apt install ffmpeg
# 核对架构和工具版本；本机应为 x86_64。
uname -m
ffmpeg -version
ffprobe -version
# 计算压缩包哈希，人工对比同版 SHA256SUMS 中该文件的行。
sha256sum ./xcoc-linux-x86_64.tar.gz
# 创建本次解压目录，保留发行包中的 target/release 与 packaging 相对布局。
mkdir xcoc-1.0.0-linux
tar -xzf ./xcoc-linux-x86_64.tar.gz -C xcoc-1.0.0-linux
cd xcoc-1.0.0-linux
# 安装程序到 /usr/local/bin 和 unit 到 /etc/systemd/system，登记并立即启动服务。
sudo sh packaging/linux/install.sh
# 确认 Client 版本及当前服务状态。
xcoc --version
systemctl status xcoc.service
```

安装脚本只询问是否开机自启，按 Enter 默认为 Yes；选择 No 仍立即启动本次服务。首次未配对时服务等待配置。脚本要求 FFmpeg/FFprobe 位于 `/usr/local/bin:/usr/bin:/bin`，并以系统服务身份运行，当前 unit 未指定独立 User，按 systemd 默认使用 root。

### 2. 首次配对、摄像头配置与验收

```sh
# 停止服务，避免与配对/配置写入同时运行。
sudo systemctl stop xcoc.service
# 交互输入 Server、授权码和名称，然后发现/录入摄像头并用 ffprobe 实际验证。
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

在 Server 检查该实例在线、快照更新、实时画面可播放；Server 录像模式还需有新录像索引，本地录像模式需看到 15 分钟 MP4 分段持续生成。单看 service active 不足以判断摄像头可用。

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

Server 删除旧实例后创建新实例时，停服后先 `sudo xcoc unpair OLD_INSTANCE_UUID`，再运行新实例的 `setup`。`unpair` 移除指定本机配对及该实例摄像头配置，保留其他实例和已存录像；不会撤销 Server 授权。删除最后一份配对时本机 config 文件会被移除。

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
# 模拟 unit 的 PATH，核对后台确实能找到工具。
env PATH=/usr/local/bin:/usr/bin:/bin ffmpeg -version
env PATH=/usr/local/bin:/usr/bin:/bin ffprobe -version
# 发现可达网段的 ONVIF 设备，最长等待 3 秒；为空不代表 RTSP 不可用。
sudo xcoc camera discover --timeout-seconds 3
# 查看最近 100 条服务日志；实时跟踪用 -f，Ctrl+C 只停止查看。
sudo journalctl -u xcoc.service -n 100 --no-pager
sudo journalctl -u xcoc.service -f
```

发现失败核对网段和 UDP 3702；有摄像头配置却无画面时，核对本机码流验证、摄像头凭据、RTSPS 证书及发布端口。日志中不公开原始 RTSP URL、token 或配置文件。

### 6. 升级与卸载

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

正式退役还需在 Server 取消/退役相应实例。本产品没有自动清除全部录像与命令记录的 purge 命令；确认保留/归档策略后再单独管理数据，普通卸载不删除它们。

## Windows x64

### 1. 安装

先安装本机可用、支持所需采集 backend 的 Windows x64 FFmpeg 发行套件：

1. 将 FFmpeg 套件解压到固定目录，找到同一 `bin` 目录中的 `ffmpeg.exe` 和 `ffprobe.exe`，不要放在会被临时清理的目录。
2. 在系统“高级系统设置 → 环境变量 → 系统变量 → Path → 编辑”中新增该 `bin` 的完整路径，确认保存。不要只修改当前用户 PATH，LocalSystem 服务需要系统配置。
3. 重新打开管理员 PowerShell，运行下方版本命令。若已运行服务后才修改系统环境，重启机器确保服务继承新环境，再继续设置。

```powershell
# 检查媒体工具可从当前系统配置找到；服务也需要同套工具。
ffmpeg -version
ffprobe -version
# 校验 MSI，与同版 SHA256SUMS 对比。
Get-FileHash .\xcoc-1.0.0-windows-x64.msi -Algorithm SHA256
# 安装程序和 XcocClient 服务，等待向导退出并写入安装日志。
$msi = (Resolve-Path .\xcoc-1.0.0-windows-x64.msi).Path
$install = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /norestart /l*v `"$env:TEMP\xcoc-install.log`"" -Wait -PassThru
# 0 成功；3010 表示成功但需重启；其他码先查看日志。
$install.ExitCode
```

MSI 可选择程序目录，全部功能一起安装；服务立即启动并等待配置。成功后执行：

```powershell
# 从注册表取真实安装路径，避免当前终端的 PATH 尚未更新。
$installRoot = (Get-ItemProperty 'HKLM:\Software\sarmg\xcoc').InstallLocation
$client = Join-Path $installRoot 'xcoc.exe'
# 确认版本、服务和登记路径；& 执行变量中的程序。
& $client --version
Get-Service -Name XcocClient
sc.exe qc XcocClient
```

### 2. 首次配对与验收

```powershell
# 先停止现有服务，为配对/摄像头配置写入提供独占窗口。
Stop-Service -Name XcocClient
# 完成 Server 配对、摄像头输入和媒体验证；默认自启 Yes，保存后配置并启动系统服务。
& $client setup --interactive
# 核对本机配对和摄像头摘要，再查看服务状态。
& $client status
& $client camera list
Get-Service -Name XcocClient
```

如果配对已保存、系统服务启动失败，修复工具路径或服务问题后执行 `& $client service` 继续，不需要新授权码。此前选择不开机自启则执行 `& $client service --no-boot-start`，它仍会立即启动，只将开机策略设为手动。Server 验收同 Linux：新快照、实时画面和录像。

### 3. 重新配对

```powershell
# 停服并核对旧实例，避免将新授权码用于错误的摄像头。
Stop-Service -Name XcocClient
& $client status
# 用同一实例的新授权码重新配对，继续核对摄像头，成功后系统服务自动启动。
& $client setup --interactive
& $client camera list
Get-Service -Name XcocClient
```

`pairing_state_incompatible` 使用 `& $client setup --interactive --replace`；切换到 Server 新建实例前使用 `& $client unpair OLD_INSTANCE_UUID`，保留录像。仅媒体验证失败时通过 `camera apply` 继续，不用授权码重做已经保存的配对。

### 4. 服务管理

以下按需单独执行：

```powershell
# 查看运行状态，sc.exe qc 查看开机类型与路径。
Get-Service -Name XcocClient
sc.exe qc XcocClient
# Windows 原生服务启停/重启。
Start-Service -Name XcocClient
Stop-Service -Name XcocClient
Restart-Service -Name XcocClient
# 分别设置自动/手动启动，不改变当前运行状态。
Set-Service -Name XcocClient -StartupType Automatic
Set-Service -Name XcocClient -StartupType Manual
# 使用已保存配对配置并启动服务，同时设为不开机自启。
& $client service --no-boot-start
```

### 5. 诊断

```powershell
# 检查配对、摄像头摘要和 ONVIF 发现。
& $client status
& $client camera list
& $client camera discover --timeout-seconds 3
# 查看最近 100 条后台持久日志；持续跟踪最多 60 秒，Ctrl+C 结束查看。
& $client logs --tail 100 --format json
& $client logs --follow --format ndjson --timeout 60s
# 查看服务状态/退出信息，辅助定位日志创建前的失败。
sc.exe query XcocClient
```

持久日志位于 `%ProgramData%\XcocClient\logs`，普通用户不可读。没有日志时同时查看 System/SCM 事件和安装日志。配置含 token，不用 `Get-Content config.json` 收集工单。

### 6. 升级、修复和卸载

本章后续维护若在新 PowerShell 会话中进行，先重新执行第 1 步读取注册表的 `$installRoot` / `$client` 两行。涉及修复或卸载时，再用 `$msi = (Resolve-Path .\xcoc-1.0.0-windows-x64.msi).Path` 指向与已安装产品相匹配的 MSI；文件在其他目录时用其实际完整路径。不要沿用指向另一版本包的变量。

校验新版 MSI 后重复安装。升级可能恢复开机自动启动，完成后核对 `sc.exe qc XcocClient`。同版修复和普通卸载分别如下，按需执行：

```powershell
# 重新安装所有组件，保留配置和录像；检查修复退出码。
$repair = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" REINSTALL=ALL REINSTALLMODE=amus /norestart /l*v `"$env:TEMP\xcoc-repair.log`"" -Wait -PassThru
$repair.ExitCode
# 卸载程序、系统服务和安装器拥有的 PATH 项；保留配对、录像和命令记录。
$remove = Start-Process msiexec.exe -ArgumentList "/x `"$msi`" /norestart /l*v `"$env:TEMP\xcoc-uninstall.log`"" -Wait -PassThru
$remove.ExitCode
# 验收服务已消失；没有返回对象是预期结果。
Get-Service -Name XcocClient -ErrorAction SilentlyContinue
```

正式退役还需在 Server 取消实例；MSI 不提供业务数据 purge。保留 `%ProgramData%\XcocClient` 是预期行为。

## macOS Apple Silicon

### 1. 手工安装与前台验证

下文通过 `sudo env PATH=...` 明确给管理命令提供与后台相同的媒体工具路径，避免 sudo 重置 PATH 后找不到 Homebrew 的 FFmpeg。

当前 Release 只提供 arm64 二进制压缩包，没有 PKG、安装/卸载助手或已登记的 LaunchDaemon。先安装可信 FFmpeg 套件；已经使用 Homebrew 的机器可执行 `brew install ffmpeg`。手工安装步骤：

```sh
# 确认 arm64，以及 ffmpeg/ffprobe 可用。
uname -m
ffmpeg -version
ffprobe -version
# 对比同版 SHA256SUMS 中的归档哈希，再解压到独立目录。
shasum -a 256 ./xcoc-macos-arm64.tar.gz
mkdir xcoc-1.0.0-macos
tar -xzf ./xcoc-macos-arm64.tar.gz -C xcoc-1.0.0-macos
# 创建程序目录，将归档中的实际二进制安装到固定路径。
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 xcoc-1.0.0-macos/target/release/xcoc /usr/local/bin/xcoc
# 核对安装版本，完成 Server 配对和摄像头探测。
/usr/local/bin/xcoc --version
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc setup --interactive
# 查看配对和摄像头摘要；前台启动发布/录像，Ctrl+C 优雅停止。
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc status
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc camera list
sudo env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/xcoc run
```

前台运行时在 Server 验收画面与录像。用内置/USB 摄像头时还需在实际登录会话通过系统摄像头权限及采集验证；以下系统级后台示例适合网络 RTSP/ONVIF 摄像头，不能替代本机摄像头权限验收。

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

### 4. 诊断、升级与卸载

```sh
# 使用与 plist 相同的 PATH 核对服务工具。
env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin ffmpeg -version
env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin ffprobe -version
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

仅前台部署只需在停止后删除 `/usr/local/bin/xcoc`。配置、录像、命令记录和日志保留；没有包收据需要清理。正式退役还需 Server 撤销实例。

## Android

### 1. 构建/安装

当前桌面 Release 不提供可直接依赖的正式 Android 签名包。按[移动端指南](mobile.md#android-构建)准备 SDK/NDK/JDK、构建 Rust 库和 APK；构建命令的作用见本文最后的命令说明。调试安装以开发机已有 ADB 和 arm64 真机为前提：

```sh
# 列出 USB 调试设备，手机确认授权后应显示 device；多设备时每条 adb 加 -s 实际序列号。
adb devices
# 覆盖安装同签名 APK并保留应用数据；不可将 Debug 当作正式包升级。
adb install -r clients/android/app/build/outputs/apk/debug/app-debug.apk
# 打开 Xcoc Camera 主界面。
adb shell am start -n org.sarmg.xcoc/.MainActivity
```

### 2. 配对、启动、查看和停止

在 Server 创建实例后，在应用依次填写 HTTPS 根地址、授权码和摄像头名称，点“配对”。授予系统摄像头权限，选择前置/后置摄像头，点“启动摄像头”。界面和常驻通知显示状态；Server 应看到新快照并能播放视频。

点界面或通知的“停止摄像头”停止采集；再次启动使用已保存配对。Android 使用前台摄像头服务，不提供桌面 systemd/SCM 服务命令，不能假定重启手机后自动开机采集。

### 3. 重新配对、诊断、卸载

先停止摄像头，请管理员提供新授权码，再在相同 Server 地址下点“配对”，成功后重新启动并验证画面。更换实例时核对目标实例，手机只保存一份配对。

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

也可在系统应用信息页卸载。先核对相机权限、通知/前台限制、网络、手机时间及 HTTPS/RTSPS 证书；没有跳过证书验证或导入私有 CA 的界面。卸载不删除 Server 录像，也不代替撤销 Server 实例。

## iOS

### 1. 安装

最低 iOS 16，当前没有可直接安装的正式签名 IPA。按[移动端指南](mobile.md#ios-构建)在 macOS 构建 Rust XCFramework，以 XcodeGen 生成 `XcocCamera.xcodeproj`，在 Xcode 选择自己的开发团队和已连接的真机，配置签名后运行。模拟器可验证构建/协议测试，实际摄像头采集必须在真机验收。

### 2. 配对、启动、查看和停止

打开应用，输入 Server HTTPS 根地址、实例授权码、名称，点“配对”。允许摄像头权限，选择前/后摄像头，点“启动摄像头”；状态在应用界面查看，到 Server 验证画面和录像。点“停止摄像头”即可结束。**进入后台会停止采集，回到前台需手动再次启动**，没有 iOS 后台常驻摄像头服务。

### 3. 重新配对、诊断和卸载

先停摄像头，用 Server 新授权码重新配对，再启动核验。权限拒绝时从系统应用设置恢复；采集失败/离线时检查设备网络、证书、相机占用和界面状态。实机日志通过 Xcode 的设备控制台查看，保留必要的脱敏错误信息；不要公开 token 或发布地址。

卸载前停止摄像头，在 Server 撤销/退役实例；使用系统“删除 App”而非仅移除主屏幕图标。应用私有数据会被删除，Keychain 项可能由系统保留，不能把重装视为凭据已撤销；重新安装后按界面实际状态和 Server 当前授权重新配对。Server 录像由 Server 保留策略管理。

## 补充：移动端构建命令的作用

| 命令/脚本 | 用途 |
|---|---|
| `rustup target add --toolchain 1.99.0 ...` | 为指定 Rust 工具链安装 Android/iOS/模拟器目标标准库 |
| `cargo +1.99.0 install cargo-ndk --version 4.1.2 --locked` | 安装固定版 Android Rust 构建辅助工具 |
| `export ANDROID_NDK_HOME=/absolute/path/...` | 指向本机已安装的 NDK，路径需要自行替换 |
| `bash scripts/build-android-rust.sh` | 编译 arm64 Rust/JNI 库供 Android 工程使用 |
| `gradle -p clients/android testDebugUnitTest assembleDebug` | 运行 JVM 单元测试并构建 Debug APK，不授予正式签名 |
| `bash scripts/build-ios-rust.sh` | 构建 iOS 实机/模拟器 Rust 库并打包 XCFramework |
| `xcodegen generate` | 在 `clients/ios` 中按 project.yml 生成 Xcode 工程 |
| `xcodebuild ... test` | 构建并运行指定目标测试，不等于真机摄像头与发布验收 |

更多错误码、录像数据兼容性和摄像头逐项配置见[运维文档](operations.md)与[配置指南](configuration.md)。
