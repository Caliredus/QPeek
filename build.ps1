<#
.SYNOPSIS
    QPeek derleme betiği.

.DESCRIPTION
    1) vendor/Win-CodexBar çekirdeğini (Rust) release modda derler.
    2) Çıkan `codexbar.exe` dosyasını dist/ altına `codexbar-cli.exe` adıyla kopyalar
       (Win-CodexBar'ın kendi release betiği de CLI'yi bu şekilde adlandırır).
    3) Çekirdeğin LICENSE ve NOTICE dosyalarını dist/third-party/ altına koyar.

    4) src/qpeek arayüzünü (Rust + Slint) derler, dist/QPeek.exe olarak koyar ve
       arayüz bağımlılıklarının lisanslarını dist/third-party/ altına kopyalar.

.EXAMPLE
    .\build.ps1
#>
[CmdletBinding()]
param(
    [switch]$DebugBuild
)

$ErrorActionPreference = 'Stop'
$root      = $PSScriptRoot
$core      = Join-Path $root 'vendor\Win-CodexBar'
$manifest  = Join-Path $core 'rust\Cargo.toml'
$targetDir = Join-Path $root 'target'
$dist      = Join-Path $root 'dist'
$buildProfile   = if ($DebugBuild) { 'debug' } else { 'release' }

if (-not (Test-Path $manifest)) {
    throw "Çekirdek bulunamadı: $manifest`nÖnce: git submodule update --init"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo bulunamadı. Rust kurulu değil ya da terminal yeniden açılmadı.'
}

Write-Host "==> Çekirdek derleniyor ($buildProfile)" -ForegroundColor Cyan
$cargoArgs = @('build', '--manifest-path', $manifest, '--bin', 'codexbar', '--target-dir', $targetDir)
if (-not $DebugBuild) { $cargoArgs += '--release' }
# cargo ilerleme ve uyarılarını stderr'e yazar; Windows PowerShell 5.1 bunları
# hata sayıp 'Stop' altında betiği keser. Başarıyı yalnız çıkış koduna göre ölçüyoruz.
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
& cargo @cargoArgs
$ErrorActionPreference = $prevEap
if ($LASTEXITCODE -ne 0) { throw "cargo build başarısız (çıkış $LASTEXITCODE)" }

Write-Host '==> dist/ hazırlanıyor' -ForegroundColor Cyan
New-Item -ItemType Directory -Force (Join-Path $dist 'third-party\Win-CodexBar') | Out-Null
Copy-Item (Join-Path $targetDir "$buildProfile\codexbar.exe") (Join-Path $dist 'codexbar-cli.exe') -Force
Copy-Item (Join-Path $core 'LICENSE') (Join-Path $dist 'third-party\Win-CodexBar\LICENSE') -Force
Copy-Item (Join-Path $core 'NOTICE')  (Join-Path $dist 'third-party\Win-CodexBar\NOTICE')  -Force

$uiManifest = Join-Path $root 'src\qpeek\Cargo.toml'
Write-Host "==> Arayüz derleniyor ($buildProfile)" -ForegroundColor Cyan
$uiArgs = @('build', '--manifest-path', $uiManifest, '--target-dir', $targetDir)
if (-not $DebugBuild) { $uiArgs += '--release' }
$ErrorActionPreference = 'Continue'
& cargo @uiArgs
$ErrorActionPreference = $prevEap
if ($LASTEXITCODE -ne 0) { throw "Arayüz derlemesi başarısız (çıkış $LASTEXITCODE)" }
Copy-Item (Join-Path $targetDir "$buildProfile\qpeek.exe") (Join-Path $dist 'QPeek.exe') -Force

# Arayüz bağımlılıklarının lisans metinleri (Slint: Royalty-free 2.0, atıf Hakkında ekranında).
$ErrorActionPreference = 'Continue'
$meta = & cargo metadata --format-version 1 --manifest-path $uiManifest 2>$null | ConvertFrom-Json
$ErrorActionPreference = $prevEap
foreach ($name in 'slint', 'tray-icon') {
    $pkg = $meta.packages | Where-Object { $_.name -eq $name } | Select-Object -First 1
    if (-not $pkg) { continue }
    $src = Split-Path $pkg.manifest_path
    $dst = Join-Path $dist "third-party\$name"
    New-Item -ItemType Directory -Force $dst | Out-Null
    Get-ChildItem $src -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING)' } | Copy-Item -Destination $dst -Force
    $licDir = Join-Path $src 'LICENSES'
    if (Test-Path $licDir) { Copy-Item $licDir -Destination $dst -Recurse -Force }
}

foreach ($f in 'QPeek.exe', 'codexbar-cli.exe') {
    $exe = Get-Item (Join-Path $dist $f)
    Write-Host ("==> Tamam: {0} ({1:N1} MB)" -f $exe.FullName, ($exe.Length / 1MB)) -ForegroundColor Green
}
