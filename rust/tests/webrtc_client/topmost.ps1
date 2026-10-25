# Puts the browser window whose title contains $Marker at the top left of the primary display and
# above every other window, without taking focus. The clock window must be fully visible on the
# captured display for the whole stream, whatever else is open. Only windows of the given process
# are touched: other browsers with the same title (a second run, the user's own) stay as they are.
param([Parameter(Mandatory)][string]$Marker, [Parameter(Mandatory)][int]$ProcessId, [int]$Width = 1500, [int]$Height = 720)
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class RlWindows {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hgt, uint flags);
  public static int Pin(string marker, uint owner, int w, int h) {
    int count = 0;
    EnumWindows((hwnd, l) => {
      var text = new StringBuilder(512);
      GetWindowText(hwnd, text, 512);
      uint pid;
      GetWindowThreadProcessId(hwnd, out pid);
      if (pid == owner && IsWindowVisible(hwnd) && text.ToString().Contains(marker)) {
        ShowWindow(hwnd, 9);
        // HWND_TOPMOST, SWP_NOACTIVATE | SWP_SHOWWINDOW
        SetWindowPos(hwnd, new IntPtr(-1), 0, 0, w, h, 0x0010 | 0x0040);
        count++;
      }
      return true;
    }, IntPtr.Zero);
    return count;
  }
}
'@
$pinned = [RlWindows]::Pin($Marker, [uint32]$ProcessId, $Width, $Height)
if ($pinned -lt 1) { Write-Error "no visible window of process $ProcessId titled like '$Marker'"; exit 1 }
