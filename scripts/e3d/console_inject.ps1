# Feed commands to a running des.exe session.
#
# des.exe reads its stdin from a pipe fed by pdmsconsole.exe, not from the console
# window, so synthesised keystrokes to that window go nowhere. This attaches to the
# session's console and writes key records straight into its input buffer, which is
# what pdmsconsole's ReadConsole picks up and relays down the pipe.
#
# Must run in its own process: FreeConsole detaches the caller's console for good.

param(
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [Parameter(Mandatory = $true)][string]$TextFile,
    [string]$LogFile = 'D:\work\plant-code\old\gen-model\output\console_inject.log'
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class ConIn
{
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr h);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa,
                                            uint disposition, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool WriteConsoleInputW(IntPtr h, INPUT_RECORD[] buffer, uint count, out uint written);

    [StructLayout(LayoutKind.Sequential)]
    public struct KEY_EVENT_RECORD
    {
        [MarshalAs(UnmanagedType.Bool)] public bool bKeyDown;
        public ushort wRepeatCount;
        public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode;
        public char UnicodeChar;
        public uint dwControlKeyState;
    }

    [StructLayout(LayoutKind.Explicit)]
    public struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }

    public static INPUT_RECORD Key(char c, ushort vk, bool down)
    {
        INPUT_RECORD r = new INPUT_RECORD();
        r.EventType = 1; // KEY_EVENT
        r.KeyEvent.bKeyDown = down;
        r.KeyEvent.wRepeatCount = 1;
        r.KeyEvent.wVirtualKeyCode = vk;
        r.KeyEvent.wVirtualScanCode = 0;
        r.KeyEvent.UnicodeChar = c;
        r.KeyEvent.dwControlKeyState = 0;
        return r;
    }
}
'@

$lines = Get-Content -Path $TextFile -Encoding UTF8
$report = New-Object Collections.Generic.List[string]
$report.Add("target pid=$ProcessId  lines=$($lines.Count)")

[void][ConIn]::FreeConsole()
$attached = [ConIn]::AttachConsole([uint32]$ProcessId)
$report.Add("AttachConsole=$attached err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())")

if ($attached) {
    $GENERIC_RW = [uint32]3221225472  # GENERIC_READ | GENERIC_WRITE
    $SHARE_RW = [uint32]3
    $OPEN_EXISTING = [uint32]3
    $h = [ConIn]::CreateFileW('CONIN$', $GENERIC_RW, $SHARE_RW, [IntPtr]::Zero, $OPEN_EXISTING, 0, [IntPtr]::Zero)
    $report.Add("CONIN handle=$h err=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())")

    if ($h -ne [IntPtr]::Zero -and $h -ne [IntPtr](-1)) {
        foreach ($line in $lines) {
            $recs = New-Object Collections.Generic.List[object]
            foreach ($ch in $line.ToCharArray()) {
                $recs.Add([ConIn]::Key($ch, 0, $true))
                $recs.Add([ConIn]::Key($ch, 0, $false))
            }
            $recs.Add([ConIn]::Key([char]13, 13, $true))
            $recs.Add([ConIn]::Key([char]13, 13, $false))
            $arr = [ConIn+INPUT_RECORD[]]$recs.ToArray()
            $written = [uint32]0
            $ok = [ConIn]::WriteConsoleInputW($h, $arr, [uint32]$arr.Length, [ref]$written)
            $report.Add("sent ok=$ok written=$written/$($arr.Length) :: $line")
            Start-Sleep -Milliseconds 500
        }
        [void][ConIn]::CloseHandle($h)
    }
    [void][ConIn]::FreeConsole()
}

Set-Content -Path $LogFile -Value $report -Encoding UTF8
