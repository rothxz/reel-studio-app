# Wraps a release binary into the Windows downloads a release carries:
#
#   Reel-Studio-<version>-windows-<arch>.zip          "Reel Studio\reel-studio.exe":
#                                                     what the in-app updater
#                                                     installs; runs unzipped
#   Reel-Studio-<version>-windows-<arch>-setup.exe    the installer (per user,
#                                                     Start menu, uninstaller)
#
#   packaging/windows/bundle.ps1 -Binary <exe> -Version <v> -Arch x64 -Out <dir>
#
# The binary must link FFmpeg statically (build with FFMPEG_DIR from
# packaging/windows/build-ffmpeg.sh): the .exe is all there is.
#
# Signing (optional; unsigned, SmartScreen warns on first run):
#   WIN_SIGN_PFX           base64 .pfx (code signing certificate)
#   WIN_SIGN_PASSWORD      its password
param(
    [Parameter(Mandatory)] [string] $Binary,
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [ValidateSet("x64", "arm64")] [string] $Arch,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Resolve-Path (Join-Path $here "..\..")
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("reel-bundle-" + [guid]::NewGuid())
$stage = Join-Path $work "Reel Studio"
New-Item -ItemType Directory -Force -Path $stage | Out-Null

try {
    $exe = Join-Path $stage "reel-studio.exe"
    Copy-Item $Binary $exe

    # Nothing but Windows' own DLLs: no FFmpeg (or other) DLL to ship.
    $dumpbin = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($dumpbin) {
        $deps = & dumpbin.exe /nologo /dependents $exe | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } |
            ForEach-Object { $_.Trim() }
        $deps | ForEach-Object { Write-Host "  needs $_" }
        $foreign = $deps | Where-Object { $_ -match '^(av|sw)[a-z]+-\d+\.dll$' }
        if ($foreign) { throw "the binary links a shared FFmpeg ($foreign); build with a static one" }
    }

    # The icon (the 256 px PNG inside an .ico), for the installer.
    $png = [System.IO.File]::ReadAllBytes((Join-Path $root "docs\images\app-icon-rounded.png"))
    $ico = New-Object System.IO.MemoryStream
    $w = New-Object System.IO.BinaryWriter($ico)
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]1)
    $w.Write([byte]0); $w.Write([byte]0); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$png.Length); $w.Write([uint32]22); $w.Write($png)
    $icoPath = Join-Path $work "reel-studio.ico"
    [System.IO.File]::WriteAllBytes($icoPath, $ico.ToArray())

    $sign = $null
    if ($env:WIN_SIGN_PFX) {
        $pfx = Join-Path $work "sign.pfx"
        [System.IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WIN_SIGN_PFX))
        $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
            Sort-Object FullName -Descending | Select-Object -First 1
        if (-not $signtool) { throw "no signtool.exe (Windows SDK)" }
        $sign = {
            param($file)
            & $signtool.FullName sign /f $pfx /p $env:WIN_SIGN_PASSWORD /fd sha256 `
                /tr http://timestamp.digicert.com /td sha256 /d "Reel Studio" $file
            if ($LASTEXITCODE -ne 0) { throw "signing $file failed" }
        }
        & $sign $exe
    }

    # ---- zip ----------------------------------------------------------------
    $zip = Join-Path $Out "Reel-Studio-$Version-windows-$Arch.zip"
    if (Test-Path $zip) { Remove-Item $zip }
    Compress-Archive -Path $stage -DestinationPath $zip

    # ---- installer -----------------------------------------------------------
    $iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
    if (-not $iscc) {
        $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
        if (-not (Test-Path $iscc)) {
            choco install innosetup --no-progress -y | Out-Host
        }
    }
    & $iscc /Q "/DVersion=$Version" "/DArch=$Arch" "/DSource=$stage" "/DIcon=$icoPath" `
        "/DOutDir=$Out" (Join-Path $here "reel-studio.iss")
    if ($LASTEXITCODE -ne 0) { throw "the installer didn't build" }
    $setup = Join-Path $Out "Reel-Studio-$Version-windows-$Arch-setup.exe"
    if ($sign) { & $sign $setup }

    Get-ChildItem $Out | Format-Table Name, Length
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
