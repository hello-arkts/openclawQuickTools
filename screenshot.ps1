# Launch and screenshot test
# Kill existing
try { Stop-Process -Name openclaw-launcher -Force -ErrorAction SilentlyContinue } catch {}
Start-Sleep -Seconds 1

# Launch
$proc = Start-Process -FilePath "E:\openclaw-launcher\src-tauri\target\release\openclaw-launcher.exe" -PassThru
Start-Sleep -Seconds 5

# Use .NET to capture window screenshot via a compiled C# snippet
$cs = @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class Screenshotter {
    [DllImport("user32.dll")] static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    public static void Capture(string title, string outPath) {
        IntPtr hwnd = FindWindow(null, title);
        if (hwnd == IntPtr.Zero) {
            Console.Error.WriteLine("WINDOW_NOT_FOUND");
            return;
        }
        ShowWindow(hwnd, 9);
        SetForegroundWindow(hwnd);
        System.Threading.Thread.Sleep(300);
        RECT rect;
        GetWindowRect(hwnd, out rect);
        int w = rect.Right - rect.Left;
        int h = rect.Bottom - rect.Top;
        if (w <= 0 || h <= 0) {
            Console.Error.WriteLine("ZERO_SIZE");
            return;
        }
        using (Bitmap bmp = new Bitmap(w, h))
        using (Graphics g = Graphics.FromImage(bmp)) {
            g.CopyFromScreen(rect.Left, rect.Top, 0, 0, new Size(w, h));
            bmp.Save(outPath, ImageFormat.Png);
        }
        Console.WriteLine("OK " + w + "x" + h);
    }
}
'@

# Compile and run
Add-Type -TypeDefinition $cs -ReferencedAssemblies System.Drawing -ErrorAction Stop
[Screenshotter]::Capture("OpenClaw Launcher", "E:\openclaw-launcher\test-screenshot.png")

# Cleanup
Start-Sleep -Seconds 1
Stop-Process -Name openclaw-launcher -Force -ErrorAction SilentlyContinue
Write-Output "Done"
