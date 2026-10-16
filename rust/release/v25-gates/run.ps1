# One idd-push case of vd_e2e.py, started as SYSTEM in the signed-in session.
# Label: <codec>[-hdr]-<fps>-<build>-r<N>[-s<seconds>], build = base (2.4.5 package) or fence (this work's package).
# The stream lasts 20 s unless -s<seconds> says otherwise.
param([Parameter(Mandatory)] [string] $Label)
$root = 'C:\src\butterpollo-release\fence-ring'
$stage = 'C:\src\butterpollo-release\stage-latency'
if ($Label -notmatch '^(?<codec>hevc|av1)(?<hdr>-hdr)?-(?<fps>\d+)-(?<build>base|fence|v25)-r(?<round>\d+)(-s(?<secs>\d+))?$') { throw "bad label $Label" }
. C:\src\butterpollo-release\settings.ps1
$package = switch ($Matches.build) { 'base' { 'C:\src\butterpollo-release\2.4.5\release\butterpollo-rust-release' } 'fence' { "$root\pkg-fence" } 'v25' { 'C:\src\butterpollo-release\v25\pkg' } }
# 2.5 has no capture setting or driver choice, and its own tools expect the virtual display.
$tools = if ($Matches.build -eq 'v25') { (Get-Content 'C:\src\butterpollo-release\v25\tools.txt' -Raw).Trim() } else { 'C:\src\rl-fence-ring\rust\release' }
$legacy = if ($Matches.build -eq 'v25') { @() } else { @('--config', 'virtual_display_driver=rubylight', '--config', 'capture=idd-push') }
$runs = Get-Content "$root\runs-dir.txt" -Raw
$runs = "$root\$($runs.Trim())"
$env:BUTTERPOLLO_E2E_RUST_LOG = 'info,stages=trace,idd_push=debug'
$env:BUTTERPOLLO_TEST_TIMING_CSV = "$runs\vd-$Label\frames.csv"
$arguments = @("$stage\vd_e2e.py", $tools, '--package', $package, '--work', $runs,
    '--client', 'C:\src\butterpollo-release\hw-fps\fixtures3\moonlight-client.exe', '--codec', "$($Matches.codec)$($Matches.hdr)",
    '--mode', "2560x1440x$($Matches.fps)", '--seconds', $(if ($Matches.secs) { $Matches.secs } else { '20' }), '--bitrate', '80000', '--label', $Label) + $legacy
New-Item -ItemType Directory -Force $runs | Out-Null
& $env:BUTTERPOLLO_TEST_PYTHON @arguments *> "$runs\$Label.run.log"
"exit $LASTEXITCODE $(whoami)" | Set-Content "$runs\$Label.done"
