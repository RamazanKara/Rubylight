# Input echo window for the WebRTC keyboard and mouse leg (rust/tests/webrtc_input_e2e.cjs).
# Runs in the logged-in user's desktop on the test machine (the 780M laptop, never the dev PC): a
# small topmost window that keeps focus and records what reaches it.
#   * low-level hooks: every INJECTED key and mouse event with its virtual key, position, button or
#     wheel delta (events from a real keyboard or mouse are not recorded);
#   * window messages: WM_KEYDOWN/UP, WM_SYSKEYDOWN/UP and WM_CHAR, so text is seen as characters;
#   * raw input: the relative mouse deltas before pointer acceleration.
# One JSON object per line goes to -Log and to a tiny HTTP server on -Port (/lines?from=N,
# /state, /stop), so the harness reads it over the LAN without a copy step.
param(
  [Parameter(Mandatory)][string]$Log,
  [int]$Port = 48150,
  [string]$Bind = '0.0.0.0',
  [int]$Width = 1000,
  [int]$Height = 640
)
$ErrorActionPreference = 'Stop'
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

public class EchoForm : Form, IMessageFilter {
  delegate IntPtr HookProc(int code, IntPtr wParam, IntPtr lParam);
  [StructLayout(LayoutKind.Sequential)] struct KBD { public uint vk, scan, flags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct MSL { public int x, y; public uint data, flags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct RAWDEV { public ushort page, usage; public uint flags; public IntPtr target; }
  [DllImport("user32.dll", SetLastError = true)] static extern IntPtr SetWindowsHookEx(int id, HookProc proc, IntPtr module, uint thread);
  [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr hook);
  [DllImport("user32.dll")] static extern IntPtr CallNextHookEx(IntPtr hook, int code, IntPtr w, IntPtr l);
  [DllImport("kernel32.dll")] static extern IntPtr GetModuleHandle(string name);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool RegisterRawInputDevices(RAWDEV[] devices, uint count, uint size);
  [DllImport("user32.dll")] static extern uint GetRawInputData(IntPtr raw, uint command, byte[] data, ref uint size, uint header);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr hwnd);
  [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
  [StructLayout(LayoutKind.Sequential)] struct POINT { public int x, y; }

  readonly List<string> lines = new List<string>();
  readonly object gate = new object();
  readonly Stopwatch clock = Stopwatch.StartNew();
  readonly double baseMs = (DateTime.UtcNow - new DateTime(1970, 1, 1)).TotalMilliseconds;
  StreamWriter file;
  IntPtr keyHook, mouseHook;
  HookProc keyProc, mouseProc;
  bool hadFocus;
  readonly System.Windows.Forms.Timer keeper = new System.Windows.Forms.Timer();
  TcpListener server;

  public static void Run(string log, string bind, int port, int width, int height) {
    SetProcessDPIAware(); // positions are physical pixels, like the injector's
    Application.EnableVisualStyles();
    var form = new EchoForm(log, bind, port, width, height);
    Application.Run(form);
  }

  EchoForm(string log, string bind, int port, int width, int height) {
    file = new StreamWriter(new FileStream(log, FileMode.Create, FileAccess.Write, FileShare.ReadWrite), new UTF8Encoding(false)) { AutoFlush = true };
    Text = "rl-input-echo";
    TopMost = true;
    ShowInTaskbar = true;
    FormBorderStyle = FormBorderStyle.FixedToolWindow;
    StartPosition = FormStartPosition.Manual;
    var area = Screen.PrimaryScreen.Bounds;
    Size = new Size(width, height);
    Location = new Point(area.Left + (area.Width - width) / 2, area.Top + (area.Height - height) / 2);
    BackColor = Color.FromArgb(30, 30, 40);
    var label = new Label { Text = "Input echo for the WebRTC test. Injected input is logged here and nowhere else.", ForeColor = Color.White, AutoSize = true, Location = new Point(10, 10) };
    Controls.Add(label);
    Application.AddMessageFilter(this);
    keyProc = KeyHook; mouseProc = MouseHook;
    var module = GetModuleHandle(null);
    keyHook = SetWindowsHookEx(13, keyProc, module, 0);
    mouseHook = SetWindowsHookEx(14, mouseProc, module, 0);
    if (keyHook == IntPtr.Zero || mouseHook == IntPtr.Zero) throw new InvalidOperationException("SetWindowsHookEx failed: " + Marshal.GetLastWin32Error());
    server = new TcpListener(IPAddress.Parse(bind), port);
    server.Start();
    var thread = new Thread(Serve) { IsBackground = true };
    thread.Start();
    keeper.Interval = 250;
    keeper.Tick += delegate { Keep(); };
    keeper.Start();
  }

  protected override void OnHandleCreated(EventArgs e) {
    base.OnHandleCreated(e);
    var device = new RAWDEV { page = 1, usage = 2, flags = 0x100, target = Handle }; // RIDEV_INPUTSINK
    RegisterRawInputDevices(new[] { device }, 1, (uint)Marshal.SizeOf(typeof(RAWDEV)));
  }

  protected override void OnShown(EventArgs e) {
    base.OnShown(e);
    var screens = new StringBuilder();
    foreach (var s in Screen.AllScreens) {
      if (screens.Length > 0) screens.Append(',');
      screens.AppendFormat("{{\"name\":\"{0}\",\"left\":{1},\"top\":{2},\"width\":{3},\"height\":{4},\"primary\":{5}}}",
        s.DeviceName.Replace("\\", "\\\\"), s.Bounds.Left, s.Bounds.Top, s.Bounds.Width, s.Bounds.Height, s.Primary ? "true" : "false");
    }
    var v = SystemInformation.VirtualScreen;
    Emit("{{\"src\":\"echo\",\"ev\":\"ready\",\"pid\":{0},\"screens\":[{1}],\"virtual\":{{\"left\":{2},\"top\":{3},\"width\":{4},\"height\":{5}}},\"window\":{{\"left\":{6},\"top\":{7},\"width\":{8},\"height\":{9}}},\"layout\":{10}}}",
      Process.GetCurrentProcess().Id, screens, v.Left, v.Top, v.Width, v.Height, Bounds.Left, Bounds.Top, Bounds.Width, Bounds.Height,
      (int)((long)InputLanguage.CurrentInputLanguage.Handle & 0xFFFF));
    Keep();
  }

  // Alt or F10 on their own would open the window's menu and swallow the next keys.
  protected override void WndProc(ref Message m) {
    if (m.Msg == 0x112 && (m.WParam.ToInt64() & 0xFFF0) == 0xF100) return; // WM_SYSCOMMAND, SC_KEYMENU
    base.WndProc(ref m);
  }

  // ---- the log ------------------------------------------------------------------------------
  void Emit(string format, params object[] args) {
    var t = baseMs + clock.Elapsed.TotalMilliseconds;
    var body = string.Format(System.Globalization.CultureInfo.InvariantCulture, format, args);
    // body is `{...}`: put the timestamp in front.
    var line = "{\"t\":" + t.ToString("F3", System.Globalization.CultureInfo.InvariantCulture) + "," + body.Substring(1);
    lock (gate) { lines.Add(line); file.WriteLine(line); }
  }

  // ---- low-level hooks: injected events only ---------------------------------------------------
  IntPtr KeyHook(int code, IntPtr w, IntPtr l) {
    if (code >= 0) {
      var k = (KBD)Marshal.PtrToStructure(l, typeof(KBD));
      if ((k.flags & 0x10) != 0) { // LLKHF_INJECTED
        int m = w.ToInt32();
        bool down = m == 0x100 || m == 0x104;
        // For VK_PACKET (0xE7), `scan` carries the UTF-16 unit of a text message.
        Emit("{{\"src\":\"ll\",\"ev\":\"{0}\",\"vk\":{1},\"scan\":{2},\"ext\":{3}}}",
          down ? "kd" : "ku", k.vk, k.scan, (k.flags & 1) != 0 ? "true" : "false");
      }
    }
    return CallNextHookEx(keyHook, code, w, l);
  }

  IntPtr MouseHook(int code, IntPtr w, IntPtr l) {
    if (code >= 0) {
      var m = (MSL)Marshal.PtrToStructure(l, typeof(MSL));
      if ((m.flags & 1) != 0) { // LLMHF_INJECTED
        int msg = w.ToInt32();
        string ev = null, button = null; int delta = 0;
        switch (msg) {
          case 0x200: ev = "mm"; break;
          case 0x201: ev = "bd"; button = "left"; break;
          case 0x202: ev = "bu"; button = "left"; break;
          case 0x204: ev = "bd"; button = "right"; break;
          case 0x205: ev = "bu"; button = "right"; break;
          case 0x207: ev = "bd"; button = "middle"; break;
          case 0x208: ev = "bu"; button = "middle"; break;
          case 0x20B: ev = "bd"; button = ((m.data >> 16) & 0xffff) == 1 ? "x1" : "x2"; break;
          case 0x20C: ev = "bu"; button = ((m.data >> 16) & 0xffff) == 1 ? "x1" : "x2"; break;
          case 0x20A: ev = "wheel"; delta = (short)((m.data >> 16) & 0xffff); break;
          case 0x20E: ev = "hwheel"; delta = (short)((m.data >> 16) & 0xffff); break;
        }
        if (ev == "mm") Emit("{{\"src\":\"ll\",\"ev\":\"mm\",\"x\":{0},\"y\":{1}}}", m.x, m.y);
        else if (button != null) Emit("{{\"src\":\"ll\",\"ev\":\"{0}\",\"button\":\"{1}\",\"x\":{2},\"y\":{3}}}", ev, button, m.x, m.y);
        else if (ev != null) Emit("{{\"src\":\"ll\",\"ev\":\"{0}\",\"delta\":{1},\"x\":{2},\"y\":{3}}}", ev, delta, m.x, m.y);
      }
    }
    return CallNextHookEx(mouseHook, code, w, l);
  }

  // ---- window messages and raw input ------------------------------------------------------------
  public bool PreFilterMessage(ref Message m) {
    switch (m.Msg) {
      case 0x100: case 0x101: case 0x104: case 0x105:
        Emit("{{\"src\":\"wnd\",\"ev\":\"{0}\",\"vk\":{1}}}",
          m.Msg == 0x100 ? "WM_KEYDOWN" : m.Msg == 0x101 ? "WM_KEYUP" : m.Msg == 0x104 ? "WM_SYSKEYDOWN" : "WM_SYSKEYUP", m.WParam.ToInt64());
        break;
      case 0x102: case 0x106:
        Emit("{{\"src\":\"wnd\",\"ev\":\"{0}\",\"char\":{1}}}", m.Msg == 0x102 ? "WM_CHAR" : "WM_SYSCHAR", m.WParam.ToInt64());
        break;
      case 0xFF: {
        uint size = 0;
        GetRawInputData(m.LParam, 0x10000003, null, ref size, 24);
        var data = new byte[size];
        if (size >= 40 && GetRawInputData(m.LParam, 0x10000003, data, ref size, 24) == size && BitConverter.ToUInt32(data, 0) == 0) {
          int dx = BitConverter.ToInt32(data, 24 + 12), dy = BitConverter.ToInt32(data, 24 + 16);
          ushort flags = BitConverter.ToUInt16(data, 24);
          if (dx != 0 || dy != 0 || (flags & 1) != 0) Emit("{{\"src\":\"raw\",\"ev\":\"mm\",\"dx\":{0},\"dy\":{1},\"absolute\":{2}}}", dx, dy, (flags & 1) != 0 ? "true" : "false");
        }
        break;
      }
    }
    return false;
  }

  // ---- focus ------------------------------------------------------------------------------------
  // Keep the window in front and focused: a stolen focus is logged and taken back.
  void Keep() {
    bool has = GetForegroundWindow() == Handle;
    if (has != hadFocus) { hadFocus = has; Emit("{{\"src\":\"echo\",\"ev\":\"focus\",\"has\":{0}}}", has ? "true" : "false"); }
    if (has) return;
    TopMost = true;
    var foreground = GetForegroundWindow();
    uint pid;
    uint other = foreground == IntPtr.Zero ? 0 : GetWindowThreadProcessId(foreground, out pid);
    uint mine = GetCurrentThreadId();
    if (other != 0 && other != mine) AttachThreadInput(mine, other, true);
    BringWindowToTop(Handle);
    SetForegroundWindow(Handle);
    Activate();
    if (other != 0 && other != mine) AttachThreadInput(mine, other, false);
  }

  // ---- reading the log over HTTP ------------------------------------------------------------------
  void Serve() {
    while (true) {
      TcpClient client;
      try { client = server.AcceptTcpClient(); } catch { return; }
      try {
        client.ReceiveTimeout = 2000;
        var stream = client.GetStream();
        var buffer = new byte[2048];
        int n = stream.Read(buffer, 0, buffer.Length);
        var request = Encoding.ASCII.GetString(buffer, 0, Math.Max(n, 0));
        var path = request.Split(' ').Length > 1 ? request.Split(' ')[1] : "/";
        string body = "";
        if (path.StartsWith("/lines")) {
          int from = 0;
          var q = path.IndexOf("from=");
          if (q >= 0) int.TryParse(path.Substring(q + 5).Split('&')[0], out from);
          var sb = new StringBuilder();
          lock (gate) { for (int i = Math.Max(from, 0); i < lines.Count; i++) sb.Append(lines[i]).Append('\n'); }
          body = sb.ToString();
        } else if (path.StartsWith("/state")) {
          lock (gate) body = "{\"lines\":" + lines.Count + ",\"focus\":" + (hadFocus ? "true" : "false") + "}";
        } else if (path.StartsWith("/stop")) {
          body = "stopping";
          BeginInvoke(new MethodInvoker(delegate { Close(); }));
        }
        var payload = Encoding.UTF8.GetBytes(body);
        var head = Encoding.ASCII.GetBytes("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: " + payload.Length + "\r\nConnection: close\r\n\r\n");
        stream.Write(head, 0, head.Length); stream.Write(payload, 0, payload.Length);
      } catch { }
      try { client.Close(); } catch { }
    }
  }

  protected override void OnFormClosed(FormClosedEventArgs e) {
    keeper.Stop();
    if (keyHook != IntPtr.Zero) UnhookWindowsHookEx(keyHook);
    if (mouseHook != IntPtr.Zero) UnhookWindowsHookEx(mouseHook);
    try { server.Stop(); } catch { }
    Emit("{{\"src\":\"echo\",\"ev\":\"closed\"}}");
    try { file.Close(); } catch { }
    base.OnFormClosed(e);
  }
}
'@
[EchoForm]::Run($Log, $Bind, $Port, $Width, $Height)
