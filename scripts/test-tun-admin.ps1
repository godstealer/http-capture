$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot
& (Join-Path $repoRoot 'target/debug/tun-smoke.exe') *> (Join-Path $repoRoot '.local/tun-smoke-admin.log')
exit $LASTEXITCODE
