# Build, run and package the app on Windows (the counterpart of the macOS
# `cargo dev` workflow). No administrator rights needed: the build tools go
# into a per-user folder.
#
#   tools\windows.ps1 setup            one time: MSYS2, libclang, the static FFmpeg
#   tools\windows.ps1 dev [files...]   build (release) and run the app
#   tools\windows.ps1 build            release build only
#   tools\windows.ps1 test [args...]   cargo test
#   tools\windows.ps1 check            cargo fmt --check + clippy
#   tools\windows.ps1 bundle [-Version v]   zip + installer into dist\
#   tools\windows.ps1 cargo <args...>  any cargo command in the build environment
#
# Needs: Rust (x86_64-pc-windows-msvc) and Visual Studio 2022 (or its Build
# Tools) with the "Desktop development with C++" workload.
#
# The C++ engine (`cpp-engine` feature) is used when REEL_ENGINE_DIR (or
# ..\reel-studio, see .cargo/config.toml) holds the engine sources;
# otherwise the app builds on the Rust stand-in.
#
# Where the tools go: $env:REEL_WIN_TOOLS, default
# %LOCALAPPDATA%\reel-studio-build (msys64\, libclang\). FFmpeg goes into
# .ffmpeg\windows-x64 in the repo, the same place CI keeps it.
param(
    [Parameter(Position = 0)]
    [ValidateSet("setup", "dev", "build", "test", "check", "bundle", "cargo", "env")]
    [string] $Command = "dev",
    [string] $Version = "",
    [Parameter(Position = 1, ValueFromRemainingArguments)] [string[]] $Rest = @()
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$tools = if ($env:REEL_WIN_TOOLS) { $env:REEL_WIN_TOOLS } else { Join-Path $env:LOCALAPPDATA "reel-studio-build" }
$msys = Join-Path $tools "msys64"
$ffmpeg = Join-Path $root ".ffmpeg\windows-x64"
$libclangVersion = "18.1.1"

function Step($text) { Write-Host "==> $text" -ForegroundColor Cyan }

# curl.exe (part of Windows): Windows PowerShell's Invoke-WebRequest can
# stall on large downloads. A dropped connection resumes where it stopped
# (-C -) until the file has the size the server announced (curl can end a
# resumed transfer early without an error).
function Get-File([string] $url, [string] $out) {
    if (Test-Path $out) { Remove-Item $out }
    $size = 0
    $head = & curl.exe -fsSIL --connect-timeout 30 $url
    foreach ($line in $head) {
        if ($line -match '^content-length:\s*(\d+)') { $size = [int64]$Matches[1] }
    }
    for ($attempt = 1; $attempt -le 30; $attempt++) {
        & curl.exe -fsSL -C - --connect-timeout 30 --speed-limit 4096 --speed-time 60 -o $out $url
        $code = $LASTEXITCODE
        $have = if (Test-Path $out) { (Get-Item $out).Length } else { 0 }
        if ($code -eq 0 -and ($size -eq 0 -or $have -eq $size)) { return }
        if ($size -gt 0 -and $have -gt $size) { Remove-Item $out; $have = 0 }
        Write-Host ("  download interrupted at {0:N1} / {1:N1} MB (curl {2}), resuming ({3}/30)" -f ($have / 1MB), ($size / 1MB), $code, $attempt)
        Start-Sleep -Seconds 2
    }
    throw "download failed: $url"
}

# ---- MSVC -------------------------------------------------------------------

# The x64 developer environment (cl, link, lib, rc, INCLUDE, LIB) imported
# into this process: FFmpeg's build and bindgen's clang read it.
function Import-Msvc {
    if ($env:VSCMD_ARG_TGT_ARCH -eq "x64" -and (Get-Command cl.exe -ErrorAction SilentlyContinue)) { return }
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { throw "Visual Studio (or its Build Tools) isn't installed" }
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) { throw "Visual Studio has no C++ tools: add the 'Desktop development with C++' workload" }
    $vcvars = Join-Path $vs "VC\Auxiliary\Build\vcvars64.bat"
    $lines = cmd.exe /d /s /c "`"$vcvars`" > nul && set"
    if ($LASTEXITCODE -ne 0) { throw "vcvars64.bat failed" }
    foreach ($line in $lines) {
        if ($line -match '^([^=]+)=(.*)$') { Set-Item "env:$($Matches[1])" $Matches[2] }
    }
}

# ---- libclang (bindgen, for FFmpeg's bindings) --------------------------------

function Find-Libclang {
    foreach ($dir in @($env:LIBCLANG_PATH, "$env:ProgramFiles\LLVM\bin", (Join-Path $tools "libclang"))) {
        if ($dir -and (Test-Path (Join-Path $dir "libclang.dll"))) { return $dir }
    }
    return $null
}

# libclang.dll alone, from the `libclang` Python wheel (PyPI, SHA-256
# checked): ~25 MB instead of a full LLVM install, and no admin rights.
function Install-Libclang {
    if (Find-Libclang) { return }
    Step "libclang $libclangVersion"
    $meta = Invoke-RestMethod "https://pypi.org/pypi/libclang/$libclangVersion/json"
    $wheel = $meta.urls | Where-Object { $_.filename -like "*win_amd64.whl" } | Select-Object -First 1
    if (-not $wheel) { throw "no win_amd64 wheel for libclang $libclangVersion" }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("libclang-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force $tmp | Out-Null
    try {
        $whl = Join-Path $tmp "libclang.zip"
        Get-File $wheel.url $whl
        $hash = (Get-FileHash $whl -Algorithm SHA256).Hash.ToLower()
        if ($hash -ne $wheel.digests.sha256) { throw "libclang wheel: SHA-256 mismatch" }
        Expand-Archive $whl -DestinationPath (Join-Path $tmp "x")
        $dest = Join-Path $tools "libclang"
        New-Item -ItemType Directory -Force $dest | Out-Null
        $dll = Get-ChildItem (Join-Path $tmp "x") -Recurse -Filter libclang.dll | Select-Object -First 1
        if (-not $dll) { throw "no libclang.dll in the libclang wheel" }
        Copy-Item $dll.FullName $dest
    }
    finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
}

# ---- MSYS2 (only to run FFmpeg's configure + make) ----------------------------

function Invoke-Msys([string] $script) {
    $env:MSYSTEM = "MSYS"
    $env:MSYS2_PATH_TYPE = "inherit" # keeps MSVC's cl / link / lib on the PATH
    $env:CHERE_INVOKING = "1"         # start in the current folder
    & (Join-Path $msys "usr\bin\bash.exe") -lc $script
    if ($LASTEXITCODE -ne 0) { throw "MSYS2: '$script' failed ($LASTEXITCODE)" }
}

function Install-Msys {
    $bash = Join-Path $msys "usr\bin\bash.exe"
    if (-not (Test-Path $bash)) {
        Step "MSYS2 (portable, into $msys)"
        New-Item -ItemType Directory -Force $tools | Out-Null
        $sfx = Join-Path $tools "msys2-base.sfx.exe"
        Get-File "https://repo.msys2.org/distrib/msys2-x86_64-latest.sfx.exe" $sfx
        & $sfx -y "-o$tools" | Out-Null
        Remove-Item $sfx
        if (-not (Test-Path $bash)) { throw "MSYS2 didn't unpack into $msys" }
        # The first login sets up the keyring and home folder.
        Invoke-Msys "true"
    }
    Invoke-Msys "command -v make >/dev/null && command -v nasm >/dev/null && command -v cmp >/dev/null || pacman -S --noconfirm --needed make diffutils nasm"
    # MSYS2's link.exe (coreutils) would shadow MSVC's.
    $link = Join-Path $msys "usr\bin\link.exe"
    if (Test-Path $link) { Move-Item -Force $link "$link.msys" }
}

function Install-Ffmpeg {
    if (Test-Path (Join-Path $ffmpeg "lib\avcodec.lib")) { return }
    Step "FFmpeg (static, LGPL, MSVC) into ${ffmpeg}: takes a while"
    Install-Msys
    Import-Msvc
    Push-Location $root
    try { Invoke-Msys "packaging/windows/build-ffmpeg.sh `"`$(cygpath -u '$ffmpeg')`"" }
    finally { Pop-Location }
}

# ---- cargo --------------------------------------------------------------------

function Use-BuildEnv {
    if (-not (Test-Path (Join-Path $ffmpeg "lib\avcodec.lib")) -and -not $env:FFMPEG_DIR) {
        throw "no FFmpeg yet: run 'tools\windows.ps1 setup' first"
    }
    $clang = Find-Libclang
    if (-not $clang) { throw "no libclang yet: run 'tools\windows.ps1 setup' first" }
    Import-Msvc
    if (-not $env:FFMPEG_DIR) { $env:FFMPEG_DIR = $ffmpeg }
    $env:LIBCLANG_PATH = $clang
}

# The C++ engine when its sources are there (.cargo/config.toml's default
# is ..\reel-studio\modules\video-engine\cpp).
function Get-Features {
    $engine = if ($env:REEL_ENGINE_DIR) { $env:REEL_ENGINE_DIR } else { Join-Path $root "..\reel-studio\modules\video-engine\cpp" }
    if (Test-Path (Join-Path $engine "src")) { return @("--features", "cpp-engine") }
    Write-Warning "no engine sources at ${engine}: building on the Rust stand-in (set REEL_ENGINE_DIR)"
    return @()
}

function Invoke-Cargo([string[]] $cargoArgs) {
    Write-Host "cargo $($cargoArgs -join ' ')" -ForegroundColor DarkGray
    Push-Location $root
    try { & cargo @cargoArgs }
    finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

switch ($Command) {
    "setup" {
        Install-Libclang
        Install-Ffmpeg
        Step "Ready: 'tools\windows.ps1 dev' builds and runs the app"
    }
    "env" {
        # Dot-source to keep it: . tools\windows.ps1 env
        Use-BuildEnv
        Write-Host "FFMPEG_DIR=$env:FFMPEG_DIR"
        Write-Host "LIBCLANG_PATH=$env:LIBCLANG_PATH"
    }
    "dev" {
        Use-BuildEnv
        $features = Get-Features
        Invoke-Cargo (@("run", "--release") + $features + @("--") + $Rest)
    }
    "build" {
        Use-BuildEnv
        Invoke-Cargo (@("build", "--release", "--locked") + (Get-Features) + $Rest)
    }
    "test" {
        Use-BuildEnv
        Invoke-Cargo (@("test", "--locked") + (Get-Features) + $Rest)
    }
    "check" {
        Use-BuildEnv
        Invoke-Cargo @("fmt", "--check")
        Invoke-Cargo (@("clippy", "--locked", "--all-targets") + (Get-Features) + $Rest)
    }
    "cargo" {
        Use-BuildEnv
        Invoke-Cargo $Rest
    }
    "bundle" {
        Use-BuildEnv
        Invoke-Cargo (@("build", "--release", "--locked") + (Get-Features))
        if (-not $Version) {
            $Version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version = "(.*)"' |
                Select-Object -First 1).Matches[0].Groups[1].Value
        }
        $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
        $userIscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
        if (-not (Get-Command iscc.exe -ErrorAction SilentlyContinue) -and -not (Test-Path $iscc)) {
            if (-not (Test-Path $userIscc)) {
                Step "Inno Setup (per user)"
                winget install --id JRSoftware.InnoSetup --scope user --silent --accept-package-agreements --accept-source-agreements | Out-Host
            }
            $env:PATH = (Split-Path $userIscc) + ";" + $env:PATH
        }
        & (Join-Path $root "packaging\windows\bundle.ps1") -Binary (Join-Path $root "target\release\reel-studio-gpui.exe") `
            -Version $Version -Arch x64 -Out (Join-Path $root "dist")
    }
}
