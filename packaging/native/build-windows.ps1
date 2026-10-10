[CmdletBinding()]
param([Parameter(Mandatory = $true)][string] $WorkDirectory)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$revision = 'e456309491fd875487bf02b8a0dca76c1b1b00cc'
$root = [IO.Path]::GetFullPath($WorkDirectory)
$vcpkg = Join-Path $root 'vcpkg'
$installed = Join-Path $root 'installed'
New-Item -ItemType Directory -Force -Path $root | Out-Null
if (-not (Test-Path -LiteralPath (Join-Path $vcpkg '.git'))) {
    & git init $vcpkg
    if ($LASTEXITCODE) { throw 'Could not initialize the vcpkg checkout.' }
    & git -C $vcpkg remote add origin https://github.com/microsoft/vcpkg.git
    if ($LASTEXITCODE) { throw 'Could not set the official vcpkg source.' }
}
& git -C $vcpkg fetch --depth 1 origin $revision
if ($LASTEXITCODE) { throw 'Could not fetch the reviewed vcpkg revision.' }
& git -C $vcpkg checkout --detach $revision
if ($LASTEXITCODE) { throw 'Could not check out the reviewed vcpkg revision.' }
if ((& git -C $vcpkg rev-parse HEAD) -ne $revision) { throw 'Unexpected vcpkg revision.' }
& git -C $vcpkg diff --exit-code HEAD --
if ($LASTEXITCODE) { throw 'The vcpkg checkout has unreviewed changes.' }
& (Join-Path $vcpkg 'bootstrap-vcpkg.bat') -disableMetrics
if ($LASTEXITCODE) { throw 'vcpkg bootstrap failed.' }
& (Join-Path $vcpkg 'vcpkg.exe') install "--x-manifest-root=$PSScriptRoot" "--x-install-root=$installed" --triplet x64-windows-static --host-triplet x64-windows-static --disable-metrics
if ($LASTEXITCODE) { throw 'Static FFmpeg build failed.' }
$env:VCPKG_ROOT = $vcpkg
$env:VCPKG_INSTALLED_ROOT = $installed
$env:VCPKGRS_TRIPLET = 'x64-windows-static'
$env:RUSTFLAGS = '-C target-feature=+crt-static'
$env:PKG_CONFIG = Join-Path $installed 'x64-windows-static\tools\pkgconf\pkgconf.exe'
$env:PKG_CONFIG_PATH = Join-Path $installed 'x64-windows-static\lib\pkgconfig'
if (-not (Test-Path -LiteralPath $env:PKG_CONFIG)) { throw 'The reviewed pkgconf host tool was not installed.' }
& $env:PKG_CONFIG --modversion libavformat libavcodec libavutil
if ($LASTEXITCODE) { throw 'FFmpeg pkg-config metadata verification failed.' }
# Export the same contract to subsequent CI steps; dot-source locally when the
# variables should remain in the caller's PowerShell session.
if ($env:GITHUB_ENV) {
    @("VCPKG_ROOT=$vcpkg", "VCPKG_INSTALLED_ROOT=$installed", 'VCPKGRS_TRIPLET=x64-windows-static', 'RUSTFLAGS=-C target-feature=+crt-static', "PKG_CONFIG=$env:PKG_CONFIG", "PKG_CONFIG_PATH=$env:PKG_CONFIG_PATH") |
        Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
Write-Host "FFmpeg 9.0.2 built with static libraries and static CRT in $installed"
