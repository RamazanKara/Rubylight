# Finds the running emulator's window, pins it on top at a fixed place on the primary monitor and prints the screen
# rectangle of its display area (the window's client area) in physical pixels, for capture-video.ps1 -HostCapture.
# Emulator windows show the device at a scale that fits the monitor; the capture takes that area as it is drawn.
# -Height sizes the window so its display area is that tall (the width follows the screen's shape).
param([int]$Left = 40, [int]$Top = 40, [int]$Height = 0, [string]$Title = 'Android Emulator')
$ErrorActionPreference = 'Stop'
if (-not ('NightjarDemo.Win32' -as [type])) {
    Add-Type -Namespace NightjarDemo -Name Win32 -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetWindowPos(System.IntPtr hWnd, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr hWnd, out RECT rect);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool GetClientRect(System.IntPtr hWnd, out RECT rect);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool ClientToScreen(System.IntPtr hWnd, ref POINT point);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)] public struct POINT { public int X, Y; }
'@
}
$null = [NightjarDemo.Win32]::SetProcessDPIAware()
$window = Get-Process qemu-system-x86_64, emulator -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 -and $_.MainWindowTitle -like "$Title*" } | Select-Object -First 1
if (-not $window) { throw 'No emulator window: start the AVD with a window (no -no-window).' }
$handle = $window.MainWindowHandle
# HWND_TOPMOST, keep the size (SWP_NOSIZE), show it.
$null = [NightjarDemo.Win32]::SetWindowPos($handle, [IntPtr](-1), $Left, $Top, 0, 0, 0x0001 -bor 0x0040)
Start-Sleep -Milliseconds 500
if ($Height -gt 0) {
    # The window's frame is its outer size less its client size; the emulator keeps the display's shape.
    $outer = New-Object NightjarDemo.Win32+RECT
    $inner = New-Object NightjarDemo.Win32+RECT
    $null = [NightjarDemo.Win32]::GetWindowRect($handle, [ref]$outer)
    $null = [NightjarDemo.Win32]::GetClientRect($handle, [ref]$inner)
    $width = [int][Math]::Round($Height * $inner.Right / $inner.Bottom)
    $null = [NightjarDemo.Win32]::SetWindowPos($handle, [IntPtr](-1), $Left, $Top,
        $width + ($outer.Right - $outer.Left) - $inner.Right, $Height + ($outer.Bottom - $outer.Top) - $inner.Bottom, 0x0040)
    Start-Sleep -Seconds 1
}
$rect = New-Object NightjarDemo.Win32+RECT
$null = [NightjarDemo.Win32]::GetClientRect($handle, [ref]$rect)
$origin = New-Object NightjarDemo.Win32+POINT
$null = [NightjarDemo.Win32]::ClientToScreen($handle, [ref]$origin)
[pscustomobject]@{ X = $origin.X; Y = $origin.Y; Width = $rect.Right - $rect.Left; Height = $rect.Bottom - $rect.Top; Title = $window.MainWindowTitle }
