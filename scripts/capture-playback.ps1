<#
Collects OS metrics and RockCast diagnostics during a manual GUI reproduction.

Usage:
  .\scripts\capture-playback.ps1 -Seconds 1200
  .\scripts\capture-playback.ps1 -ProcessId 1234 -Seconds 1200

If -ProcessId is omitted, it launches `cargo run --release` with
ROCKCAST_PROFILE=1.  Stop the GUI when the manual switching scenario ends.
#>
param(
    [int]$ProcessId = 0,
    [int]$Seconds = 1200,
    [string]$OutputDir = "target\rockcast-soak\manual-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
)

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$csvPath = Join-Path $OutputDir 'process.csv'
$logPath = Join-Path $env:LOCALAPPDATA 'RockCast\rockcast.log'
"timestamp,cpu_seconds,threads,handles,working_set_mb,private_mb" | Set-Content -Encoding utf8 $csvPath

if ($ProcessId -eq 0) {
    $env:ROCKCAST_PROFILE = '1'
    $proc = Start-Process -FilePath 'cargo' -ArgumentList 'run --release' -PassThru
    $ProcessId = $proc.Id
    Write-Host "Started cargo process $ProcessId. Perform the switching scenario, then close RockCast."
} else {
    Write-Host "Monitoring process $ProcessId. Ensure it was started with ROCKCAST_PROFILE=1."
}

for ($i = 0; $i -lt $Seconds; $i++) {
    try {
        $p = Get-Process -Id $ProcessId -ErrorAction Stop
    } catch {
        break
    }
    $line = '{0:o},{1:F3},{2},{3},{4:F2},{5:F2}' -f (Get-Date), $p.CPU, $p.Threads.Count, $p.HandleCount, ($p.WorkingSet64 / 1MB), ($p.PrivateMemorySize64 / 1MB)
    Add-Content -Encoding utf8 -Path $csvPath -Value $line
    Start-Sleep -Seconds 1
}

if (Test-Path -LiteralPath $logPath) {
    Copy-Item -LiteralPath $logPath -Destination (Join-Path $OutputDir 'rockcast.log') -Force
}
Write-Host "Captured metrics in $OutputDir"
