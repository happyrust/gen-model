param(
    [Parameter(Mandatory)][string]$OutputPath,
    [string]$Workspace = 'D:\work\plant-code\old\test-worklspace',
    [int]$DurationSeconds = 7200,
    [string]$StopFile = ''
)
$ErrorActionPreference = 'Stop'
$deadline = [DateTime]::UtcNow.AddSeconds($DurationSeconds)
$writer = [IO.StreamWriter]::new([IO.Path]::GetFullPath($OutputPath), $false)
try {
    $writer.WriteLine('timestamp,pid,start_time,process,private_bytes,working_set_bytes')
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($StopFile -and (Test-Path -LiteralPath $StopFile)) { break }
        $timestamp = [DateTime]::UtcNow.ToString('o')
        foreach ($process in Get-Process surreal,aios-database -ErrorAction SilentlyContinue) {
            try {
                if ($process.Path -and $process.Path.StartsWith($Workspace + '\', [StringComparison]::OrdinalIgnoreCase)) {
                    $writer.WriteLine(('{0},{1},{2},{3},{4},{5}' -f $timestamp,$process.Id,
                        $process.StartTime.ToUniversalTime().ToString('o'),$process.ProcessName,
                        $process.PrivateMemorySize64,$process.WorkingSet64))
                }
            } catch { if (-not $process.HasExited) { throw } }
        }
        $writer.Flush()
        Start-Sleep -Milliseconds 1000
    }
} finally { $writer.Dispose() }
