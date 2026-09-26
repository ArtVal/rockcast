Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class WinFind {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll")]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    public static List<IntPtr> FindWindowsForPid(uint targetPid) {
        var results = new List<IntPtr>();
        EnumWindows((hWnd, lParam) => {
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);
            if (pid == targetPid) {
                results.Add(hWnd);
            }
            return true;
        }, IntPtr.Zero);
        return results;
    }

    public static string GetTitle(IntPtr hWnd) {
        var sb = new StringBuilder(256);
        GetWindowText(hWnd, sb, 256);
        return sb.ToString();
    }
}
'@

$p = Get-Process rockcast -ErrorAction SilentlyContinue
if (-not $p) {
    Write-Output "No rockcast process"
    exit
}
$windows = [WinFind]::FindWindowsForPid($p.Id)
Write-Output ("Found " + $windows.Count + " windows for PID " + $p.Id)
foreach ($w in $windows) {
    $t = [WinFind]::GetTitle($w)
    Write-Output ("HWND: " + $w + " Title: '" + $t + "'")
}
