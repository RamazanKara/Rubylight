# Kept separate from the elevated entry point so tests can substitute the
# installer process and driver-state query without touching Windows devices.
function Install-UsbIpTransport([string]$Installer, [string]$Log) {
    $state = Get-UsbIpState
    if ($state.Existing) {
        if (!$state.Ready) { throw 'An existing USB/IP installation is incomplete. Restart Windows or repair USBip separately; Rubylight has left it unchanged.' }
        Write-Host 'Reusing the existing shared USB/IP installation without changing it.'
        return 0
    }
    # Upstream installs its signed drivers and bundled VC++ runtime. Never
    # restart Windows automatically. No GUI, SDK, debug symbols or shortcuts.
    $arguments = @('/SP-', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/RESTARTEXITCODE=3010',
        '/COMPONENTS="main,client"', '/TASKS="vcredist"',
        ('/DIR="{0}"' -f (Join-Path $env:ProgramFiles 'USBip')), ('/LOG="{0}"' -f $Log))
    $process = Start-Process -FilePath $Installer -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) { throw "USB/IP installer failed with exit code $($process.ExitCode). See $Log" }
    if ($process.ExitCode -eq 3010) { Write-Host 'RESTART_REQUIRED' }
    if (!(Get-UsbIpState).Ready) { throw "USB/IP setup did not register its controller driver and client. Restart Windows or repair USBip; see $Log" }
    Write-Host 'USB/IP installed for Steam Deck controllers and DualSense USB audio and waveform haptics.'
    return $process.ExitCode
}
