# List every top-level window of a process: handle, title, rect, state.
param([Parameter(Mandatory = $true)][int]$ProcessId)

Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class Win32Enum {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

    public static List<string> Collect(uint target) {
        var rows = new List<string>();
        EnumWindows((hwnd, lp) => {
            uint pid; GetWindowThreadProcessId(hwnd, out pid);
            if (pid == target) {
                var sb = new StringBuilder(512); GetWindowText(hwnd, sb, 512);
                RECT r; GetWindowRect(hwnd, out r);
                rows.Add(string.Format("hwnd=0x{0:X} visible={1} iconic={2} rect={3},{4},{5},{6} title={7}",
                    hwnd.ToInt64(), IsWindowVisible(hwnd), IsIconic(hwnd), r.Left, r.Top, r.Right, r.Bottom, sb));
            }
            return true;
        }, IntPtr.Zero);
        return rows;
    }
}
"@

[Win32Enum]::Collect([uint32]$ProcessId) | ForEach-Object { Write-Output $_ }
