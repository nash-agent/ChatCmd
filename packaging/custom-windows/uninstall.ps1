[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\ChatCMD-Custom"),
    [switch]$RemoveData
)

$ErrorActionPreference = "Stop"
$targetExe = Join-Path $InstallDir "ChatCMD.exe"
Get-CimInstance Win32_Process -Filter "Name='ChatCMD.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -eq $targetExe } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

$startupShortcut = Join-Path ([Environment]::GetFolderPath("Startup")) "ChatCMD Custom.lnk"
$startMenuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "ChatCMD Custom"
Remove-Item -LiteralPath $startupShortcut -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $startMenuDir -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue

if ($RemoveData) {
    $dataDir = Join-Path $env:LOCALAPPDATA "ChatCmdClient"
    Remove-Item -LiteralPath $dataDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Application and local ChatCMD data removed."
} else {
    Write-Host "Application removed. ChatCMD database/settings were preserved."
    Write-Host "Use -RemoveData only if you also want to delete %LOCALAPPDATA%\ChatCmdClient."
}
