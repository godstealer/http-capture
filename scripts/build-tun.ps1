param([string]$Go = "go")
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$bundledGo = Join-Path $repoRoot '.local/go-tun/go/bin/go.exe'
if ($Go -eq 'go' -and (Test-Path -LiteralPath $bundledGo)) { $Go = $bundledGo }
$env:GOMODCACHE = Join-Path $repoRoot '.local/go-mod'
$env:GOCACHE = Join-Path $repoRoot '.local/go-build'
$outputDirectory = Join-Path $repoRoot '.local/tun'
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
Push-Location (Join-Path $repoRoot 'helpers/tun')
try {
    & $Go build -mod=readonly -trimpath -o (Join-Path $outputDirectory 'http-capture-tun.exe') .
    if ($LASTEXITCODE -ne 0) { throw 'TUN helper build failed. Go 1.25.5+ is required.' }
    & (Join-Path $outputDirectory 'http-capture-tun.exe') --version
} finally { Pop-Location }
