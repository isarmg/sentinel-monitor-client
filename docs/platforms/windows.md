# Windows x64：安装、构建与维护

适用于 Windows x64。正式安装使用 MSI；SCM 服务名为 `XcocClient`，以 LocalSystem 运行。默认配置为 `C:\ProgramData\XcocClient\config.json`，录像位于同级 `recordings`。请在管理员 PowerShell 中执行安装和维护命令。

当前源码版本为 **1.1.0**；截至 2026-10-10，公开 Release 仍是 v1.0.0。下列 1.1.0 安装命令用于取得同版构建产物之后；先核对[发行页](https://github.com/isarmg/xcoc/releases)，未发布时使用本页的源码构建。

安装前准备服务端 HTTPS 根地址和实例授权码，并核对[共同部署准备](../platform-setup.md#部署前准备)。配对字段和摄像头 JSON 统一见[配置指南](../configuration.md)，业务状态与录像排障见[运行维护](../operations.md)。

## 安装与维护

### 1. 安装

网络摄像头使用客户端内置媒体运行时，不需要额外安装 `ffmpeg.exe` 或 `ffprobe.exe`。
只有内置/USB 摄像头需要本机可用、支持 DirectShow 与 `libx264` 的 Windows x64 FFmpeg 采集工具：

1. 将包含 `ffmpeg.exe` 的套件解压到固定目录，不要放在会被临时清理的目录。
2. 在实际采集账户的 PATH 中添加该 `bin` 的完整路径；如由 LocalSystem 服务采集，须使用系统变量 Path，不能只修改当前用户 PATH。
3. 重新打开对应账户的 PowerShell，用 `ffmpeg -version` 检查。若已运行服务后才修改系统环境，重启机器确保服务继承新环境。系统服务仍需单独验证摄像头权限。

```powershell
# 校验 MSI，与同版 SHA256SUMS 对比。
Get-FileHash .\xcoc-1.1.0-windows-x64.msi -Algorithm SHA256
# 安装程序和 XcocClient 服务，等待向导退出并写入安装日志。
$msi = (Resolve-Path .\xcoc-1.1.0-windows-x64.msi).Path
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
& $client media-worker --check
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

如果配对已保存、系统服务启动失败，修复工具路径或服务问题后执行 `& $client service` 继续，不需要新授权码。此前选择不开机自启则执行 `& $client service --no-boot-start`，它仍会立即启动，只将开机策略设为手动。到服务端确认新快照、实时画面和录像。

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

`pairing_state_incompatible` 使用 `& $client setup --interactive --replace`；切换到服务端新建实例前使用 `& $client unpair OLD_INSTANCE_UUID`，保留录像。仅媒体验证失败时通过 `camera apply` 继续，不用授权码重做已经保存的配对。

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

### 6. 修复安装和卸载

本章后续维护若在新 PowerShell 会话中进行，先重新执行第 1 步读取注册表的 `$installRoot` / `$client` 两行。涉及修复或卸载时，再用 `$msi = (Resolve-Path .\xcoc-1.1.0-windows-x64.msi).Path` 指向与已安装产品相匹配的 MSI；文件在其他目录时用其实际完整路径。不要沿用指向另一版本包的变量。

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

正式退役还需在服务端取消实例；MSI 不提供业务数据 purge。保留 `%ProgramData%\XcocClient` 是预期行为。

## 从源码构建

在仓库根目录执行。共享的媒体版本、静态链接要求与许可证见[媒体运行时](../media-worker.md#原生构建依赖)。

需要 Visual Studio C++ Build Tools/Windows SDK、Rust MSVC 工具链、PowerShell、Python 3 和 Git。
原生脚本从官方来源获取固定 vcpkg revision，使用 `x64-windows-static` triplet，只选择 FFmpeg 的
`avcodec`、`avformat` 功能与内置编解码器；Windows TLS 使用 Schannel。
Rust 构建必须同时设置 `RUSTFLAGS=-C target-feature=+crt-static`，与原生静态 CRT 保持一致。
不要混用动态 CRT triplet、另一个 vcpkg 安装树或任意下载的 FFmpeg DLL。

在 x64 Native Tools PowerShell 中执行：

```powershell
. .\packaging\native\build-windows.ps1 -WorkDirectory "$PWD\target\native-media"
cargo +1.99.0 build --locked --release
python packaging/native/check-runtime.py target/release/xcoc.exe
```

脚本设置 `VCPKG_ROOT`、`VCPKG_INSTALLED_ROOT`、`VCPKGRS_TRIPLET` 和 `RUSTFLAGS`；必须在
同一 PowerShell 会话中继续构建。依赖检查还会执行 `media-worker --check`。
MSI 和 ZIP 使用同一个静态媒体库构建的单文件客户端，发布前必须完成依赖审计；DirectShow 摄像头另需 `ffmpeg.exe` 与 `libx264`。

### 从本机二进制生成 MSI

另准备 .NET SDK。安装器工程会解析固定的 WiX `4.0.6` 依赖；本机首次构建需要能够取得这些构建依赖。在同一仓库根目录、同一 PowerShell 会话运行：

```powershell
# 本机构建设置临时工作根；脚本只清理其中自己创建的唯一子目录。
$env:RUNNER_TEMP = $env:TEMP
.\packaging\windows\build-installer.ps1 `
  -ClientExe "$PWD\target\release\xcoc.exe" `
  -Version "1.1.0" `
  -Output "$PWD\dist"
```

输出为 `dist\xcoc-1.1.0-windows-x64.msi`。按本页[安装](#1-安装)操作，将 `$msi` 指向该实际文件；本机产物尚没有 GitHub Release 的 `SHA256SUMS`，校验结果应与自己的构建记录对照。对外分发还需完成[发行校验与依赖材料](../media-worker.md#发行与验证边界)。

## 后台日志与权限

SCM 在私有状态校验后创建 `%ProgramData%/XcocClient/logs`，使用共享类型化的 sink 写入 `xcoc.jsonl`。最多保留活动文件和四份归档，每份 8 MiB，总上限 40 MiB。服务账户首次创建此目录；管理员查询不会先替服务建立日志目录。ACL 拒绝普通用户，已有不安全对象不修复。服务启动失败且日志输出器尚不可用时，可同时查看 Windows SCM 的服务退出代码。

日志按活动文件 `xcoc.jsonl` 和归档 `xcoc.jsonl.1` 至 `.4` 查询。Windows 服务使用 LocalSystem；目录仅授予 SYSTEM 和 Administrators 访问。

```powershell
xcoc logs --tail 100 --format json
xcoc logs --since 2026-10-07T00:00:00Z --level warn --format json
xcoc logs --follow --format ndjson --timeout 60s
```

可用 `--instance-id`、`--event`、`--request-id` 和 `--task-id` 精确筛选。后台整体启动事件使用 `scope=client`；属于某个已知实例的事件使用 `scope=instance` 并携带 `instance_id`。日志源损坏、超限或持续跟踪游标已从保留窗口移除时明确失败，避免把丢失记录显示为空成功。

## 下一步

[日常使用](../usage.md) · [共同排障](../operations.md) · [选择其他平台](../README.md#选择平台)
