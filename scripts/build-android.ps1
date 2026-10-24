param(
    [ValidateSet('debug', 'release')][string]$Configuration = 'debug',
    [ValidateSet('arm64-v8a', 'x86_64', 'both')][string]$Architecture = 'both'
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$androidProject = Join-Path $projectRoot 'apps/mobile/android'
$localSdk = Join-Path $projectRoot '.local/android-toolchain/sdk'
$sdk = if ($env:ANDROID_SDK_ROOT) { $env:ANDROID_SDK_ROOT } elseif ($env:ANDROID_HOME) { $env:ANDROID_HOME } elseif (Test-Path -LiteralPath (Join-Path $localSdk 'platforms/android-36/android.jar')) { $localSdk } else { 'D:\AndroidSDK' }
$ndk = $env:ANDROID_NDK_HOME
if (-not $ndk) {
    $installed = Join-Path $sdk 'ndk'
    if (Test-Path -LiteralPath $installed) {
        $candidate = Get-ChildItem -LiteralPath $installed -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if ($candidate) { $ndk = $candidate.FullName }
    }
}
if (-not $ndk) {
    $localNdk = Join-Path $projectRoot '.local/android-toolchain/android-ndk-r30'
    if (Test-Path -LiteralPath $localNdk) { $ndk = $localNdk }
}
if (-not $ndk -or -not (Test-Path -LiteralPath $ndk)) { throw 'Android NDK is required; set ANDROID_NDK_HOME or install it under the Android SDK.' }
if (-not (Test-Path -LiteralPath (Join-Path $sdk 'platforms/android-36/android.jar'))) {
    throw "Android API 36 platform is required under $sdk. Install platforms;android-36 with sdkmanager."
}
$llvm = Join-Path $ndk 'toolchains/llvm/prebuilt/windows-x86_64/bin'
$gradleExe = Join-Path $projectRoot '.local/android-toolchain/gradle-8.11.1/bin/gradle.bat'
if (-not (Test-Path -LiteralPath $gradleExe)) {
    $gradle = Get-Command gradle -ErrorAction SilentlyContinue
    if (-not $gradle) { throw 'Gradle 8.11.1 or newer is required to build the Android app.' }
    $gradleExe = $gradle.Source
    $versionLine = & $gradleExe --version | Select-String '^Gradle (\d+\.\d+(?:\.\d+)?)$' | Select-Object -First 1
    if (-not $versionLine -or [version]$versionLine.Matches[0].Groups[1].Value -lt [version]'8.11.1') {
        throw "Gradle 8.11.1 or newer is required; found $gradleExe."
    }
}
$env:ANDROID_SDK_ROOT = $sdk
$localRustup = Join-Path $projectRoot '.local/rustup'
$localCargo = Join-Path $projectRoot '.local/cargo'
if (Test-Path -LiteralPath $localRustup) { $env:RUSTUP_HOME = $localRustup }
if (Test-Path -LiteralPath $localCargo) { $env:CARGO_HOME = $localCargo }
$env:PATH = "$llvm;$env:PATH"
$localMake = Join-Path $projectRoot '.local/android-toolchain/bin'
if (Test-Path -LiteralPath (Join-Path $localMake 'make.exe')) { $env:PATH = "$localMake;$env:PATH" }
$ndkMake = Join-Path $ndk 'prebuilt/windows-x86_64/bin'
if (Test-Path -LiteralPath (Join-Path $ndkMake 'make.exe')) { $env:PATH = "$ndkMake;$env:PATH" }
$gitPerl = 'D:\Program Files\Git\usr\bin\perl.exe'
$gitBin = Split-Path -Parent $gitPerl
if (Test-Path -LiteralPath (Join-Path $gitBin 'sh.exe')) {
    # NDK's native Make searches PATH for sh; keep Git sh ahead of the
    # bundled Cygwin shell so Perl and Make agree on /e/... paths.
    $env:PATH = "$ndkMake;$gitBin;$env:PATH"
}
$bundledClang = Join-Path $projectRoot '.local/build-tools/clang/clang/native'
if (-not $env:LIBCLANG_PATH) {
    $env:LIBCLANG_PATH = if (Test-Path -LiteralPath (Join-Path $bundledClang 'libclang.dll')) { $bundledClang } else { $llvm }
}
if ($env:LIBCLANG_PATH -eq $bundledClang) { $env:PATH = "$bundledClang;$env:PATH" }
if (-not (Get-Command perl -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath $gitPerl)) {
    $env:PATH = "$llvm;$(Split-Path -Parent $gitPerl);$env:PATH"
}
$perlModules = Join-Path $projectRoot '.local/android-toolchain/perl-modules'
if (Test-Path -LiteralPath $perlModules) {
    $perlSource = Join-Path $perlModules 'perl-5.38.2'
    if (Test-Path -LiteralPath $perlSource) {
        $libs = @(Join-Path $perlSource 'lib')
        foreach ($directory in @('cpan', 'dist')) {
            $libs += @(Get-ChildItem -LiteralPath (Join-Path $perlSource $directory) -Directory |
                ForEach-Object { Join-Path $_.FullName 'lib' } | Where-Object { Test-Path -LiteralPath $_ })
        }
        $libs += Join-Path $perlModules 'Locale-Maketext-Simple-0.21/lib'
        $env:PERL5LIB = ($libs | ForEach-Object {
            '/' + $_.Substring(0, 1).ToLowerInvariant() + $_.Substring(2).Replace('\', '/')
        }) -join ':'
    }
}
$msysPerl = Join-Path $projectRoot '.local/android-toolchain/msys-perl/bin/perl.exe'
$unixPerl = if (Test-Path -LiteralPath $gitPerl) { $gitPerl } else { $msysPerl }
if (-not $env:OPENSSL_SRC_PERL -and (Test-Path -LiteralPath $unixPerl)) {
    $wrapperSource = Join-Path $projectRoot 'scripts/android-perl-wrapper.rs'
    $wrapper = Join-Path $projectRoot '.local/android-toolchain/bin/http-capture-perl.exe'
    $rustc = Join-Path $env:CARGO_HOME 'bin/rustc.exe'
    if (-not (Test-Path -LiteralPath $rustc)) { throw 'Rust compiler is required for the Android OpenSSL Perl path adapter.' }
    if (-not (Test-Path -LiteralPath $wrapper) -or
        (Get-Item -LiteralPath $wrapperSource).LastWriteTimeUtc -gt (Get-Item -LiteralPath $wrapper).LastWriteTimeUtc) {
        & $rustc --edition 2021 -O $wrapperSource -o $wrapper
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
    $env:HTTP_CAPTURE_UNIX_PERL = $unixPerl
    $env:HTTP_CAPTURE_PROJECT_ROOT = $projectRoot
    $env:OPENSSL_SRC_PERL = $wrapper
    $env:PERL = $wrapper.Replace('\', '/')
}
Set-Location -LiteralPath $projectRoot
$cargo = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin/cargo.exe' } else { (Get-Command cargo -ErrorAction Stop).Source }
$strip = Join-Path $llvm 'llvm-strip.exe'
$architectures = if ($Architecture -eq 'both') { @('arm64-v8a', 'x86_64') } else { @($Architecture) }
foreach ($abi in $architectures) {
    $rustArch = if ($abi -eq 'arm64-v8a') { 'aarch64' } else { 'x86_64' }
    $target = "$rustArch-linux-android"
    $targetEnv = "${rustArch}_linux_android"
    $compilerTarget = "--target=$rustArch-linux-android26"
    $linker = Join-Path $llvm "$rustArch-linux-android26-clang.cmd"
    if (-not (Test-Path -LiteralPath $linker)) { throw "Android NDK linker is missing: $linker" }
    Set-Item -Path "Env:BINDGEN_EXTRA_CLANG_ARGS_$targetEnv" -Value $compilerTarget
    Set-Item -Path "Env:CARGO_TARGET_$($targetEnv.ToUpperInvariant())_LINKER" -Value $linker
    Set-Item -Path "Env:CC_$targetEnv" -Value (Join-Path $llvm 'clang.exe').Replace('\', '/')
    Set-Item -Path "Env:CFLAGS_$targetEnv" -Value $compilerTarget
    Set-Item -Path "Env:AR_$targetEnv" -Value (Join-Path $llvm 'llvm-ar.exe').Replace('\', '/')
    $args = @('build', '--locked', '-p', 'http-capture-android', '--target', $target)
    if ($Configuration -eq 'release') { $args += '--release' }
    & $cargo @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $library = Join-Path $projectRoot "target/$target/$Configuration/libhttp_capture_android.so"
    $destination = Join-Path $androidProject "app/src/main/jniLibs/$abi"
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Copy-Item -LiteralPath $library -Destination (Join-Path $destination 'libhttp_capture_android.so') -Force
    & $strip --strip-debug (Join-Path $destination 'libhttp_capture_android.so')
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
$task = if ($Configuration -eq 'release') { ':app:assembleRelease' } else { ':app:assembleDebug' }
$env:GRADLE_USER_HOME = Join-Path $projectRoot '.local/gradle-user-home'
& $gradleExe -p $androidProject $task
exit $LASTEXITCODE
