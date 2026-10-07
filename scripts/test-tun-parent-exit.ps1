$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot
if (Get-Process -Name http-capture-tun -ErrorAction SilentlyContinue) { throw 'Another TUN helper is running; skip this test.' }
& (Join-Path $repoRoot 'target/debug/tun-smoke.exe') --parent-exit *> (Join-Path $repoRoot '.local/tun-parent-exit.log')
if ($LASTEXITCODE -ne 0) { throw 'Parent-exit test failed to start TUN' }
$deadline = [DateTime]::UtcNow.AddSeconds(20)
do {
    Start-Sleep -Milliseconds 250
    $helpers = @(Get-Process -Name http-capture-tun -ErrorAction SilentlyContinue)
} while ($helpers.Count -gt 0 -and [DateTime]::UtcNow -lt $deadline)
$addresses = @(Get-NetIPAddress -ErrorAction Stop | Where-Object { $_.IPAddress -eq '172.31.255.1' -or $_.IPAddress -eq 'fdfe:dcba:9876::1' })
$report = "helperExited=$($helpers.Count -eq 0)`ntunAddressesRemoved=$($addresses.Count -eq 0)`n"
Set-Content -LiteralPath (Join-Path $repoRoot '.local/tun-parent-exit-result.txt') -Value $report -Encoding utf8
if ($helpers.Count -gt 0 -or $addresses.Count -gt 0) { throw 'TUN cleanup after parent exit failed; inspect the report.' }
