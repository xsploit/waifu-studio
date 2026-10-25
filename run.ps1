# Build and launch from any working directory, including paths containing spaces.
$ErrorActionPreference = 'Stop'
$saved = @{}
foreach ($key in @('PATH', 'WAIFU_STUDIO_ASSET_ROOT', 'WAIFU_STUDIO_BUNDLE', 'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS')) {
    $saved[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
try {
    $rustupBin = if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $env:USERPROFILE '.cargo\bin' }
    if (Test-Path -LiteralPath (Join-Path $rustupBin 'rustup.exe')) {
        $env:PATH = "$rustupBin;$env:PATH"
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'Install Rust with rustup and the Visual Studio Desktop development with C++ workload first.'
    }
    if (-not $env:WAIFU_STUDIO_ASSET_ROOT) { $env:WAIFU_STUDIO_ASSET_ROOT = Join-Path $PSScriptRoot 'assets' }
    if (-not $env:WAIFU_STUDIO_BUNDLE) { $env:WAIFU_STUDIO_BUNDLE = Join-Path $env:WAIFU_STUDIO_ASSET_ROOT 'characters\bundle.json' }
    if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target\windows' }
    if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '4' }
    Push-Location -LiteralPath $PSScriptRoot
    try {
        & cargo run --release --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml') --bin waifu-studio -- @args
        $code = $LASTEXITCODE
    } finally { Pop-Location }
} finally {
    foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key, $saved[$key], 'Process') }
}
exit $code
