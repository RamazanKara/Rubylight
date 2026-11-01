# Builds butterpollo-service.exe in release, the same file from the same
# sources (rust/release/README.md, "The service executable"): rust/build.ps1
# packages what this leaves in <TargetDirectory>\release. Run it with
# MSYS2's ucrt64\bin on PATH, as rust/build.ps1 does, for windres.
param(
    [Parameter(Mandatory)][string]$TargetDirectory,
    [string]$Toolchain = '+1.98.1-x86_64-pc-windows-gnu'
)
$ErrorActionPreference = 'Stop'
# Only this crate's settings: a developer's flags (lld, say) or a compiler
# wrapper would change the file or the way it is made. The +toolchain above
# overrides rust-toolchain.toml, and the linker and flags set below override
# .cargo/config.toml's (test_release.py checks that it sets nothing else).
# The caller's values come back when the build ends.
$cleared = @('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
    'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS', 'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER',
    'RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_INCREMENTAL') +
    @(Get-ChildItem Env: | Where-Object Name -like 'CARGO_PROFILE_*' | ForEach-Object Name)
$saved = @{}
foreach ($name in $cleared) {
    $value = [Environment]::GetEnvironmentVariable($name, 'Process')
    if ($null -ne $value) { $saved[$name] = $value }
    [Environment]::SetEnvironmentVariable($name, $null, 'Process')
}
# Cargo reads its configuration from the folders above the current one.
Push-Location -LiteralPath $PSScriptRoot
try {
    # Linked with the Rust toolchain's own MinGW linker, start files and import
    # libraries, not MSYS2's: an MSYS2 update would change the file.
    $sysroot = (& rustc $Toolchain --print sysroot)
    if ($LASTEXITCODE -ne 0) { throw "rustc $Toolchain --print sysroot failed with exit code $LASTEXITCODE" }
    $mingw = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained'
    $linker = Join-Path $mingw 'x86_64-w64-mingw32-gcc.exe'
    if (!(Test-Path -LiteralPath $linker)) { throw "The toolchain's MinGW linker is missing: $linker" }
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $linker
    # Panic messages name the source files of dependencies by their path in
    # Cargo's registry, which holds the user's name.
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $env:CARGO_ENCODED_RUSTFLAGS = @(
        '-Clink-self-contained=yes',
        "--remap-path-prefix=$((Resolve-Path -LiteralPath $cargoHome).ProviderPath)=/cargo"
    ) -join [char]0x1f
    & cargo $Toolchain build --release --locked --target-dir $TargetDirectory
    if ($LASTEXITCODE -ne 0) { throw "The service build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
    foreach ($name in $cleared) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
    }
}
$service = Join-Path $TargetDirectory 'release\butterpollo-service.exe'
"butterpollo-service.exe $((Get-FileHash -LiteralPath $service -Algorithm SHA256).Hash.ToLowerInvariant())"
