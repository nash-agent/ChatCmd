[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [string]$ExtensionPath,

    [string]$OutputRoot,

    [string]$PackageName = "ChatCMD-MultiPC-Custom"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

if (-not $ExtensionPath) { $ExtensionPath = Join-Path $repoRoot "chatgpt-extension" }
if (-not $OutputRoot) { $OutputRoot = Join-Path $repoRoot "release-custom" }

$BinaryPath = (Resolve-Path -LiteralPath $BinaryPath).Path
$ExtensionPath = (Resolve-Path -LiteralPath $ExtensionPath).Path
$templateRoot = Join-Path $repoRoot "packaging\custom-windows"

if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) { throw "Binary not found: $BinaryPath" }
if (-not (Test-Path -LiteralPath $ExtensionPath -PathType Container)) { throw "Extension directory not found: $ExtensionPath" }
if (-not (Test-Path -LiteralPath $templateRoot -PathType Container)) { throw "Installer template directory not found: $templateRoot" }

New-Item -ItemType Directory -Force -Path $OutputRoot | Out-Null
$packageDir = Join-Path $OutputRoot $PackageName
$zipPath = "$packageDir.zip"

if (Test-Path -LiteralPath $packageDir) { Remove-Item -LiteralPath $packageDir -Recurse -Force }
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }

New-Item -ItemType Directory -Force -Path $packageDir | Out-Null
$payloadDir = Join-Path $packageDir "payload"
New-Item -ItemType Directory -Force -Path $payloadDir | Out-Null

foreach ($name in @("INSTALL.cmd", "install.ps1", "uninstall.ps1", "README_FIRST.txt")) {
    Copy-Item -LiteralPath (Join-Path $templateRoot $name) -Destination (Join-Path $packageDir $name) -Force
}

Copy-Item -LiteralPath $BinaryPath -Destination (Join-Path $payloadDir "ChatCMD.exe") -Force
Copy-Item -LiteralPath $ExtensionPath -Destination (Join-Path $payloadDir "chatgpt-extension") -Recurse -Force

$packageDir = (Resolve-Path -LiteralPath $packageDir).Path
$lines = Get-ChildItem -LiteralPath $packageDir -File -Recurse |
    Where-Object { $_.Name -ne "SHA256SUMS.txt" } |
    Sort-Object FullName |
    ForEach-Object {
        $relative = $_.FullName.Substring($packageDir.Length).TrimStart('\', '/').Replace('\', '/')
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
        "$hash  $relative"
    }

$sumsPath = Join-Path $packageDir "SHA256SUMS.txt"
Set-Content -LiteralPath $sumsPath -Value $lines -Encoding ASCII

Compress-Archive -Path $packageDir -DestinationPath $zipPath -CompressionLevel Optimal -Force

$bad = @()
foreach ($line in Get-Content -LiteralPath $sumsPath) {
    if ($line -notmatch "^([0-9a-f]{64})  (.+)$") { continue }
    $expected = $Matches[1]
    $relative = $Matches[2].Replace("/", "\")
    $file = Join-Path $packageDir $relative
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { $bad += "$relative expected=$expected actual=$actual" }
}
if ($bad.Count) { throw "Package manifest verification failed:`n$($bad -join "`n")" }

$zipHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zipPath).Hash.ToLowerInvariant()
Write-Host "Package: $packageDir"
Write-Host "ZIP:     $zipPath"
Write-Host "SHA256:  $zipHash"
