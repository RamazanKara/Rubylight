# Scripted content for pyrowave-loopback.ps1: a detailed texture scrolling across the primary display.
param([int]$Seconds = 80)
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Windows.Forms;
using System.Diagnostics;
using System.Runtime.InteropServices;
public class Anim : Form {
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    Bitmap tile; int w, h, t, frames; Stopwatch sw = Stopwatch.StartNew(); int seconds;
    public static void Run(int seconds) { SetProcessDPIAware(); Application.Run(new Anim(seconds)); }
    Anim(int seconds) {
        this.seconds = seconds;
        FormBorderStyle = FormBorderStyle.None; ShowInTaskbar = false; StartPosition = FormStartPosition.Manual;
        var b = Screen.PrimaryScreen.Bounds; w = 3400; h = b.Height; Location = b.Location; Size = new Size(w, h);
        SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.UserPaint | ControlStyles.OptimizedDoubleBuffer, true);
        tile = new Bitmap(w * 2, h, PixelFormat.Format32bppPArgb);
        var rnd = new Random(7); var data = tile.LockBits(new Rectangle(0, 0, w * 2, h), ImageLockMode.WriteOnly, PixelFormat.Format32bppPArgb);
        var row = new byte[data.Stride];
        for (int y = 0; y < h; y++) {
            for (int x = 0; x < w * 2; x++) {
                // Smooth field plus fine noise: detailed like a photograph, not white noise.
                double v = 128 + 70 * Math.Sin(x / 37.0 + y / 91.0) * Math.Cos(y / 23.0) + rnd.Next(-34, 35);
                byte c = (byte)Math.Max(0, Math.Min(255, v));
                row[x * 4] = c; row[x * 4 + 1] = (byte)(255 - c / 2); row[x * 4 + 2] = (byte)((c * 3) & 255); row[x * 4 + 3] = 255;
            }
            Marshal.Copy(row, 0, new IntPtr(data.Scan0.ToInt64() + (long)y * data.Stride), data.Stride);
        }
        tile.UnlockBits(data);
        Application.Idle += (s, e) => { while (!IsDisposed && Application.MessageLoop && !PeekMsg()) { Invalidate(); Update(); Tick(); } };
    }
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr h; public uint m; public IntPtr w, l; public uint t; public int x, y; }
    [DllImport("user32.dll")] static extern bool PeekMessage(out MSG m, IntPtr h, uint a, uint b, uint r);
    bool PeekMsg() { MSG m; return PeekMessage(out m, IntPtr.Zero, 0, 0, 0); }
    void Tick() {
        if (sw.Elapsed.TotalSeconds > seconds) { Console.WriteLine("painted " + frames + " frames in " + sw.Elapsed.TotalSeconds.ToString("F1") + " s"); Close(); }
    }
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override void OnPaint(PaintEventArgs e) {
        t = (t + 11) % w; frames++;
        e.Graphics.CompositingMode = System.Drawing.Drawing2D.CompositingMode.SourceCopy;
        e.Graphics.DrawImage(tile, new Rectangle(0, 0, w, h), t, 0, w, h, GraphicsUnit.Pixel);
    }
}
'@
[Anim]::Run($Seconds)
