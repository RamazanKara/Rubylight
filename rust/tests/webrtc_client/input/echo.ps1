# Input echo window for the WebRTC keyboard and mouse leg (rust/tests/webrtc_input_e2e.cjs).
# Runs in the logged-in user's desktop on the test machine (the 780M laptop, never the dev PC): a
# small topmost window that keeps focus and records what reaches it.
#   * injected key and mouse events (from SendInput, which has no device), logged as src "ll" with
#     their virtual key, position, button or wheel delta; events from a real keyboard or mouse are
#     not recorded. They are read from Raw Input (WM_INPUT, RIDEV_INPUTSINK), which Windows posts to
#     this window: unlike a low-level hook it never makes the injecting SendInput wait for this UI
#     thread, so the echo adds no latency of its own. -Hooks reads them from WH_KEYBOARD_LL and
#     WH_MOUSE_LL instead, as the first version of this window did;
#   * window messages: WM_KEYDOWN/UP, WM_SYSKEYDOWN/UP and WM_CHAR, so text is seen as characters;
#   * raw input: the relative mouse deltas before pointer acceleration.
# One JSON object per line goes to -Log and to a tiny HTTP server on -Bind:-Port (/lines?from=N,
# /state, /arm?ms=N, /release, /stop), so the harness reads it over the LAN without a copy step.
# The window records real keystrokes and mouse moves only while the harness has armed it (/arm) for an
# injected sequence, and closes itself after -MaxSeconds so a killed harness leaves nothing behind.
# /release sends key-ups for Shift, Ctrl, Alt, Win and Q and button-ups for all five buttons.
param(
  [Parameter(Mandatory)][string]$Log,
  [int]$Port = 48150,
  [string]$Bind = '0.0.0.0',
  [int]$Width = 1000,
  [int]$Height = 640,
  [switch]$Hooks,
  [int]$MaxSeconds = 900,
  [string]$Nonce = ''
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
  [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
  [DllImport("user32.dll")] static extern bool RegisterRawInputDevices(RAWDEV[] devices, uint count, uint size);
  [DllImport("user32.dll")] static extern uint GetRawInputData(IntPtr raw, uint command, byte[] data, ref uint size, uint header);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
  [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr hwnd);
  [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  [StructLayout(LayoutKind.Sequential)] struct POINT { public int x, y; }

  readonly List<string> lines = new List<string>();
  readonly object gate = new object();
  readonly Stopwatch clock = Stopwatch.StartNew();
  readonly double baseMs = (DateTime.UtcNow - new DateTime(1970, 1, 1)).TotalMilliseconds;
  StreamWriter file;
  IntPtr keyHook, mouseHook;
  HookProc keyProc, mouseProc;
  bool hadFocus;
  readonly bool hooks;
  readonly string nonce;
  long armedUntil; // clock milliseconds: real keystrokes and mouse moves are recorded only before this
  readonly System.Windows.Forms.Timer keeper = new System.Windows.Forms.Timer();
  readonly System.Windows.Forms.Timer life = new System.Windows.Forms.Timer();
  TcpListener server;

  public static void Run(string log, string bind, int port, int width, int height, bool hooks, int maxSeconds, string nonce) {
    SetProcessDPIAware(); // positions are physical pixels, like the injector's
    Application.EnableVisualStyles();
    var form = new EchoForm(log, bind, port, width, height, hooks, maxSeconds, nonce);
    Application.Run(form);
  }

  EchoForm(string log, string bind, int port, int width, int height, bool hooks, int maxSeconds, string nonce) {
    this.hooks = hooks;
    this.nonce = nonce;
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
    if (hooks) {
      keyProc = KeyHook; mouseProc = MouseHook;
      var module = GetModuleHandle(null);
      keyHook = SetWindowsHookEx(13, keyProc, module, 0);
      mouseHook = SetWindowsHookEx(14, mouseProc, module, 0);
      if (keyHook == IntPtr.Zero || mouseHook == IntPtr.Zero) throw new InvalidOperationException("SetWindowsHookEx failed: " + Marshal.GetLastWin32Error());
    }
    server = new TcpListener(IPAddress.Parse(bind), port);
    server.Start();
    var thread = new Thread(Serve) { IsBackground = true };
    thread.Start();
    keeper.Interval = 250;
    keeper.Tick += delegate { Keep(); };
    keeper.Start();
    life.Interval = Math.Max(maxSeconds, 1) * 1000;
    life.Tick += delegate { life.Stop(); Emit("{{\"src\":\"echo\",\"ev\":\"expired\"}}"); Close(); };
    life.Start();
  }

  bool Armed() { return clock.ElapsedMilliseconds < armedUntil; }

  protected override void OnHandleCreated(EventArgs e) {
    base.OnHandleCreated(e);
    // RIDEV_INPUTSINK: the mouse and the keyboard, whether or not this window has the focus.
    var devices = new[] {
      new RAWDEV { page = 1, usage = 2, flags = 0x100, target = Handle },
      new RAWDEV { page = 1, usage = 6, flags = 0x100, target = Handle },
    };
    if (!RegisterRawInputDevices(devices, 2, (uint)Marshal.SizeOf(typeof(RAWDEV)))) throw new InvalidOperationException("RegisterRawInputDevices failed: " + Marshal.GetLastWin32Error());
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
    Emit("{{\"src\":\"echo\",\"ev\":\"ready\",\"pid\":{0},\"screens\":[{1}],\"virtual\":{{\"left\":{2},\"top\":{3},\"width\":{4},\"height\":{5}}},\"window\":{{\"left\":{6},\"top\":{7},\"width\":{8},\"height\":{9}}},\"layout\":{10},\"nonce\":\"{11}\"}}",
      Process.GetCurrentProcess().Id, screens, v.Left, v.Top, v.Width, v.Height, Bounds.Left, Bounds.Top, Bounds.Width, Bounds.Height,
      (int)((long)InputLanguage.CurrentInputLanguage.Handle & 0xFFFF), nonce);
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
  // Raw Input has no report of a VK_PACKET (text sent as characters), so those are read where they
  // surface: the key message with VK_PACKET, then the WM_CHAR with the UTF-16 unit.
  bool packet;
  long packetUnit;

  public bool PreFilterMessage(ref Message m) {
    switch (m.Msg) {
      case 0x100: case 0x101: case 0x104: case 0x105:
        if (!hooks && m.WParam.ToInt64() == 0xE7) {
          if (m.Msg == 0x100 || m.Msg == 0x104) packet = true;
          else Emit("{{\"src\":\"ll\",\"ev\":\"ku\",\"vk\":231,\"scan\":{0},\"ext\":false}}", packetUnit);
        }
        if (Armed()) Emit("{{\"src\":\"wnd\",\"ev\":\"{0}\",\"vk\":{1}}}",
          m.Msg == 0x100 ? "WM_KEYDOWN" : m.Msg == 0x101 ? "WM_KEYUP" : m.Msg == 0x104 ? "WM_SYSKEYDOWN" : "WM_SYSKEYUP", m.WParam.ToInt64());
        break;
      case 0x102: case 0x106:
        if (!hooks && packet) {
          packet = false;
          packetUnit = m.WParam.ToInt64();
          Emit("{{\"src\":\"ll\",\"ev\":\"kd\",\"vk\":231,\"scan\":{0},\"ext\":false}}", packetUnit);
        }
        if (Armed()) Emit("{{\"src\":\"wnd\",\"ev\":\"{0}\",\"char\":{1}}}", m.Msg == 0x102 ? "WM_CHAR" : "WM_SYSCHAR", m.WParam.ToInt64());
        break;
      case 0xFF: {
        uint size = 0;
        GetRawInputData(m.LParam, 0x10000003, null, ref size, 24);
        var data = new byte[size];
        if (size >= 40 && GetRawInputData(m.LParam, 0x10000003, data, ref size, 24) == size) {
          uint type = BitConverter.ToUInt32(data, 0);
          // SendInput has no device: the header's handle is null for injected input only.
          bool injected = BitConverter.ToInt64(data, 8) == 0;
          if (type == 0) RawMouse(data, injected);
          else if (type == 1 && !hooks && injected) RawKey(data);
        }
        break;
      }
    }
    return false;
  }

  void RawMouse(byte[] data, bool injected) {
    int dx = BitConverter.ToInt32(data, 24 + 12), dy = BitConverter.ToInt32(data, 24 + 16);
    ushort flags = BitConverter.ToUInt16(data, 24);
    bool absolute = (flags & 1) != 0;
    if (injected && Armed() && (dx != 0 || dy != 0 || absolute)) Emit("{{\"src\":\"raw\",\"ev\":\"mm\",\"dx\":{0},\"dy\":{1},\"absolute\":{2}}}", dx, dy, absolute ? "true" : "false");
    if (hooks || !injected) return;
    POINT p;
    GetCursorPos(out p);
    if (dx != 0 || dy != 0 || absolute) {
      if (absolute) {
        // 0..65535 over the primary display, or over the virtual desktop with MOUSE_VIRTUAL_DESKTOP (2).
        bool desk = (flags & 2) != 0;
        int left = desk ? GetSystemMetrics(76) : 0, top = desk ? GetSystemMetrics(77) : 0;
        int w = desk ? GetSystemMetrics(78) : GetSystemMetrics(0), h = desk ? GetSystemMetrics(79) : GetSystemMetrics(1);
        p.x = left + (int)(((long)dx * (w - 1) + 32767) / 65535);
        p.y = top + (int)(((long)dy * (h - 1) + 32767) / 65535);
      }
      Emit("{{\"src\":\"ll\",\"ev\":\"mm\",\"x\":{0},\"y\":{1}}}", p.x, p.y);
    }
    ushort buttons = BitConverter.ToUInt16(data, 24 + 4);
    short wheel = BitConverter.ToInt16(data, 24 + 6);
    var names = new[] { "left", "right", "middle", "x1", "x2" };
    // RI_MOUSE_*_DOWN and _UP, two bits a button in this order: left, right, middle, button 4, button 5.
    int[] bits = { 0x1, 0x4, 0x10, 0x40, 0x100 };
    for (int i = 0; i < bits.Length; i++) {
      if ((buttons & bits[i]) != 0) Emit("{{\"src\":\"ll\",\"ev\":\"bd\",\"button\":\"{0}\",\"x\":{1},\"y\":{2}}}", names[i], p.x, p.y);
      if ((buttons & (bits[i] << 1)) != 0) Emit("{{\"src\":\"ll\",\"ev\":\"bu\",\"button\":\"{0}\",\"x\":{1},\"y\":{2}}}", names[i], p.x, p.y);
    }
    if ((buttons & 0x400) != 0) Emit("{{\"src\":\"ll\",\"ev\":\"wheel\",\"delta\":{0},\"x\":{1},\"y\":{2}}}", wheel, p.x, p.y);
    if ((buttons & 0x800) != 0) Emit("{{\"src\":\"ll\",\"ev\":\"hwheel\",\"delta\":{0},\"x\":{1},\"y\":{2}}}", wheel, p.x, p.y);
  }

  // The keyboard as the low-level hook reports it: the side-specific virtual key, the extended bit and,
  // for VK_PACKET, the UTF-16 unit.
  void RawKey(byte[] data) {
    ushort make = BitConverter.ToUInt16(data, 24), flags = BitConverter.ToUInt16(data, 24 + 2), vk = BitConverter.ToUInt16(data, 24 + 6);
    if (vk == 0xFF) return; // the fake Shift around a numpad key
    bool e0 = (flags & 2) != 0;
    if (vk == 0x10) vk = (ushort)(make == 0x36 ? 0xA1 : 0xA0);
    // Right Alt on a layout with AltGr is preceded by a Control that has Alt's scan code (0x38): the
    // hook calls it Left Control, and the plan leaves it out.
    else if (vk == 0x11) vk = (ushort)(make == 0x38 ? 0xA2 : e0 ? 0xA3 : 0xA2);
    else if (vk == 0x12) vk = (ushort)(e0 ? 0xA5 : 0xA4);
    Emit("{{\"src\":\"ll\",\"ev\":\"{0}\",\"vk\":{1},\"scan\":{2},\"ext\":{3}}}", (flags & 1) != 0 ? "ku" : "kd", vk, make, e0 ? "true" : "false");
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

  // Key-ups for both Shift, Ctrl, Alt and Win keys and Q, and button-ups for all five buttons: what a
  // killed run can leave held on the desktop. Sent after the host is gone.
  void ReleaseAll() {
    Emit("{{\"src\":\"echo\",\"ev\":\"release\"}}");
    foreach (byte vk in new byte[] { 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C, 0x51 }) {
      bool ext = vk == 0xA3 || vk == 0xA5 || vk == 0x5B || vk == 0x5C;
      keybd_event(vk, 0, 2u | (ext ? 1u : 0u), UIntPtr.Zero);
    }
    mouse_event(0x4, 0, 0, 0, UIntPtr.Zero);
    mouse_event(0x10, 0, 0, 0, UIntPtr.Zero);
    mouse_event(0x40, 0, 0, 0, UIntPtr.Zero);
    mouse_event(0x100, 0, 0, 1, UIntPtr.Zero);
    mouse_event(0x100, 0, 0, 2, UIntPtr.Zero);
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
        } else if (path.StartsWith("/arm")) {
          int ms = 0;
          var a = path.IndexOf("ms=");
          if (a >= 0) int.TryParse(path.Substring(a + 3).Split('&')[0], out ms);
          armedUntil = ms > 0 ? clock.ElapsedMilliseconds + Math.Min(ms, 120000) : 0;
          body = "armed " + ms;
        } else if (path.StartsWith("/release")) {
          ReleaseAll();
          body = "released";
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
    life.Stop();
    if (keyHook != IntPtr.Zero) UnhookWindowsHookEx(keyHook);
    if (mouseHook != IntPtr.Zero) UnhookWindowsHookEx(mouseHook);
    try { server.Stop(); } catch { }
    Emit("{{\"src\":\"echo\",\"ev\":\"closed\"}}");
    try { file.Close(); } catch { }
    base.OnFormClosed(e);
  }
}
'@
[EchoForm]::Run($Log, $Bind, $Port, $Width, $Height, [bool]$Hooks, $MaxSeconds, $Nonce)
