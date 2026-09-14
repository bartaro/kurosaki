param([switch]$Offline)
$ErrorActionPreference = 'Stop'
# Build from the repository containing this script and remember caller
# flags so the finally block can restore the process environment.
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$previousRustFlags = $env:RUSTFLAGS
Push-Location $repositoryRoot
try {
    # Request the static C runtime for the Windows x64 CLI while retaining
    # existing Rust flags. --locked prevents dependency resolution changes.
    $env:RUSTFLAGS = (($previousRustFlags + ' -C target-feature=+crt-static').Trim())
    $cargoArgs = @('build','--release','--locked','--target','x86_64-pc-windows-msvc','-p','kurosaki-cli','--bin','kurosaki')
    # Offline mode also disables Cargo network access during the build;
    # required dependencies must already be cached.
    if ($Offline) { $cargoArgs += '--offline' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    # Resolve Cargo's actual target directory after the successful build,
    # including any configured target-directory override.
    $metadataText = & cargo metadata --no-deps --format-version 1 --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Unable to locate the Cargo target directory' }
    $metadata = $metadataText | ConvertFrom-Json
    $binary = Join-Path $metadata.target_directory 'x86_64-pc-windows-msvc/release/kurosaki.exe'
    # Replace the repository-root executable only after both native Cargo
    # commands succeed. This script does not execute tests or package an archive.
    Copy-Item -LiteralPath $binary -Destination (Join-Path $repositoryRoot 'kurosaki.exe') -Force
    Write-Host 'Ready: kurosaki.exe (Windows x64 CLI)'
} finally {
    # Restore caller flags and working directory even when a build, metadata
    # query or executable copy raises an error.
    $env:RUSTFLAGS = $previousRustFlags
    Pop-Location
}
