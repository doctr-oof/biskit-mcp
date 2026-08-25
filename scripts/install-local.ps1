<#
.SYNOPSIS
    Builds Biskit MCP from the working tree and replaces the local install.

.DESCRIPTION
    Runs a release build and copies the resulting binary over the one the published
    installer writes, so the machine's `biskit-mcp` command reflects local changes.
    Nothing is downloaded and the user PATH is left alone.

.PARAMETER InstallDir
    Directory holding the installed binary. Defaults to BISKIT_INSTALL_DIR, and then
    to %LOCALAPPDATA%\biskit\bin, which is where install.ps1 puts it.

.NOTES
    Windows refuses to overwrite an executable that is running, which is the normal
    case when an agent still has the server open. The old binary is moved aside
    instead, and leftovers are cleared on the next run.
#>
[CmdletBinding()]
param(
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $InstallDir) {
    if ($env:BISKIT_INSTALL_DIR) {
        $InstallDir = $env:BISKIT_INSTALL_DIR
    }
    else {
        $InstallDir = Join-Path $env:LOCALAPPDATA "biskit\bin"
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is required but was not found on PATH."
}

$root = Split-Path -Parent $PSScriptRoot

Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}

$source = Join-Path $root "target\release\biskit-mcp.exe"
if (-not (Test-Path -LiteralPath $source)) {
    throw "The build finished but $source is missing."
}

if (-not (Test-Path -LiteralPath $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

$destination = Join-Path $InstallDir "biskit-mcp.exe"

Get-ChildItem -LiteralPath $InstallDir -Filter "biskit-mcp.exe.old-*" -ErrorAction SilentlyContinue |
ForEach-Object {
    try {
        Remove-Item -LiteralPath $_.FullName -Force -ErrorAction Stop
    }
    catch {
        # Still held by a process that has not exited yet. It gets cleared next time.
    }
}

if (Test-Path -LiteralPath $destination) {
    try {
        Remove-Item -LiteralPath $destination -Force -ErrorAction Stop
    }
    catch {
        $suffix = [System.Guid]::NewGuid().ToString("N").Substring(0, 8)
        $parked = Join-Path $InstallDir "biskit-mcp.exe.old-$suffix"
        Move-Item -LiteralPath $destination -Destination $parked -Force
        Write-Host "The installed binary is still running, so it was moved aside as biskit-mcp.exe.old-$suffix."
        Write-Host "Restart any agent holding Biskit open to pick up the new build."
    }
}

Copy-Item -LiteralPath $source -Destination $destination -Force

$reported = (& $destination --version) -join " "

Write-Host ""
Write-Host "Installed $reported to $destination"
