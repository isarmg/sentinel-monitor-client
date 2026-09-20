[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $ClientExe,
    [Parameter(Mandatory = $true)][string] $Version,
    [Parameter(Mandatory = $true)][string] $Output
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$client = (Resolve-Path -LiteralPath $ClientExe).Path
$outputDirectory = [IO.Path]::GetFullPath($Output)
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$intermediate = Join-Path $env:RUNNER_TEMP ('sentinel-wix-' + [guid]::NewGuid())
try {
    & dotnet build (Join-Path $root 'packaging\windows\SentinelClient.wixproj') `
        -c Release "-p:ClientExe=$client" "-p:ProductVersion=$Version" `
        "-p:BaseIntermediateOutputPath=$intermediate\obj\" "-p:OutputPath=$outputDirectory\"
    if ($LASTEXITCODE) { throw "WiX build failed with exit code $LASTEXITCODE" }
    $msi = @(Get-ChildItem -LiteralPath $outputDirectory -Filter '*.msi' -File)
    if ($msi.Count -ne 1) { throw "Expected exactly one MSI output; found $($msi.Count)." }
    Get-ChildItem -LiteralPath $outputDirectory -Filter '*.wixpdb' -File | Remove-Item -Force
    Write-Host "Sentinel Client installer built successfully: $($msi[0].FullName)"
}
finally {
    if (Test-Path -LiteralPath $intermediate) {
        Remove-Item -LiteralPath $intermediate -Recurse -Force
    }
}
