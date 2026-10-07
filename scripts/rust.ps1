$CargoArguments = $args
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$env:CARGO_HOME = Join-Path $projectRoot '.local/cargo'
$env:RUSTUP_HOME = Join-Path $projectRoot '.local/rustup'
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
$nasmDir = Join-Path $projectRoot '.local/build-tools/nasm-3.02'
if (Test-Path (Join-Path $nasmDir 'nasm.exe')) { $env:PATH = "$nasmDir;$env:PATH" }
$clangDir = Join-Path $projectRoot '.local/build-tools/clang/clang/native'
if (!$env:LIBCLANG_PATH -and (Test-Path (Join-Path $clangDir 'libclang.dll'))) { $env:LIBCLANG_PATH = $clangDir }

$vsShell = 'D:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\Launch-VsDevShell.ps1'
if (Test-Path -LiteralPath $vsShell) {
    & $vsShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
}
Set-Location -LiteralPath $projectRoot
& cargo @CargoArguments
exit $LASTEXITCODE
