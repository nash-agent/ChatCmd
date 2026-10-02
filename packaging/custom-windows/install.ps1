[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\ChatCMD-Custom"),
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"
$packageRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$payloadRoot = Join-Path $packageRoot "payload"
$sourceExe = Join-Path $payloadRoot "ChatCMD.exe"
$sourceExtension = Join-Path $payloadRoot "chatgpt-extension"

if (-not (Test-Path -LiteralPath $sourceExe)) { throw "Package is incomplete: payload\ChatCMD.exe is missing." }
if (-not (Test-Path -LiteralPath $sourceExtension)) { throw "Package is incomplete: payload\chatgpt-extension is missing." }

Write-Host ""
Write-Host "ChatCMD custom multi-PC installer"
Write-Host "Install directory: $InstallDir"
Write-Host ""

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$targetExe = Join-Path $InstallDir "ChatCMD.exe"
$targetExtension = Join-Path $InstallDir "chatgpt-extension"

# Preserve %LOCALAPPDATA%\ChatCmdClient; replace only app/bridge files.
Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force
if (Test-Path -LiteralPath $targetExtension) { Remove-Item -LiteralPath $targetExtension -Recurse -Force }
Copy-Item -LiteralPath $sourceExtension -Destination $targetExtension -Recurse -Force

$expected = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourceExe).Hash
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $targetExe).Hash
if ($expected -ne $actual) { throw "ChatCMD.exe hash verification failed." }

$startupDir = [Environment]::GetFolderPath("Startup")
$startupShortcut = Join-Path $startupDir "ChatCMD Custom.lnk"
$wsh = New-Object -ComObject WScript.Shell
$shortcut = $wsh.CreateShortcut($startupShortcut)
$shortcut.TargetPath = $targetExe
$shortcut.WorkingDirectory = $InstallDir
$shortcut.Description = "Start custom ChatCMD at sign-in"
$shortcut.Save()

$startMenuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "ChatCMD Custom"
New-Item -ItemType Directory -Force -Path $startMenuDir | Out-Null
$startMenuShortcut = Join-Path $startMenuDir "ChatCMD Custom.lnk"
$menuLink = $wsh.CreateShortcut($startMenuShortcut)
$menuLink.TargetPath = $targetExe
$menuLink.WorkingDirectory = $InstallDir
$menuLink.Description = "Custom ChatCMD"
$menuLink.Save()

$nextSteps = @"
ChatCMD custom installation completed.

Installed:
  $targetExe

Browser extension:
  $targetExtension

Automatic startup:
  $startupShortcut

Manual steps:
  1. Open ChatCMD and register work folders with:
     Projects -> + -> Project folder -> Choose folder
  2. In Chrome/Edge/Brave extensions, enable Developer mode.
  3. Choose "Load unpacked" and select:
     $targetExtension
  4. Confirm the extension is "ChatCMD ChatGPT Bridge".
     It is the only Chromium extension required by this package.
  5. Sign in to chatgpt.com in the same browser profile and reload the ChatGPT tab.
  6. Create a NEW MCP access profile / tunnel / plugin connection for this PC.
     The Astra Workspace / ChatCMD plugin is an MCP connection, not a second browser extension.
     Tokens and credentials are intentionally not bundled.

Local ChatCMD address:
  http://127.0.0.1:8080

ChatCMD.exe SHA-256:
  $actual

See CUSTOM_SETUP.md in the source repository for the PC2 acceptance checklist.
"@
$nextStepsPath = Join-Path $InstallDir "NEXT_STEPS.txt"
Set-Content -LiteralPath $nextStepsPath -Value $nextSteps -Encoding UTF8
try { Set-Clipboard -Value $targetExtension } catch {}

if (-not $NoStart) {
    $alreadyRunning = Get-CimInstance Win32_Process -Filter "Name='ChatCMD.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $targetExe }
    if (-not $alreadyRunning) { Start-Process -FilePath $targetExe -WorkingDirectory $InstallDir | Out-Null }

    $healthy = $false
    for ($i = 0; $i -lt 20; $i++) {
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:8080/api/ping" -TimeoutSec 2
            if ($response.StatusCode -eq 200) { $healthy = $true; break }
        } catch {}
        Start-Sleep -Milliseconds 500
    }
    if ($healthy) { Write-Host "ChatCMD local health check: OK" }
    else { Write-Warning "ChatCMD was installed, but http://127.0.0.1:8080/api/ping did not answer yet." }
}

Write-Host ""
Write-Host "Installed successfully."
Write-Host "Extension folder path copied to clipboard:"
Write-Host "  $targetExtension"
Write-Host ""
Write-Host "Browser extension loading and per-PC credentials remain manual."
Write-Host "See: $nextStepsPath"
Write-Host ""
