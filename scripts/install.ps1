# SaintHarness Windows x64 installer (PowerShell 5.1+ / pwsh 7+)
# Usage:
#   irm https://github.com/Locotine/SaintHarness/releases/latest/download/install.ps1 | iex
#   .\install.ps1 [-Version v0.1.0] [-Repo Locotine/SaintHarness] [-Uninstall]

[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Repo = "Locotine/SaintHarness",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "SaintHarness\bin"),
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

if ($Uninstall) {
    $targetExe = Join-Path $InstallDir "saint-harness.exe"
    if (Test-Path $targetExe) {
        Remove-Item -Force $targetExe
    }
    Write-Host "==> Removed saint-harness.exe. Session data in $env:LOCALAPPDATA\saint-harness is preserved."
    exit 0
}

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
    throw "SaintHarness Windows releases currently target x64 (x86_64-pc-windows-msvc)."
}

if ($Version -eq "latest") {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "SaintHarness-Installer" }
    $Tag = $release.tag_name
} else {
    $Tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
}
$CleanVersion = $Tag.TrimStart("v")
$Asset = "saint-harness_${CleanVersion}_windows-x86_64.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Tag"

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("saint-harness-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    $SumsPath = Join-Path $TmpDir "SHA256SUMS.txt"
    $ZipPath = Join-Path $TmpDir $Asset

    Write-Host "==> Downloading checksums for SaintHarness $Tag..."
    Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS.txt" -OutFile $SumsPath -UseBasicParsing

    Write-Host "==> Downloading $Asset..."
    Invoke-WebRequest -Uri "$BaseUrl/$Asset" -OutFile $ZipPath -UseBasicParsing

    $ExpectedLine = Get-Content $SumsPath | Where-Object { $_ -match "\s+$([regex]::Escape($Asset))$" } | Select-Object -First 1
    if (-not $ExpectedLine) {
        throw "No SHA-256 entry found for $Asset in SHA256SUMS.txt"
    }
    $ExpectedHash = ($ExpectedLine -split "\s+")[0].ToLowerInvariant()
    $ActualHash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "SHA-256 mismatch for $Asset (expected $ExpectedHash, got $ActualHash)"
    }

    $ExtractDir = Join-Path $TmpDir "extracted"
    Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir -Force
    $BinSrc = Get-ChildItem -Path $ExtractDir -Filter "saint-harness.exe" -Recurse | Select-Object -First 1
    if (-not $BinSrc) {
        throw "Archive did not contain saint-harness.exe"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $DestExe = Join-Path $InstallDir "saint-harness.exe"
    Copy-Item -Path $BinSrc.FullName -Destination $DestExe -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $Entries = if ($UserPath) { $UserPath -split ";" | Where-Object { $_ -ne "" } } else { @() }
    if ($Entries -notcontains $InstallDir) {
        $NewPath = ($Entries + $InstallDir) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        $env:Path = "$InstallDir;$env:Path"
        Write-Host "==> Added $InstallDir to your user PATH."
    }

    $InstalledVer = & $DestExe --version
    Write-Host "==> Installed $InstalledVer to $DestExe"
}
finally {
    if (Test-Path $TmpDir) {
        Remove-Item -Recurse -Force $TmpDir
    }
}
