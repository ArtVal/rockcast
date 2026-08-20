<#
Summarises one `examples/playback_soak` run and highlights CPU / worker drift.

Usage:
  .\scripts\analyze-soak.ps1 target\rockcast-soak\<run-id>
#>
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$RunDir
)

$ErrorActionPreference = 'Stop'
$csvPath = Join-Path $RunDir 'cycles.csv'
if (-not (Test-Path -LiteralPath $csvPath)) {
    throw "Missing soak artefact: $csvPath"
}

$rows = Import-Csv -LiteralPath $csvPath
if ($rows.Count -eq 0) {
    throw 'No soak cycles were recorded.'
}

$summary = foreach ($group in ($rows | Group-Object mode)) {
    $ok = @($group.Group | Where-Object result -eq 'ok')
    $first = @($group.Group | Select-Object -First 5)
    $last = @($group.Group | Select-Object -Last 5)
    $firstCpu = ($first | Measure-Object -Property cpu_pct -Average).Average
    $lastCpu = ($last | Measure-Object -Property cpu_pct -Average).Average
    [pscustomobject]@{
        Mode = $group.Name
        Cycles = $group.Count
        Passed = $ok.Count
        Failed = $group.Count - $ok.Count
        First5CpuPct = [math]::Round($firstCpu, 2)
        Last5CpuPct = [math]::Round($lastCpu, 2)
        CpuDeltaPct = [math]::Round($lastCpu - $firstCpu, 2)
        LastWorkers = ($last | Select-Object -Last 1).workers
    }
}

$summary | Format-Table -AutoSize
foreach ($item in $summary) {
    if ($item.Failed -gt 0) {
        Write-Warning "$($item.Mode): failed cycles exist; inspect events.jsonl and rockcast.log."
    }
    if ($item.CpuDeltaPct -gt 5) {
        Write-Warning "$($item.Mode): CPU grew by $($item.CpuDeltaPct)% between first and last five cycles."
    }
    if ($item.LastWorkers -match '=(?:[1-9][0-9]*)') {
        Write-Warning "$($item.Mode): active worker gauges after the final stop: $($item.LastWorkers)"
    }
}
