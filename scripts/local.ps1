#Requires -Version 5.1
<#
.SYNOPSIS
Rebuild a personal AgentLens index and serve usage on IPv4 loopback.
.DESCRIPTION
Run from any directory. See docs/personal-launcher.md for environment overrides.
.PARAMETER Port
Loopback port for the usage web server. Defaults to 3001.
.PARAMETER IndexOnly
Rebuild the index without opening a listening port.
#>
[CmdletBinding()]
param(
    [ValidateRange(1, 65535)]
    [int] $Port = 3001,
    [switch] $IndexOnly
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

function Get-FullLocalPath([string] $Path, [string] $Name) {
    if ($Path -notmatch '^[A-Za-z]:[\\/]' -or $Path.Substring(2).Contains(':') -or
        $Path -match '(?:[. ]|~[0-9]+)(?:[\\/]|$)') {
        throw "$Name must be an absolute drive path without dot components, trailing dots or spaces, or short-name aliases."
    }
    $fullPath = [IO.Path]::GetFullPath($Path)
    if ($fullPath.Length -gt 3) { $fullPath = $fullPath.TrimEnd('\') }
    return $fullPath
}

function Get-PathAttributes([string] $Path) {
    try {
        return [IO.File]::GetAttributes($Path)
    } catch {
        if ($_.Exception.InnerException -is [IO.FileNotFoundException] -or
            $_.Exception.InnerException -is [IO.DirectoryNotFoundException]) {
            return $null
        }
        throw
    }
}

function Assert-PlainDirectoryPath([string] $Path) {
    for ($part = $Path; $part; $part = [IO.Path]::GetDirectoryName($part)) {
        $attributes = Get-PathAttributes $part
        if ($null -eq $attributes) { continue }
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -or
            -not ($attributes -band [IO.FileAttributes]::Directory)) {
            throw "Expected an ordinary directory at '$part'. Choose a path without junctions or symbolic links."
        }
    }
}

function Test-Inside([string] $Path, [string] $Directory) {
    return ($Path.TrimEnd('\') + '\').StartsWith(
        $Directory.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
}

function Get-AgentHome([string] $Name, [string] $Override, [string] $Default) {
    $candidate = $Default
    if ($Override) { $candidate = $Override }
    $candidate = Get-FullLocalPath $candidate "$Name home"
    Assert-PlainDirectoryPath $candidate
    if ($null -ne (Get-PathAttributes $candidate)) {
        return $candidate
    }
    if ($Override) { throw "$Name home does not exist at '$candidate'." }
}

function Assert-FreePort([int] $Number) {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Number)
    $listener.ExclusiveAddressUse = $true
    try {
        $listener.Start()
    } catch {
        throw "Cannot listen on 127.0.0.1:${Number}. The port may be occupied or reserved. Use -Port with another number."
    } finally {
        $listener.Stop()
    }
}

$lock = $null
try {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        throw 'Use scripts/local.sh on Linux or macOS.'
    }
    $profilePath = Get-FullLocalPath $env:USERPROFILE 'USERPROFILE'
    $repo = Get-FullLocalPath (Split-Path -Parent $PSScriptRoot) 'repository'
    $codexOverride = $env:CODEX_HOME
    if ($env:AGENT_LENS_CODEX_HOME) { $codexOverride = $env:AGENT_LENS_CODEX_HOME }
    $hermesOverride = $env:HERMES_HOME
    if ($env:AGENT_LENS_HERMES_HOME) { $hermesOverride = $env:AGENT_LENS_HERMES_HOME }
    $homes = [ordered] @{
        codex = Get-AgentHome 'Codex' $codexOverride (Join-Path $profilePath '.codex')
        gemini = Get-AgentHome 'Gemini CLI' $env:AGENT_LENS_GEMINI_HOME (Join-Path $profilePath '.gemini')
        hermes = Get-AgentHome 'Hermes' $hermesOverride (Join-Path $profilePath '.hermes')
    }
    $sourceArguments = @()
    foreach ($agent in $homes.Keys) {
        if ($homes[$agent]) { $sourceArguments += @("--$agent-home", $homes[$agent]) }
    }
    if ($sourceArguments.Count -eq 0) {
        throw 'No supported agent homes found. See docs/personal-launcher.md for custom home settings.'
    }

    $storage = $env:AGENT_LENS_LOCAL_DIR
    if (-not $storage) {
        $localAppData = Get-FullLocalPath $env:LOCALAPPDATA 'LOCALAPPDATA'
        $storage = Join-Path $localAppData 'AgentLens'
    }
    $storage = Get-FullLocalPath $storage 'personal storage'
    Assert-PlainDirectoryPath $storage
    foreach ($agent in $homes.Keys) {
        $agentHome = $homes[$agent]
        if (-not $agentHome) { continue }
        if ((Test-Inside $storage $agentHome) -or (Test-Inside $agentHome $storage)) {
            throw 'Personal storage and agent homes must be separate directories.'
        }
        if (-not $env:AGENT_LENS_BIN -and (Test-Inside $repo $agentHome)) {
            throw 'Move the repository outside the agent homes before building.'
        }
        foreach ($otherAgent in $homes.Keys) {
            if ($agent -ne $otherAgent -and $homes[$otherAgent] -and
                (Test-Inside $agentHome $homes[$otherAgent])) {
                throw 'Configure separate, non-overlapping directories for each agent.'
            }
        }
    }

    $target = Join-Path $repo 'target'
    $cargo = $null
    if ($env:AGENT_LENS_BIN) {
        $binary = Get-FullLocalPath $env:AGENT_LENS_BIN 'AGENT_LENS_BIN'
        if ([IO.Path]::GetExtension($binary) -ine '.exe' -or -not [IO.File]::Exists($binary)) {
            throw "AGENT_LENS_BIN must point to an existing compatible Windows .exe: '$binary'."
        }
    } else {
        $cargoCommand = Get-Command cargo.exe -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($cargoCommand) { $cargo = $cargoCommand.Source }
        if (-not $cargo) {
            $cargo = Join-Path $profilePath '.cargo\bin\cargo.exe'
            if (-not [IO.File]::Exists($cargo)) {
                throw 'Cargo was not found. Install Rust/Cargo, MSVC with a Windows SDK, and OpenSSL, or set AGENT_LENS_BIN. See docs/personal-launcher.md.'
            }
        }
        Assert-PlainDirectoryPath $target
        foreach ($agentHome in $homes.Values) {
            if ($agentHome -and (Test-Inside $agentHome $target)) {
                throw 'An agent home is inside the Cargo target directory. Move it or use AGENT_LENS_BIN.'
            }
        }
        $binary = Join-Path $target 'debug\agent-lens.exe'
    }

    if (-not $IndexOnly) { Assert-FreePort $Port }
    [void] [IO.Directory]::CreateDirectory($storage)
    foreach ($entry in Get-ChildItem -LiteralPath $storage -Force) {
        if ($entry.Name -match '^(launcher\.lock$|index\.sqlite($|-)|state\.sqlite($|-)|mfa-keys\.json($|\.))' -and
            ($entry.PSIsContainer -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
                $entry.LinkType -eq 'HardLink')) {
            throw "Expected a regular launcher file at '$($entry.FullName)'. Choose a separate personal storage directory."
        }
    }
    try {
        $lock = [IO.File]::Open((Join-Path $storage 'launcher.lock'), [IO.FileMode]::OpenOrCreate,
            [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    } catch {
        throw "Cannot lock personal storage at '$storage'. Stop any other launcher using it and confirm you can write there. $($_.Exception.Message)"
    }

    if ($cargo) {
        Write-Host 'Checking the AgentLens build with Cargo...'
        & $cargo build --locked --manifest-path (Join-Path $repo 'Cargo.toml') --target-dir $target
        if ($LASTEXITCODE -ne 0 -or -not [IO.File]::Exists($binary)) {
            throw "Cargo build failed or did not produce '$binary'. Check the errors above and the Windows prerequisites in docs/personal-launcher.md."
        }
    }

    foreach ($agent in $homes.Keys) {
        if ($homes[$agent]) { Write-Host "Found $agent" }
    }
    $databaseArguments = @(
        '--database', (Join-Path $storage 'index.sqlite'),
        '--state-database', (Join-Path $storage 'state.sqlite'),
        '--mfa-key-file', (Join-Path $storage 'mfa-keys.json')
    )
    Write-Host 'Rebuilding the personal usage index...'
    & $binary @databaseArguments rebuild @sourceArguments
    if ($LASTEXITCODE -ne 0) {
        throw "AgentLens rebuild failed (exit $LASTEXITCODE). If supplying an executable, use a build from this checkout or a compatible newer version."
    }
    if ($IndexOnly) {
        Write-Host 'Personal index rebuilt. Run again without -IndexOnly to view usage.'
        exit 0
    }

    Assert-FreePort $Port
    Write-Host "Open http://127.0.0.1:${Port}/usage (Ctrl-C to stop; rerun to update usage)."
    # Run in the foreground so the console delivers Ctrl-C to AgentLens as well.
    & $binary @databaseArguments serve --local --bind "127.0.0.1:${Port}"
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1073741510) {
        throw "AgentLens server exited with code $LASTEXITCODE. Check the errors above or try another -Port."
    }
} catch {
    [Console]::Error.WriteLine("AgentLens: $($_.Exception.Message)")
    exit 1
} finally {
    if ($null -ne $lock) { $lock.Dispose() }
}
