# Dump the visible text of another process's console screen buffer.
#
# Companion to console_inject.ps1: the des.exe console window cannot be reliably
# brought to the foreground (another app holds the foreground lock), so the reply
# to injected commands is read straight out of the screen buffer instead.
#
# Must run in its own process: FreeConsole detaches the caller's console for good.

param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [string]$OutFile = 'D:\work\plant-code\old\gen-model\output\console_dump.txt'
)

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class ConOut
{
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr h);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa,
                                            uint disposition, uint flags, IntPtr template);

    [StructLayout(LayoutKind.Sequential)] public struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct SMALL_RECT { public short Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    public struct CONSOLE_SCREEN_BUFFER_INFO
    {
        public COORD dwSize;
        public COORD dwCursorPosition;
        public ushort wAttributes;
        public SMALL_RECT srWindow;
        public COORD dwMaximumWindowSize;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool ReadConsoleOutputCharacterW(IntPtr h, StringBuilder buf, uint len,
                                                          COORD pos, out uint read);
}
'@

$report = New-Object Collections.Generic.List[string]
# Nothing may be written to the host after attaching: it would land in the target's
# own screen buffer and corrupt the very text being read.
$ErrorActionPreference = 'SilentlyContinue'
[void][ConOut]::FreeConsole()
$attached = [ConOut]::AttachConsole([uint32]$ProcessId)
$report.Add("AttachConsole=$attached")

if ($attached) {
    $GENERIC_RW = [uint32]3221225472
    $SHARE_RW = [uint32]3
    $OPEN_EXISTING = [uint32]3
    $h = [ConOut]::CreateFileW('CONOUT$', $GENERIC_RW, $SHARE_RW, [IntPtr]::Zero, $OPEN_EXISTING, 0, [IntPtr]::Zero)
    $info = New-Object ConOut+CONSOLE_SCREEN_BUFFER_INFO
    if ([ConOut]::GetConsoleScreenBufferInfo($h, [ref]$info)) {
        $w = $info.dwSize.X
        $rows = $info.dwCursorPosition.Y + 2
        $report.Add("buffer ${w}x$($info.dwSize.Y)  cursor=($($info.dwCursorPosition.X),$($info.dwCursorPosition.Y))")
        for ($y = 0; $y -lt $rows; $y++) {
            $sb = New-Object Text.StringBuilder ($w + 1)
            $pos = New-Object ConOut+COORD
            $pos.X = [int16]0
            $pos.Y = [int16]$y
            $read = [uint32]0
            if ([ConOut]::ReadConsoleOutputCharacterW($h, $sb, [uint32]$w, $pos, [ref]$read)) {
                $report.Add($sb.ToString().TrimEnd())
            }
        }
    } else {
        $report.Add("GetConsoleScreenBufferInfo failed err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())")
    }
    [void][ConOut]::CloseHandle($h)
    [void][ConOut]::FreeConsole()
}

Set-Content -Path $OutFile -Value $report -Encoding UTF8
