param([switch]$Offline)
$ErrorActionPreference = 'Stop'
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$previousRustFlags = $env:RUSTFLAGS
Push-Location $repositoryRoot
try {
    $env:RUSTFLAGS = (($previousRustFlags + ' -C target-feature=+crt-static').Trim())
    $cargoArgs = @('build','--release','--locked','--target','x86_64-pc-windows-msvc','-p','kurosaki-cli','--bin','kurosaki')
    if ($Offline) { $cargoArgs += '--offline' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $metadataText = & cargo metadata --no-deps --format-version 1 --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Unable to locate the Cargo target directory' }
    $metadata = $metadataText | ConvertFrom-Json
    $binary = Join-Path $metadata.target_directory 'x86_64-pc-windows-msvc/release/kurosaki.exe'
    Copy-Item -LiteralPath $binary -Destination (Join-Path $repositoryRoot 'kurosaki.exe') -Force
    Write-Host 'Ready: kurosaki.exe (Windows x64 CLI)'
} finally {
    $env:RUSTFLAGS = $previousRustFlags
    Pop-Location
}
