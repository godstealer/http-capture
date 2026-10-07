param([string]$Client = '')
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot
if (Get-Process -Name http-capture-tun -ErrorAction SilentlyContinue) { throw 'Another TUN helper is running; skip test.' }
if (!(Test-Path -LiteralPath (Join-Path $repoRoot '.local/tun/http-capture-tun.exe'))) { throw 'Missing TUN helper: run scripts/build-tun.ps1 first.' }
if (!(Test-Path -LiteralPath (Join-Path $repoRoot 'target/debug/examples/tun-h3-smoke.exe'))) { throw 'Build the tun-h3-smoke example first.' }
New-Item -ItemType Directory -Path (Join-Path $repoRoot '.local') -Force | Out-Null
if ($Client) { $env:HTTP_CAPTURE_TUN_TEST_CLIENT = (Resolve-Path -LiteralPath $Client).Path }
Set-Content -LiteralPath (Join-Path $repoRoot '.local/tun-h3-smoke-result.txt') -Value 'execution=Not completed; see tun-h3-smoke.log'
& (Join-Path $repoRoot 'target/debug/examples/tun-h3-smoke.exe') *> (Join-Path $repoRoot '.local/tun-h3-smoke.log')
$testExit = $LASTEXITCODE
$addresses = @(Get-NetIPAddress | Where-Object { $_.IPAddress -eq '172.31.255.1' -or $_.IPAddress -eq 'fdfe:dcba:9876::1' })
$helpers = @(Get-Process -Name http-capture-tun -ErrorAction SilentlyContinue)
Add-Content -LiteralPath (Join-Path $repoRoot '.local/tun-h3-smoke-result.txt') -Value "helperExited=$($helpers.Count -eq 0)`ntunAddressesRemoved=$($addresses.Count -eq 0)"
if ($addresses.Count -gt 0 -or $helpers.Count -gt 0) { throw 'TUN cleanup check failed' }
exit $testExit
