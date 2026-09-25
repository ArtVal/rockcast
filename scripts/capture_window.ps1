param(
    [int]$TargetPid = 0
)

Add-Type @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class WindowCapture {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBkgnd, uint nFlags);

    [DllImport("user32.dll")]
    public static extern IntPtr GetWindowDC(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern int ReleaseDC(IntPtr hWnd, IntPtr hDC);

    [DllImport("gdi32.dll")]
    public static extern bool BitBlt(IntPtr hObject, int nXDest, int nYDest, int nWidth, int nHeight, IntPtr hObjectSource, int nXSrc, int nYSrc, uint dwRop);

    public const uint SRCCOPY = 0x00CC0020;
    public const uint PW_RENDERFULLCONTENT = 0x00000002;

    public static Bitmap CaptureWindow(IntPtr hWnd) {
        RECT rect;
        GetWindowRect(hWnd, out rect);
        int width = rect.Right - rect.Left;
        int height = rect.Bottom - rect.Top;
        if (width <= 0 || height <= 0) return null;

        Bitmap bmp = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(bmp)) {
            IntPtr hdc = g.GetHdc();
            bool success = PrintWindow(hWnd, hdc, PW_RENDERFULLCONTENT);
            if (!success) {
                // fallback to 0
                PrintWindow(hWnd, hdc, 0);
            }
            g.ReleaseHdc(hdc);
        }
        return bmp;
    }

    public static Bitmap CaptureWindowDC(IntPtr hWnd) {
        RECT rect;
        GetWindowRect(hWnd, out rect);
        int width = rect.Right - rect.Left;
        int height = rect.Bottom - rect.Top;
        if (width <= 0 || height <= 0) return null;

        Bitmap bmp = new Bitmap(width, height, PixelFormat.Format32bppArgb);
        using (Graphics g = Graphics.FromImage(bmp)) {
            IntPtr hdcDest = g.GetHdc();
            IntPtr hdcSrc = GetWindowDC(hWnd);
            BitBlt(hdcDest, 0, 0, width, height, hdcSrc, 0, 0, SRCCOPY);
            ReleaseDC(hWnd, hdcSrc);
            g.ReleaseHdc(hdcDest);
        }
        return bmp;
    }
}
"@ -ReferencedAssemblies System.Drawing

$p = if ($TargetPid -ne 0) {
    Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
} else {
    Get-Process rockcast -ErrorAction SilentlyContinue
}
if (-not $p -or $p.MainWindowHandle -eq 0) {
    Write-Error "rockcast not running or no window handle"
    exit 1
}

$bmp = [WindowCapture]::CaptureWindow($p.MainWindowHandle)
if ($bmp -ne $null) {
    $bmp.Save("C:\repos\rockcast\screen_verify_printwindow.png", [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "Saved screen_verify_printwindow.png"
}

$bmp2 = [WindowCapture]::CaptureWindowDC($p.MainWindowHandle)
if ($bmp2 -ne $null) {
    $bmp2.Save("C:\repos\rockcast\screen_verify_dc.png", [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp2.Dispose()
    Write-Output "Saved screen_verify_dc.png"
}
