Add-Type @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class ScreenCap {
    [DllImport("user32.dll")]
    public static extern IntPtr OpenWindowStation(string lpszWinSta, bool fInherit, uint dwDesiredAccess);

    [DllImport("user32.dll")]
    public static extern bool SetProcessWindowStation(IntPtr hWinSta);

    [DllImport("user32.dll")]
    public static extern IntPtr OpenDesktop(string lpszDesktop, uint dwFlags, bool fInherit, uint dwDesiredAccess);

    [DllImport("user32.dll")]
    public static extern bool SetThreadDesktop(IntPtr hDesktop);

    [DllImport("user32.dll")]
    public static extern IntPtr GetDC(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern int ReleaseDC(IntPtr hWnd, IntPtr hDC);

    [DllImport("gdi32.dll")]
    public static extern bool BitBlt(IntPtr hdcDest, int nXDest, int nYDest, int nWidth, int nHeight, IntPtr hdcSrc, int nXSrc, int nYSrc, uint dwRop);

    public const uint GENERIC_ALL = 0x10000000;
    public const uint SRCCOPY = 0x00CC0020;

    public static bool TryCapture(string filename) {
        IntPtr hWinsta = OpenWindowStation("winsta0", false, GENERIC_ALL);
        if (hWinsta != IntPtr.Zero) {
            SetProcessWindowStation(hWinsta);
        }
        IntPtr hDesk = OpenDesktop("default", 0, false, GENERIC_ALL);
        if (hDesk != IntPtr.Zero) {
            SetThreadDesktop(hDesk);
        }

        IntPtr hdcSrc = GetDC(IntPtr.Zero);
        if (hdcSrc == IntPtr.Zero) {
            return false;
        }

        int width = 1920;
        int height = 1080;
        Bitmap bmp = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(bmp)) {
            IntPtr hdcDest = g.GetHdc();
            BitBlt(hdcDest, 0, 0, width, height, hdcSrc, 0, 0, SRCCOPY);
            g.ReleaseHdc(hdcDest);
        }
        ReleaseDC(IntPtr.Zero, hdcSrc);
        bmp.Save(filename, ImageFormat.Png);
        bmp.Dispose();
        return true;
    }
}
'@ -ReferencedAssemblies System.Drawing

$ok = [ScreenCap]::TryCapture('C:\repos\rockcast\screen_winsta.png')
Write-Output ("Capture result: " + $ok)
