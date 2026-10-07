param([string]$Go = 'go')
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$bundledGo = Join-Path $repoRoot '.local/go-tun/go/bin/go.exe'
if ($Go -eq 'go' -and (Test-Path -LiteralPath $bundledGo)) { $Go = $bundledGo }
$env:GOMODCACHE = Join-Path $repoRoot '.local/go-mod'
$env:GOCACHE = Join-Path $repoRoot '.local/go-build'
$env:GOPROXY = 'https://goproxy.cn,direct'
$outputDirectory = Join-Path $repoRoot '.local/httpcloak'
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
Push-Location (Join-Path $repoRoot 'helpers/httpcloak')
try {
 & $Go build -mod=readonly -trimpath -o (Join-Path $outputDirectory 'http-capture-httpcloak.exe') .
 if ($LASTEXITCODE -ne 0) { throw 'httpcloak helper build failed' }
 $caps = & (Join-Path $outputDirectory 'http-capture-httpcloak.exe') --capabilities
 if ($LASTEXITCODE -ne 0) { throw 'httpcloak capability discovery failed' }
 [IO.File]::WriteAllText((Join-Path $outputDirectory 'http-capture-httpcloak.json'), ($caps -join "`n"), [Text.UTF8Encoding]::new($false))
 $caps
} finally { Pop-Location }
