# A background stream's Nucleus Co-op session for a one-player game without a
# hub handler, on this PC as SYSTEM (the host's ignored test
# nucleus::tests::a_background_stream_without_a_hub_handler_on_this_pc): the
# app goes through the host's admission, Rubylight's handler and list entry
# are written, Nucleus Co-op starts the game, its window is captured with a
# frame that is not black, the first player's A reaches the session's XInput
# index, and the compatibility settings are as before afterwards. The
# handler, entry and settings are removed again, also on a failure. Nucleus
# Co-op must be installed (the console's Set up co-op) and not running.
# -Run user (or elevated) asserts how the admission runs it: as the signed-in
# user from their own copy, NucleusCoop.exe and the game unelevated at medium
# integrity, or elevated from the host's.
# -Cold first gives Nucleus' Settings.ini the values that stalled #59
# (folder check on, favourites only, only single-keyboard games listed) and
# puts it back afterwards, also on a failure; the test then checks the click
# within 15 s, the DPI mode set and restored, and the forced values.
# Nucleus' Settings.ini is copied to $Out\Settings.ini.before first. When
# session.log lacks "Settings.ini as before" (the test was stopped at the
# 10-minute timeout, or its cleanup failed), that copy is put back and the
# session's Rubylight-settings.json removed, so no seeded value stays.
# Exit code: the test's.
param(
    [Parameter(Mandatory)] [string] $Out,
    # The game's .exe, or else the Steam app whose .exe Steam starts.
    [string] $Game = '',
    [string] $SteamApp = '',
    [int] $Hold = 20,
    # How the session must run; empty takes whichever the admission chooses.
    [ValidateSet('user', 'elevated')] [string] $Run = '',
    [switch] $Cold
)
$ErrorActionPreference = 'Stop'
if (-not $Game -and -not $SteamApp) { throw 'pass -Game <exe> or -SteamApp <id>' }
New-Item -ItemType Directory -Force $Out | Out-Null
$built = cargo +1.98.1-x86_64-pc-windows-gnu test -p butterpollo --bin butterpollo --locked --no-run --message-format=json 2>$null
$exe = ($built | ForEach-Object { try { $_ | ConvertFrom-Json } catch { $null } } |
    Where-Object { $_ -and $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.target.name -eq 'butterpollo' -and $_.profile.test } |
    Select-Object -Last 1).executable
if (-not $exe) { throw 'the butterpollo test binary was not built' }

# The folder the test runs Nucleus Co-op from (the host's own install).
$nucleus = Join-Path $env:ProgramData 'Butterpollo\nucleus\2.4.2'
$settings = Join-Path $nucleus 'Settings.ini'
$journal = Join-Path $nucleus 'Rubylight-settings.json'
$kept = Join-Path $Out 'Settings.ini.before'
Remove-Item -LiteralPath $kept -ErrorAction SilentlyContinue
if (Test-Path -LiteralPath $settings) {
    Copy-Item -LiteralPath $settings -Destination $kept -Force
    "Settings.ini kept: $kept"
} else {
    "no Settings.ini in $nucleus before the run"
}

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
if ($Run) { $lines += "set RUBYLIGHT_NUCLEUS_RUN=$Run" }
if ($Cold) { $lines += 'set RUBYLIGHT_NUCLEUS_COLD=1' }
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
        if ((Get-Date) -gt $deadline) {
            # Ending the task leaves the test running; stop it too.
            schtasks /End /TN $task | Out-Null
            Get-Process | Where-Object { $_.Path -eq $exe } | Stop-Process -Force
            throw "no result within 10 minutes: $log"
        }
        Start-Sleep -Seconds 2
    }
} finally {
    schtasks /Delete /F /TN $task | Out-Null
    $restored = (Test-Path -LiteralPath $log) -and
        (Select-String -LiteralPath $log -Pattern '^Settings\.ini as before' -Quiet)
    if (-not $restored -and $Cold) {
        if (Test-Path -LiteralPath $kept) {
            Copy-Item -LiteralPath $kept -Destination $settings -Force
            "Settings.ini put back from $kept"
        }
        if (Test-Path -LiteralPath $journal) {
            Remove-Item -LiteralPath $journal -Force
            "removed $journal"
        }
    }
}
$code = [int](Get-Content $done -Raw).Trim()
Get-Content $log | Select-String -Pattern '^(runs |handler|running after|profile clicked|game process|rights|capture|pad A|stopped in|compatibility|crashlog|Settings\.ini|cleanup|handler and entry|warning|test result)|Nucleus Co-op shows|panicked|Error'
exit $code
