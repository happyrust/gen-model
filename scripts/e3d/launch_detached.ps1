# des.exe is a GUI-subsystem binary, so launching it from cmd makes it inherit
# that console. core.dll then sees GetConsoleWindow() != 0 and skips spawning
# pdmsconsole.exe, leaving the session with no command channel at all. Creating
# it with DETACHED_PROCESS reproduces the condition of a working session, where
# core.dll allocates its own console host.
#
# The environment is inherited from the caller, so this must be invoked from the
# batch file that has already sourced evars.bat.

param(
    [string]$Exe = 'D:\AVEVA\Everything3D3.1\des.exe',
    [string]$Arguments = 'ams SYSTEM/XXXXXX /ALL',
    [string]$WorkingDirectory = 'D:\AVEVA\Everything3D3.1'
)

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class DetachedLauncher
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct STARTUPINFO
    {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2;
        public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct PROCESS_INFORMATION
    {
        public IntPtr hProcess, hThread;
        public int dwProcessId, dwThreadId;
    }

    public const uint DETACHED_PROCESS = 0x00000008;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CreateProcess(
        string lpApplicationName, string lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
}
'@

$si = New-Object DetachedLauncher+STARTUPINFO
$si.cb = [Runtime.InteropServices.Marshal]::SizeOf($si)
$pi = New-Object DetachedLauncher+PROCESS_INFORMATION

$cmdLine = '"' + $Exe + '" ' + $Arguments
$ok = [DetachedLauncher]::CreateProcess(
    $Exe, $cmdLine, [IntPtr]::Zero, [IntPtr]::Zero, $false,
    [DetachedLauncher]::DETACHED_PROCESS, [IntPtr]::Zero, $WorkingDirectory,
    [ref]$si, [ref]$pi)

if ($ok) {
    "detached pid=$($pi.dwProcessId)"
} else {
    "CreateProcess failed, win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}
