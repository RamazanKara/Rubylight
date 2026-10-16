# rust/README.md "Running the native checks" for one worktree, with every report the tests
# ask for and a moving primary display. Skips what the README skips, plus the two crash
# subprocess entry points (their parent test starts them; run directly, reporter_process
# exits the whole test binary).
# native.ps1 -Tree C:\src\rl-fence-ring -Out native-fence [-Only <filter>]
param([Parameter(Mandatory)] [string] $Tree, [Parameter(Mandatory)] [string] $Out, [string] $Only)
. C:\src\rust-env.ps1
$root = 'C:\src\butterpollo-release\fence-ring'
$d = Join-Path $root $Out
New-Item -ItemType Directory -Force $d | Out-Null
$pkg = 'C:\src\butterpollo-release\2.4.5\release\butterpollo-rust-release'
$env:PATH = "$pkg;" + $env:PATH
$env:BUTTERPOLLO_TEST_OPUS_ROOT = $pkg
$env:BUTTERPOLLO_TEST_FFMPEG = (Get-Command ffmpeg).Source
$env:BUTTERPOLLO_TEST_FFPROBE = (Get-Command ffprobe).Source
$env:BUTTERPOLLO_TEST_RFI_REPORT = "$d\rfi.json"
$env:BUTTERPOLLO_TEST_GPU_REPORT = "$d\gpu.json"
$env:BUTTERPOLLO_TEST_REFERENCE_BUDGET_REPORT = "$d\reference-budget.json"
$env:BUTTERPOLLO_TEST_AUDIO_REPORT = "$d\audio.json"
$env:BUTTERPOLLO_TEST_GEOMETRY_DIRECTORY = "$d\geometry"
Set-Location $Tree
Copy-Item "$env:BUTTERPOLLO_PYROWAVE_ROOT\bin\libpyrowave-shared-1.dll" target\qa\debug\deps\ -Force -ErrorAction SilentlyContinue
$probe = Start-Process -PassThru -WindowStyle Hidden C:\src\butterpollo-release\hw-fps\fixtures3\motion_probe.exe -ArgumentList '\\.\DISPLAY1', '2400', "$d\motion.json", 'current', '128'
try {
    $filter = if ($Only) { @($Only) } else { @() }
    cargo test -p butterpollo-windows --locked --target-dir target\qa -- --ignored @filter --skip native_nvenc_loss_recovery_and_444_hdr_decode --skip native_av1_geometry_and_hdr_are_preserved --skip primary_multitouch_and_secondary_isolation_match_native_hid_reports --skip two_announced_touchpads_share_one_native_surface_side_by_side --skip crash::tests::reporter_process --skip crash::tests::crashing_process --test-threads=1 --nocapture *> "$d\native.log"
    "exit=$LASTEXITCODE" | Set-Content "$d\exit.txt"
} finally {
    Stop-Process -Id $probe.Id -ErrorAction SilentlyContinue
}
