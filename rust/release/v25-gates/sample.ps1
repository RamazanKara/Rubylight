# Samples the display driver's WUDFHost and the isolated test host every 10 s for $Seconds:
# handle counts and dedicated GPU memory (per process, and the adapter's total). Elevated.
param([Parameter(Mandatory)] [int] $Seconds, [Parameter(Mandatory)] [string] $Out)
$wudf = (tasklist /m rubylight_vdisplay.dll /fo csv | ConvertFrom-Csv | Select-Object -First 1).PID
"utc,elapsed_s,wudf_pid,wudf_handles,wudf_dedicated_mb,host_pid,host_handles,host_dedicated_mb,adapter_dedicated_mb" | Set-Content $Out
$start = Get-Date
while (((Get-Date) - $start).TotalSeconds -lt $Seconds) {
    $hostProcess = Get-CimInstance Win32_Process -Filter "Name='butterpollo.exe'" | Where-Object { $_.CommandLine -like '*--config-dir*fence-ring*' } | Select-Object -First 1
    $hostPid = if ($hostProcess) { $hostProcess.ProcessId } else { 0 }
    $memory = (Get-Counter '\GPU Process Memory(*)\Dedicated Usage' -ErrorAction SilentlyContinue).CounterSamples
    $of = { param($id) [math]::Round((($memory | Where-Object InstanceName -like "pid_${id}_*" | Measure-Object CookedValue -Sum).Sum) / 1MB, 1) }
    $adapter = [math]::Round(((Get-Counter '\GPU Adapter Memory(*)\Dedicated Usage').CounterSamples | Measure-Object CookedValue -Maximum).Maximum / 1MB, 1)
    $wudfHandles = (Get-Process -Id $wudf -ErrorAction SilentlyContinue).HandleCount
    $hostHandles = if ($hostPid) { (Get-Process -Id $hostPid -ErrorAction SilentlyContinue).HandleCount } else { 0 }
    "$((Get-Date).ToUniversalTime().ToString('o')),$([int]((Get-Date) - $start).TotalSeconds),$wudf,$wudfHandles,$(& $of $wudf),$hostPid,$hostHandles,$(& $of $hostPid),$adapter" | Add-Content $Out
    Start-Sleep -Seconds 10
}
