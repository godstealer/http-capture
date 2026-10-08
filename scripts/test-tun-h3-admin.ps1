param([string]$Client = '', [switch]$LocalEcho, [switch]$UdpProbes, [switch]$ObserveUdp)
$ErrorActionPreference = 'Stop'
if ($ObserveUdp -and !$UdpProbes) { throw 'ObserveUdp requires UdpProbes' }
if ($ObserveUdp) { $env:HTTP_CAPTURE_TUN_TEST_OBSERVE_UDP = '1' } else { Remove-Item Env:HTTP_CAPTURE_TUN_TEST_OBSERVE_UDP -ErrorAction SilentlyContinue }
if ($UdpProbes -and !$LocalEcho) { throw 'UdpProbes requires LocalEcho' }
if ($UdpProbes) { $env:HTTP_CAPTURE_TUN_TEST_UDP_PROBES = '1' } else { Remove-Item Env:HTTP_CAPTURE_TUN_TEST_UDP_PROBES -ErrorAction SilentlyContinue }
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot
if (Get-Process -Name http-capture-tun -ErrorAction SilentlyContinue) { throw 'Another TUN helper is running; skip test.' }
if (!(Test-Path -LiteralPath (Join-Path $repoRoot '.local/tun/http-capture-tun.exe'))) { throw 'Missing TUN helper: run scripts/build-tun.ps1 first.' }
if (!(Test-Path -LiteralPath (Join-Path $repoRoot 'target/debug/examples/tun-h3-smoke.exe'))) { throw 'Build the tun-h3-smoke example first.' }
New-Item -ItemType Directory -Path (Join-Path $repoRoot '.local') -Force | Out-Null
if ($LocalEcho -and !$Client) { throw 'LocalEcho requires -Client pointing to the Go H3 test client.' }
if ($Client) { $env:HTTP_CAPTURE_TUN_TEST_CLIENT = (Resolve-Path -LiteralPath $Client).Path } else { Remove-Item Env:HTTP_CAPTURE_TUN_TEST_CLIENT -ErrorAction SilentlyContinue }
if ($LocalEcho) { $env:HTTP_CAPTURE_TUN_TEST_LOCAL = '1' } else { Remove-Item Env:HTTP_CAPTURE_TUN_TEST_LOCAL -ErrorAction SilentlyContinue }
Set-Content -LiteralPath (Join-Path $repoRoot '.local/tun-h3-smoke-result.txt') -Value 'execution=Not completed; see tun-h3-smoke.log'
Set-Content -LiteralPath (Join-Path $repoRoot '.local/tun-h3-helper.log') -Value ''
& (Join-Path $repoRoot 'target/debug/examples/tun-h3-smoke.exe') *> (Join-Path $repoRoot '.local/tun-h3-smoke.log')
$testExit = $LASTEXITCODE
$addresses = @(Get-NetIPAddress | Where-Object { $_.IPAddress -eq '172.31.255.1' -or $_.IPAddress -eq 'fdfe:dcba:9876::1' })
$helpers = @(Get-Process -Name http-capture-tun -ErrorAction SilentlyContinue)
Add-Content -LiteralPath (Join-Path $repoRoot '.local/tun-h3-smoke-result.txt') -Value "helperExited=$($helpers.Count -eq 0)`ntunAddressesRemoved=$($addresses.Count -eq 0)"
if ($addresses.Count -gt 0 -or $helpers.Count -gt 0) { throw 'TUN cleanup check failed' }
exit $testExit
