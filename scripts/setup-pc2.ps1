[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\ChatCMD-Custom"),
    [switch]$SkipPrerequisiteInstall,
    [switch]$NoStart
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$RepoRoot = Split-Path -Parent $PSScriptRoot

function Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Refresh-Path {
    $parts = @(
        [Environment]::GetEnvironmentVariable("Path", "Machine"),
        [Environment]::GetEnvironmentVariable("Path", "User"),
        (Join-Path $env:USERPROFILE ".cargo\bin")
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    $env:Path = $parts -join ";"
}

function Command-Path([string]$Name) {
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Winget-Install([string]$Id, [string[]]$Extra = @()) {
    $winget = Command-Path "winget"
    if (-not $winget) {
        throw "winget is unavailable. Install Microsoft App Installer, then run SETUP_PC2.cmd again."
    }
    $args = @("install","--id",$Id,"-e","--source","winget","--accept-source-agreements","--accept-package-agreements","--disable-interactivity")
    if ($Extra.Count -gt 0) { $args += $Extra }
    & $winget @args
    if ($LASTEXITCODE -ne 0) { throw "winget install failed for $Id (exit $LASTEXITCODE)." }
    Refresh-Path
}

function Node-OK {
    $node = Command-Path "node"
    if (-not $node) { return $false }
    $v = & $node --version 2>$null
    if ($v -notmatch '^v(\d+)\.(\d+)\.(\d+)') { return $false }
    $major = [int]$Matches[1]
    $minor = [int]$Matches[2]
    if ($major -eq 20) { return $minor -ge 19 }
    if ($major -eq 22) { return $minor -ge 12 }
    return $major -gt 22
}

function Rust-OK {
    $rustc = Command-Path "rustc"
    if (-not $rustc) { return $false }
    $v = & $rustc --version 2>$null
    if ($v -notmatch 'rustc\s+(\d+)\.(\d+)\.(\d+)') { return $false }
    return ([Version]::new([int]$Matches[1],[int]$Matches[2],[int]$Matches[3]) -ge [Version]::new(1,85,0))
}

function VsWhere-Path {
    foreach ($base in @([Environment]::GetFolderPath("ProgramFilesX86"),[Environment]::GetFolderPath("ProgramFiles"))) {
        if ([string]::IsNullOrWhiteSpace($base)) { continue }
        $candidate = Join-Path $base "Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
    return $null
}

function Vs-Install {
    $vswhere = VsWhere-Path
    if (-not $vswhere) { return $null }
    $path = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($path)) { return $null }
    return $path.Trim()
}

function Import-VsEnvironment {
    $install = Vs-Install
    if (-not $install) { throw "Visual Studio C++ Build Tools were not found." }
    $devcmd = Join-Path $install "Common7\Tools\VsDevCmd.bat"
    if (-not (Test-Path -LiteralPath $devcmd -PathType Leaf)) { throw "VsDevCmd.bat was not found." }
    $tmp = Join-Path $env:TEMP ("chatcmd-vs-" + [Guid]::NewGuid().ToString("N") + ".cmd")
    try {
        @("@echo off",('call "{0}" -no_logo -arch=x64 -host_arch=x64 >nul' -f $devcmd),"if errorlevel 1 exit /b %errorlevel%","set") | Set-Content -LiteralPath $tmp -Encoding ASCII
        $lines = & $env:ComSpec /d /c $tmp
        if ($LASTEXITCODE -ne 0) { throw "VsDevCmd failed." }
        foreach ($line in $lines) {
            $i = $line.IndexOf("=")
            if ($i -le 0) { continue }
            [Environment]::SetEnvironmentVariable($line.Substring(0,$i),$line.Substring($i+1),"Process")
        }
    } finally {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
}

function Ensure-Prerequisites {
    Refresh-Path

    if (-not (Node-OK)) {
        if ($SkipPrerequisiteInstall) { throw "Node.js 20.19+ or 22.12+ is required." }
        Step "Installing Node.js LTS"
        Winget-Install "OpenJS.NodeJS.LTS"
    }
    if (-not (Node-OK)) { throw "Compatible Node.js is still unavailable. Reopen Terminal and run SETUP_PC2.cmd again." }

    if (-not (Command-Path "rustup")) {
        if ($SkipPrerequisiteInstall) { throw "Rustup is required." }
        Step "Installing Rustup"
        Winget-Install "Rustlang.Rustup"
    }

    Refresh-Path
    $rustup = Command-Path "rustup"
    if (-not $rustup) { throw "rustup is still unavailable. Reopen Terminal and run SETUP_PC2.cmd again." }

    if (-not (Rust-OK)) {
        Step "Installing/updating Rust stable"
        & $rustup toolchain install stable --profile minimal
        if ($LASTEXITCODE -ne 0) { throw "rustup toolchain install failed." }
        & $rustup default stable
        if ($LASTEXITCODE -ne 0) { throw "rustup default stable failed." }
        Refresh-Path
    }
    if (-not (Rust-OK)) { throw "Rust 1.85+ is required." }

    if (-not (Vs-Install)) {
        if ($SkipPrerequisiteInstall) { throw "Visual Studio C++ Build Tools are required." }
        Step "Installing Visual Studio 2022 C++ Build Tools"
        Winget-Install "Microsoft.VisualStudio.2022.BuildTools" @("--override","--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended")
    }
    if (-not (Vs-Install)) { throw "C++ Build Tools could not be verified. Reboot if Windows requested it, then run SETUP_PC2.cmd again." }

    Write-Host "Node: $(& (Command-Path 'node') --version)"
    Write-Host "Rust: $(& (Command-Path 'rustc') --version)"
    Write-Host "MSVC: $(Vs-Install)"
}

function Build-ChatCMD {
    Import-VsEnvironment
    Refresh-Path

    $npm = Command-Path "npm.cmd"
    if (-not $npm) { $npm = Command-Path "npm" }
    $cargo = Command-Path "cargo"
    $rustup = Command-Path "rustup"
    if (-not $npm -or -not $cargo -or -not $rustup) { throw "Build tools are missing from PATH." }

    Step "Installing web dependencies"
    Push-Location (Join-Path $RepoRoot "web")
    try {
        & $npm ci --prefer-offline --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw "npm ci failed." }
        Step "Building ChatCMD web UI"
        & $npm run build
        if ($LASTEXITCODE -ne 0) { throw "npm run build failed." }
    } finally {
        Pop-Location
    }

    $target = "x86_64-pc-windows-msvc"
    $targets = @(& $rustup target list --installed)
    if ($target -notin $targets) {
        Step "Installing Rust Windows x64 target"
        & $rustup target add $target
        if ($LASTEXITCODE -ne 0) { throw "rustup target add failed." }
    }

    Step "Building ChatCMD x64 release"
    Push-Location $RepoRoot
    try {
        & $cargo build --release --features embedded-web --target $target
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed." }
    } finally {
        Pop-Location
    }

    $exe = Join-Path $RepoRoot "target\$target\release\chat-cmd-client.exe"
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Build output was not created: $exe" }
    return $exe
}

function Install-ChatCMD([string]$BuiltExe) {
    Step "Installing ChatCMD"

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

    $targetExe = Join-Path $InstallDir "ChatCMD.exe"
    $targetExtension = Join-Path $InstallDir "chatgpt-extension"
    $sourceExtension = Join-Path $RepoRoot "chatgpt-extension"

    Get-CimInstance Win32_Process -Filter "Name='ChatCMD.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $targetExe } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

    Copy-Item -LiteralPath $BuiltExe -Destination $targetExe -Force

    if (Test-Path -LiteralPath $targetExtension) {
        Remove-Item -LiteralPath $targetExtension -Recurse -Force
    }
    Copy-Item -LiteralPath $sourceExtension -Destination $targetExtension -Recurse -Force

    if (-not (Test-Path -LiteralPath (Join-Path $targetExtension "manifest.json") -PathType Leaf)) {
        throw "Extension install is incomplete: manifest.json is missing."
    }

    $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $BuiltExe).Hash
    $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $targetExe).Hash
    if ($sourceHash -ne $installedHash) { throw "Installed ChatCMD.exe failed SHA-256 verification." }

    $wsh = New-Object -ComObject WScript.Shell

    $startupLink = Join-Path ([Environment]::GetFolderPath("Startup")) "ChatCMD Custom.lnk"
    $link = $wsh.CreateShortcut($startupLink)
    $link.TargetPath = $targetExe
    $link.WorkingDirectory = $InstallDir
    $link.Description = "ChatCMD Astra Workspace"
    $link.Save()

    $menuDir = Join-Path ([Environment]::GetFolderPath("Programs")) "ChatCMD Custom"
    New-Item -ItemType Directory -Force -Path $menuDir | Out-Null
    $menu = $wsh.CreateShortcut((Join-Path $menuDir "ChatCMD Custom.lnk"))
    $menu.TargetPath = $targetExe
    $menu.WorkingDirectory = $InstallDir
    $menu.Description = "ChatCMD Astra Workspace"
    $menu.Save()

    $manual = @"
AUTOMATIC SETUP COMPLETE

Installed ChatCMD:
$targetExe

Installed browser bridge:
$targetExtension

ONLY HUMAN STEPS LEFT

1. Chrome / Edge / Brave:
   - Open the browser extension manager.
   - Enable Developer mode.
   - Click "Load unpacked".
   - Select this exact folder:
     $targetExtension
   - Confirm the extension name is "ChatCMD ChatGPT Bridge".

2. Sign in to chatgpt.com in that same browser profile and reload ChatGPT once.

3. In ChatCMD create this PC's MCP access profile/access code.
   Local UI:
   http://127.0.0.1:8080

4. Add the local project folders this PC is allowed to use:
   Projects -> + -> Project folder -> Choose folder

5. Do NOT install or configure Cloudflare just because the MCP URL uses 127.0.0.1.
   Use the same external connection method as the working PC1 setup if one is actually required.

Everything the installer can safely automate is already complete.
"@

    $manualPath = Join-Path $InstallDir "HUMAN_STEPS_ONLY.txt"
    Set-Content -LiteralPath $manualPath -Value $manual -Encoding UTF8

    try { Set-Clipboard -Value $targetExtension } catch {}

    if (-not $NoStart) {
        Start-Process -FilePath $targetExe -WorkingDirectory $InstallDir | Out-Null

        Step "Checking ChatCMD local server"
        $healthy = $false

        for ($i = 0; $i -lt 40; $i++) {
            try {
                $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:8080/api/ping" -TimeoutSec 2
                if ($response.StatusCode -eq 200) {
                    $healthy = $true
                    break
                }
            } catch {}
            Start-Sleep -Milliseconds 500
        }

        if (-not $healthy) {
            throw "ChatCMD installed but http://127.0.0.1:8080/api/ping did not become healthy."
        }

        try { Start-Process explorer.exe -ArgumentList ('"' + $targetExtension + '"') | Out-Null } catch {}
        try { Start-Process "http://127.0.0.1:8080" | Out-Null } catch {}
    }

    Write-Host ""
    Write-Host "SETUP COMPLETE" -ForegroundColor Green
    Write-Host "ChatCMD: $targetExe"
    Write-Host "Extension folder: $targetExtension"
    Write-Host "Human-only instructions: $manualPath"
    Write-Host "The extension folder path was copied to the clipboard."
}

if ($env:OS -ne "Windows_NT") { throw "This setup is for Windows only." }

Step "Checking prerequisites"
Ensure-Prerequisites
$builtExe = Build-ChatCMD
Install-ChatCMD $builtExe
