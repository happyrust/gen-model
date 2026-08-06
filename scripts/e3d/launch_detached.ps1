# des.exe is a GUI-subsystem binary, so launching it from cmd makes it inherit
# that console. core.dll then sees GetConsoleWindow() != 0 and skips spawning
# pdmsconsole.exe, leaving the session with no command channel at all. Creating
# it with DETACHED_PROCESS reproduces the condition of a working session, where
# core.dll allocates its own console host.
#
# The environment is inherited from the caller, so this must be invoked from the
# batch file that has already sourced evars.bat.

param(
    [string]$Exe = 'C:\Program Files (x86)\AVEVA\Everything3D3.1\des.exe',
    [string]$Arguments = 'ams SYSTEM/XXXXXX /ALL',
    [string]$WorkingDirectory = 'C:\Program Files (x86)\AVEVA\Everything3D3.1',
    [switch]$Wait,
    [ValidateRange(0, 86400)]
    [int]$TimeoutSeconds = 0,
    [string]$PidFile
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
    throw "Executable does not exist: $Exe"
}
if (-not (Test-Path -LiteralPath $WorkingDirectory -PathType Container)) {
    throw "Working directory does not exist: $WorkingDirectory"
}

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
    public const int STARTF_USESHOWWINDOW = 0x00000001;
    public const short SW_HIDE = 0;
    public const uint WAIT_OBJECT_0 = 0x00000000;
    public const uint WAIT_TIMEOUT = 0x00000102;
    public const uint INFINITE = 0xFFFFFFFF;

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CreateProcess(
        string lpApplicationName, string lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);

    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr hObject);
}
'@

$si = New-Object DetachedLauncher+STARTUPINFO
$si.cb = [Runtime.InteropServices.Marshal]::SizeOf($si)
$si.dwFlags = [DetachedLauncher]::STARTF_USESHOWWINDOW
$si.wShowWindow = [DetachedLauncher]::SW_HIDE
$pi = New-Object DetachedLauncher+PROCESS_INFORMATION

$cmdLine = '"' + $Exe + '" ' + $Arguments
$ok = [DetachedLauncher]::CreateProcess(
    $Exe, $cmdLine, [IntPtr]::Zero, [IntPtr]::Zero, $false,
    [DetachedLauncher]::DETACHED_PROCESS, [IntPtr]::Zero, $WorkingDirectory,
    [ref]$si, [ref]$pi)

if (-not $ok) {
    throw "CreateProcess failed, win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
}

$cleanupVerified = $false
try {
    if ($PidFile) {
        $pidPath = [IO.Path]::GetFullPath($PidFile)
        $pidDir = [IO.Path]::GetDirectoryName($pidPath)
        if ($pidDir) { [IO.Directory]::CreateDirectory($pidDir) | Out-Null }
        [IO.File]::WriteAllText($pidPath, [string]$pi.dwProcessId)
    }

    "detached pid=$($pi.dwProcessId)"
    if (-not $Wait) {
        $cleanupVerified = $true
        return
    }

    $waitMs = if ($TimeoutSeconds -eq 0) {
        [DetachedLauncher]::INFINITE
    } else {
        [uint32]($TimeoutSeconds * 1000)
    }
    $waitResult = [DetachedLauncher]::WaitForSingleObject($pi.hProcess, $waitMs)
    if ($waitResult -eq [DetachedLauncher]::WAIT_TIMEOUT) {
        & taskkill.exe /PID $pi.dwProcessId /T /F 2>&1 | Out-String | Write-Verbose
        $killExit = $LASTEXITCODE
        $reaped = [DetachedLauncher]::WaitForSingleObject($pi.hProcess, 10000)
        if ($reaped -ne [DetachedLauncher]::WAIT_OBJECT_0) {
            throw "Process $($pi.dwProcessId) cleanup failed (taskkill=$killExit wait=$reaped)"
        }
        $cleanupVerified = $true
        [Console]::Error.WriteLine("Process $($pi.dwProcessId) timed out after $TimeoutSeconds seconds")
        exit 124
    }
    if ($waitResult -ne [DetachedLauncher]::WAIT_OBJECT_0) {
        throw "WaitForSingleObject failed, result=$waitResult win32=$([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    $cleanupVerified = $true

    [uint32]$exitCode = 0
    if (-not [DetachedLauncher]::GetExitCodeProcess($pi.hProcess, [ref]$exitCode)) {
        throw "GetExitCodeProcess failed, win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    "detached pid=$($pi.dwProcessId) exit=$exitCode"
    if ($exitCode -eq 0) { exit 0 }
    exit 1
}
catch {
    if (-not $cleanupVerified) {
        & taskkill.exe /PID $pi.dwProcessId /T /F 2>&1 | Out-String | Write-Verbose
        $reaped = [DetachedLauncher]::WaitForSingleObject($pi.hProcess, 10000)
        $cleanupVerified = $reaped -eq [DetachedLauncher]::WAIT_OBJECT_0
    }
    throw
}
finally {
    if ($Wait) {
        Get-CimInstance Win32_Process -Filter "ParentProcessId = $($pi.dwProcessId)" -ErrorAction SilentlyContinue |
            ForEach-Object { & taskkill.exe /PID $_.ProcessId /T /F 2>&1 | Out-Null }
    }
    if ($PidFile -and $cleanupVerified) { Remove-Item -LiteralPath $PidFile -Force -ErrorAction SilentlyContinue }
    if ($pi.hThread -ne [IntPtr]::Zero) { [DetachedLauncher]::CloseHandle($pi.hThread) | Out-Null }
    if ($pi.hProcess -ne [IntPtr]::Zero) { [DetachedLauncher]::CloseHandle($pi.hProcess) | Out-Null }
}
