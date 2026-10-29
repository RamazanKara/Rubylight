# A background stream's Nucleus Co-op session for a one-player game without a
# hub handler, on this PC as SYSTEM (the host's ignored test
# nucleus::tests::a_background_stream_without_a_hub_handler_on_this_pc): the
# app goes through the host's admission, Rubylight's handler and list entry
# are written, Nucleus Co-op starts the game, its window is captured with a
# frame that is not black, the first player's A reaches the session's XInput
# index, and the compatibility settings are as before afterwards. The
# handler, entry and settings are removed again, also on a failure. Nucleus
# Co-op must be installed (the console's Set up co-op) and not running.
# Exit code: the test's.
param(
    [Parameter(Mandatory)] [string] $Out,
    # The game's .exe, or else the Steam app whose .exe Steam starts.
    [string] $Game = '',
    [string] $SteamApp = '',
    [int] $Hold = 20
)
$ErrorActionPreference = 'Stop'
if (-not $Game -and -not $SteamApp) { throw 'pass -Game <exe> or -SteamApp <id>' }
New-Item -ItemType Directory -Force $Out | Out-Null
$built = cargo +1.98.1-x86_64-pc-windows-gnu test -p butterpollo --bin butterpollo --locked --no-run --message-format=json 2>$null
$exe = ($built | ForEach-Object { try { $_ | ConvertFrom-Json } catch { $null } } |
    Where-Object { $_ -and $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.target.name -eq 'butterpollo' -and $_.profile.test } |
    Select-Object -Last 1).executable
if (-not $exe) { throw 'the butterpollo test binary was not built' }

$log = Join-Path $Out 'session.log'
$done = Join-Path $Out 'done.txt'
$script = Join-Path $Out 'run.cmd'
Remove-Item $done -ErrorAction SilentlyContinue
$lines = @(
    '@echo off',
    "set RUBYLIGHT_NUCLEUS_HOLD=$Hold"
)
if ($Game) { $lines += "set RUBYLIGHT_NUCLEUS_GAME=$Game" }
if ($SteamApp) { $lines += "set RUBYLIGHT_NUCLEUS_STEAM_APP=$SteamApp" }
$lines += "`"$exe`" --ignored --exact nucleus::tests::a_background_stream_without_a_hub_handler_on_this_pc --nocapture > `"$log`" 2>&1"
$lines += "echo %ERRORLEVEL% > `"$done`""
Set-Content -Path $script -Value $lines -Encoding ascii

# SYSTEM, like the service: Nucleus Co-op starts as the signed-in user.
$task = 'RubylightNucleusGeneric'
schtasks /Create /F /TN $task /RU SYSTEM /RL HIGHEST /SC ONCE /ST 00:00 /TR "cmd /c `"$script`"" | Out-Null
try {
    schtasks /Run /TN $task | Out-Null
    $deadline = (Get-Date).AddMinutes(10)
    while (-not (Test-Path $done)) {
        if ((Get-Date) -gt $deadline) { throw "no result within 10 minutes: $log" }
        Start-Sleep -Seconds 2
    }
} finally {
    schtasks /Delete /F /TN $task | Out-Null
}
$code = [int](Get-Content $done -Raw).Trim()
Get-Content $log | Select-String -Pattern '^(handler|running after|game process|capture|pad A|stopped in|compatibility|cleanup|handler and entry|warning|test result)|panicked|Error'
exit $code
