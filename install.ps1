<#
.SYNOPSIS
    Installs Biskit MCP.

.DESCRIPTION
    Downloads the latest (or a specified) Biskit release, verifies its SHA-256 digest
    against the published SHA256SUMS file, and installs it to the user's local bin
    directory.

.PARAMETER Version
    Release tag to install, for example "v0.1.0". Defaults to the latest release.

.PARAMETER InstallDir
    Directory to install into. Defaults to %LOCALAPPDATA%\biskit\bin.

.PARAMETER SetupProject
    Project to register Biskit in without prompting. Use with -SetupClients or -SetupHooks.

.PARAMETER SetupClients
    Agents to configure without prompting: claude, cursor, vscode.

.PARAMETER SetupHooks
    Add the Claude Code SessionStart hook without prompting.

.PARAMETER SetupHooksTarget
    Which Claude Code settings file the hook goes in: local (default) or shared.

.PARAMETER NoProjectFromCwd
    Omit --project-from-cwd from the generated registration.

.PARAMETER NoSetup
    Install only. Never offer to register Biskit in a project.

.NOTES
    When run through `irm | iex` no parameters can be passed, so the same options are
    readable from the environment: BISKIT_SETUP_PROJECT, BISKIT_SETUP_CLIENTS,
    BISKIT_SETUP_HOOKS, BISKIT_SETUP_HOOKS_TARGET, BISKIT_SETUP_FROM_CWD, BISKIT_NO_SETUP.

    `irm | iex` runs this file in the caller's scope, which has two consequences. Every
    parameter carrying a validation attribute needs a default that satisfies it, because
    the attribute is applied to the caller's variable rather than to a bound argument.
    And the body runs inside `& { ... }` so that its preferences, strict mode, and helper
    functions do not outlive the install.
#>
[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "biskit\bin"),
    [string]$SetupProject,
    [ValidateSet("claude", "cursor", "vscode")]
    [string[]]$SetupClients = @(),
    [switch]$SetupHooks,
    [ValidateSet("local", "shared")]
    [string]$SetupHooksTarget = "local",
    [switch]$NoProjectFromCwd,
    [switch]$NoSetup
)

& {
    $ErrorActionPreference = "Stop"
    Set-StrictMode -Version Latest

    $Repository = "doctr-oof/biskit-mcp"

    function Get-TargetAsset {
        $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
        switch ($architecture) {
            "X64" { return "biskit-mcp-windows-x86_64.zip" }
            default { throw "Biskit does not publish a Windows build for $architecture." }
        }
    }

    function Resolve-ReleaseTag {
        param([string]$Requested)
        if ($Requested -ne "latest") { return $Requested }

        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases/latest" `
            -Headers @{ "User-Agent" = "biskit-installer" }
        return $release.tag_name
    }

    $asset = Get-TargetAsset
    $tag = Resolve-ReleaseTag -Requested $Version
    $baseUrl = "https://github.com/$Repository/releases/download/$tag"

    $workspace = Join-Path ([System.IO.Path]::GetTempPath()) ("biskit-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $workspace | Out-Null

    try {
        $archivePath = Join-Path $workspace $asset
        $sumsPath = Join-Path $workspace "SHA256SUMS"

        Write-Host "Downloading $asset ($tag)..."
        Invoke-WebRequest -Uri "$baseUrl/$asset" -OutFile $archivePath -UseBasicParsing
        Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $sumsPath -UseBasicParsing

        $expectedLine = Select-String -Path $sumsPath -Pattern ([regex]::Escape($asset)) | Select-Object -First 1
        if ($null -eq $expectedLine) {
            throw "SHA256SUMS does not list $asset. Refusing to install."
        }
        $expected = ($expectedLine.Line -split '\s+')[0].ToLower()
        $actual = (Get-FileHash $archivePath -Algorithm SHA256).Hash.ToLower()

        if ($expected -ne $actual) {
            throw "Checksum mismatch for ${asset}: expected $expected, got $actual. The download was discarded."
        }
        Write-Host "Checksum verified."

        if (-not (Test-Path $InstallDir)) {
            New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        }
        Expand-Archive -Path $archivePath -DestinationPath $InstallDir -Force

        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        if ($userPath -notlike "*$InstallDir*") {
            [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
            Write-Host "Added $InstallDir to your user PATH. Restart your terminal to pick it up."
        }

        Write-Host ""
        Write-Host "Biskit installed to $InstallDir"
    }
    finally {
        Remove-Item -Recurse -Force $workspace -ErrorAction SilentlyContinue
    }

    $binary = Join-Path $InstallDir "biskit-mcp.exe"

    function Write-ManualSteps {
        Write-Host ""
        Write-Host "Register it with your agent, for example:"
        Write-Host "  claude mcp add biskit -- `"$binary`" start"
        Write-Host ""
        Write-Host "Or register it inside a project at any time:"
        Write-Host "  cd C:\path\to\project; & `"$binary`" setup --client claude --hooks"
    }

    function Test-CanPrompt {
        if ([Console]::IsInputRedirected) { return $false }
        return [Environment]::UserInteractive
    }

    function Read-YesNo {
        param([string]$Question, [bool]$Default)

        $hint = if ($Default) { "Y/n" } else { "y/N" }
        while ($true) {
            try {
                $reply = Read-Host "$Question [$hint]"
            }
            catch {
                return $Default
            }
            if ([string]::IsNullOrWhiteSpace($reply)) { return $Default }
            switch -Regex ($reply.Trim()) {
                '^(y|yes)$' { return $true }
                '^(n|no)$' { return $false }
                default { Write-Host "Please enter y or n." }
            }
        }
    }

    function Read-ProjectDir {
        param([string]$Default)

        while ($true) {
            try {
                $reply = Read-Host "Project directory [$Default]"
            }
            catch {
                return $Default
            }
            if ([string]::IsNullOrWhiteSpace($reply)) { $reply = $Default }
            $reply = $reply.Trim('"').Trim()
            if (Test-Path -LiteralPath $reply -PathType Container) { return $reply }
            Write-Host "Not a directory: $reply"
        }
    }

    if ($NoSetup -or ($env:BISKIT_NO_SETUP -eq "1")) {
        Write-ManualSteps
        return
    }

    $clients = @(if ($SetupClients) { $SetupClients } elseif ($env:BISKIT_SETUP_CLIENTS) { $env:BISKIT_SETUP_CLIENTS -split '[,\s]+' | Where-Object { $_ } })
    $wantHooks = $SetupHooks -or ($env:BISKIT_SETUP_HOOKS -eq "1")
    $hooksTarget = if ($env:BISKIT_SETUP_HOOKS_TARGET) { $env:BISKIT_SETUP_HOOKS_TARGET } else { $SetupHooksTarget }
    $fromCwd = -not ($NoProjectFromCwd -or ($env:BISKIT_SETUP_FROM_CWD -eq "0"))

    if ($clients.Count -gt 0 -or $wantHooks) {
        $project = if ($SetupProject) { $SetupProject } elseif ($env:BISKIT_SETUP_PROJECT) { $env:BISKIT_SETUP_PROJECT } else { (Get-Location).Path }
        $setupArgs = @("setup", "--project", $project)
        foreach ($client in $clients) { $setupArgs += @("--client", $client) }
        if ($wantHooks) { $setupArgs += @("--hooks", "--hooks-target", $hooksTarget) }
        if ($fromCwd) { $setupArgs += "--project-from-cwd" }

        Write-Host ""
        & $binary @setupArgs
        return
    }

    if (-not (Test-CanPrompt)) {
        Write-ManualSteps
        return
    }

    Write-Host ""
    if (-not (Read-YesNo "Register Biskit in a project now?" $false)) {
        Write-ManualSteps
        return
    }

    $project = Read-ProjectDir (Get-Location).Path
    $setupArgs = @("setup", "--project", $project)
    $selected = $false

    if (Read-YesNo "  Write .mcp.json (Claude Code)?" $true) {
        $setupArgs += @("--client", "claude")
        $selected = $true
    }
    if (Read-YesNo "  Write .cursor\mcp.json (Cursor)?" $false) {
        $setupArgs += @("--client", "cursor")
        $selected = $true
    }
    if (Read-YesNo "  Write .vscode\mcp.json (VS Code)?" $false) {
        $setupArgs += @("--client", "vscode")
        $selected = $true
    }
    if (Read-YesNo "  Add the Claude Code SessionStart hook to .claude\settings.local.json?" $false) {
        $setupArgs += "--hooks"
        $selected = $true
    }

    if (-not $selected) {
        Write-Host ""
        Write-Host "Nothing selected, so nothing was written."
        Write-ManualSteps
        return
    }

    if (Read-YesNo "  Pin the registration to this project (--project-from-cwd)?" $true) {
        $setupArgs += "--project-from-cwd"
    }

    Write-Host ""
    & $binary @setupArgs
}
