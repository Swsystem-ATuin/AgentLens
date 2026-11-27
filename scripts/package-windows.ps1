#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Binary,
    [Parameter(Mandatory)] [string] $Output
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$binaryPath = (Get-Item -LiteralPath $Binary).FullName
$outputPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output)
[void] [IO.Directory]::CreateDirectory($outputPath)
$staging = Join-Path ([IO.Path]::GetTempPath()) ("agentlens-package-" + [Guid]::NewGuid())
$content = Join-Path $staging 'AgentLens'
[void] [IO.Directory]::CreateDirectory($content)
try {
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $content 'agent-lens.exe')
    Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $content
    Copy-Item -LiteralPath (Join-Path $repo 'packaging\windows\Start AgentLens.cmd') -Destination $content
    Copy-Item -LiteralPath (Join-Path $repo 'packaging\windows\Quick start.txt') -Destination $content

    $zip = Join-Path $outputPath 'AgentLens-windows-x64.zip'
    Compress-Archive -LiteralPath $content -DestinationPath $zip
    $sha = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$zip.sha256", "$sha  AgentLens-windows-x64.zip`n", [Text.Encoding]::ASCII)
} finally {
    # These are the only paths created under the fresh staging directory.
    foreach ($name in 'agent-lens.exe', 'LICENSE', 'Start AgentLens.cmd', 'Quick start.txt') {
        [IO.File]::Delete((Join-Path $content $name))
    }
    [IO.Directory]::Delete($content)
    [IO.Directory]::Delete($staging)
}
