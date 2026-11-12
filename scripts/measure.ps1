# Reads the installer size, the idle memory and CPU of a running ResSwap and its web view
# processes, and the PERF timing lines in ResSwap's log, then writes one JSON result file under
# docs\results\. It never starts, stops or changes a process, a registry value or a display
# setting; the caller starts and stops ResSwap.
#
# Example, from the repository root, with ResSwap already running:
#   pwsh -File scripts\measure.ps1 -Phase P0 -What Idle -Window open -AutoSwitch none -OutFile docs\results\2026-11-12\measure-P0-idle-1.json
#
# This file holds ASCII characters only, so its encoding never matters.
#Requires -Version 7.0
[CmdletBinding()]
param(
  [Parameter(Mandatory)][ValidatePattern('^P[0-8]$')][string]$Phase,
  [ValidateSet('Installer', 'Idle', 'Log', 'All')][string]$What = 'All',
  [ValidateRange(10, 300)][int]$Seconds = 60,
  [ValidateRange(0, 120)][int]$SettleSeconds = 30,
  [ValidateSet('open', 'closed')][string]$Window = 'closed',
  [ValidateSet('none', 'paused', 'watching')][string]$AutoSwitch = 'paused',
  [ValidateSet('release', 'debug')][string]$Build = 'release',
  [ValidatePattern('^[A-Za-z0-9.-]+$')][string]$Identifier = 'io.github.T7SEN.ResSwap',
  [ValidatePattern('^[A-Za-z0-9_.-]+$')][string]$ProcessName = 'ResSwap',
  [string]$LogFolder = '',
  [ValidateSet('Utc', 'Local')][string]$LogTime = 'Utc',
  [datetime]$Since = [datetime]::MinValue,
  [Parameter(Mandatory)][string]$OutFile
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$invariant = [Globalization.CultureInfo]::InvariantCulture

# Where the result goes.
$repo = Split-Path -Parent $PSScriptRoot
$resultsRoot = [IO.Path]::GetFullPath((Join-Path $repo 'docs\results')).TrimEnd('\') + '\'
$target = [IO.Path]::GetFullPath($OutFile, $repo)
$dayFolder = Split-Path -Parent $target
$fileName = Split-Path -Leaf $target
if (-not $target.StartsWith($resultsRoot, [StringComparison]::OrdinalIgnoreCase)) {
  throw 'OutFile must be under docs\results\.'
}
if (($dayFolder.TrimEnd('\') + '\') -ne ($resultsRoot + (Split-Path -Leaf $dayFolder) + '\') -or
    (Split-Path -Leaf $dayFolder) -notmatch '^\d{4}-\d{2}-\d{2}$') {
  throw 'OutFile must sit in a day folder: docs\results\<YYYY-MM-DD>\.'
}
if ($fileName -notmatch "^measure-$Phase-[a-z0-9]+-\d+\.json$") {
  throw "OutFile must be named measure-$Phase-<what>-<n>.json."
}
# A recorded result is never overwritten; a new reading takes the next number.
if (Test-Path -LiteralPath $target) {
  throw "$fileName already exists. Use the next number."
}
$shownTarget = $target.Substring($repo.TrimEnd('\').Length + 1)

# Common fields.
$commit = $null
try {
  $head = & git -C $repo rev-parse HEAD 2>$null
  if ($LASTEXITCODE -eq 0) { $commit = "$head".Trim() }
} catch {
  $commit = $null
}

$data = [ordered]@{
  schemaVersion = 1
  phase = $Phase
  build = $Build
  measuredAt = (Get-Date).ToString("yyyy-MM-dd'T'HH:mm:ss.fffzzz", $invariant)
  commit = $commit
  osBuild = [Environment]::OSVersion.Version.Build
  source = 'measure.ps1'
}

# Installer.
if ($What -in 'Installer', 'All') {
  if ($Build -ne 'release') { throw 'The installer reading needs -Build release.' }
  $nsis = Join-Path $repo 'src-tauri\target\release\bundle\nsis'
  $files = @(Get-ChildItem -LiteralPath $nsis -Filter '*-setup.exe' -File -ErrorAction SilentlyContinue)
  if ($files.Count -ne 1) {
    throw "Expected one installer in src-tauri\target\release\bundle\nsis, found $($files.Count)."
  }
  # The file name only: a full path would hold the Windows user name.
  $data.installer = [ordered]@{ file = $files[0].Name; bytes = $files[0].Length }
}

# Idle memory and CPU.
function Get-Alive([object[]]$Known) {
  # A process counts only while its ID still belongs to the same start time, since Windows
  # reuses process IDs.
  @($Known | ForEach-Object {
      $p = Get-Process -Id $_.Id -ErrorAction SilentlyContinue
      if ($null -ne $p -and $p.StartTime -eq $_.StartTime) { $p }
    })
}

if ($What -in 'Idle', 'All') {
  $roots = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
  if ($roots.Count -ne 1) {
    throw "Expected one $ProcessName process, found $($roots.Count). Start exactly one before measuring."
  }
  $rootId = $roots[0].Id

  Write-Output "Waiting $SettleSeconds s before the reading. Leave the PC alone until it ends."
  Start-Sleep -Seconds $SettleSeconds

  # The web view processes are found after the wait, so ones that start during it are counted.
  # Only msedgewebview2.exe processes created after their parent belong to the tree, because a
  # parent ID can point at an older process that reused the number.
  $all = @(Get-CimInstance -ClassName Win32_Process | Select-Object ProcessId, ParentProcessId, Name, CreationDate)
  $ids = [Collections.Generic.List[int]]::new()
  $ids.Add($rootId)
  for ($i = 0; $i -lt $ids.Count; $i++) {
    $parent = $all | Where-Object { $_.ProcessId -eq $ids[$i] } | Select-Object -First 1
    if ($null -eq $parent) { continue }
    foreach ($child in $all) {
      if ($child.ParentProcessId -eq $ids[$i] -and
          $child.Name -eq 'msedgewebview2.exe' -and
          $child.CreationDate -ge $parent.CreationDate -and
          -not $ids.Contains([int]$child.ProcessId)) {
        $ids.Add([int]$child.ProcessId)
      }
    }
  }

  $known = @($ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue } | Where-Object { $null -ne $_ })
  # The @() keeps a one-process list a list.
  $start = @(Get-Alive $known)
  $cpuStart = @{}
  foreach ($p in $start) { $cpuStart[$p.Id] = $p.TotalProcessorTime.TotalMilliseconds }
  $clock = [Diagnostics.Stopwatch]::StartNew()
  Start-Sleep -Seconds $Seconds
  $end = @(Get-Alive $start)
  $wallMs = $clock.Elapsed.TotalMilliseconds

  # CPU time counts only for processes alive at both ends; the counts at both ends are recorded
  # so a process that exited during the sample shows up.
  $cpuMs = 0.0
  foreach ($p in $end) { $cpuMs += $p.TotalProcessorTime.TotalMilliseconds - $cpuStart[$p.Id] }
  $workingSet = [long]0
  $privateBytes = [long]0
  foreach ($p in $end) {
    $workingSet += $p.WorkingSet64
    $privateBytes += $p.PrivateMemorySize64
  }

  $data.idle = [ordered]@{
    seconds = $Seconds
    window = $Window
    autoSwitch = $AutoSwitch
    processesStart = $start.Count
    processesEnd = $end.Count
    webviewProcesses = @($end | Where-Object { $_.ProcessName -eq 'msedgewebview2' }).Count
    workingSetBytes = $workingSet
    privateBytes = $privateBytes
    cpuPercent = [math]::Round(100 * $cpuMs / ($wallMs * [Environment]::ProcessorCount), 3)
  }
}

# Timing lines from the log.
function Get-Summary([double[]]$Runs) {
  $sorted = @($Runs | Sort-Object)
  $middle = [int][math]::Floor($sorted.Count / 2)
  $median = if ($sorted.Count % 2 -eq 1) { $sorted[$middle] } else { ($sorted[$middle - 1] + $sorted[$middle]) / 2 }
  [ordered]@{ runs = @($Runs); median = $median; max = $sorted[-1] }
}

if ($What -in 'Log', 'All') {
  $folder = if ($LogFolder -ne '') { $LogFolder } else { Join-Path $env:LOCALAPPDATA "$Identifier\logs" }
  $logFiles = @(Get-ChildItem -LiteralPath $folder -Filter '*.log' -File -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime, Name)

  # tauri-plugin-log 2.10.2 writes "[date][time][target][LEVEL] message" by default, and
  # "[date][time][LEVEL][target] message" once a time zone strategy is set; both orders match.
  $pattern = '^\[(?<day>\d{4}-\d{2}-\d{2})\]\[(?<time>\d{2}:\d{2}:\d{2})\](?:\[[^\]]*\]){2} PERF (?<event>[A-Za-z0-9_.]+)(?<fields>(?: [A-Za-z0-9_]+=\S*)*)\s*$'
  $sinceUtc = if ($Since -eq [datetime]::MinValue) { [datetime]::MinValue } else { $Since.ToUniversalTime() }

  $received = @{}
  $lastMatched = @{}
  $modeSwitch = [Collections.Generic.List[double]]::new()
  $autoDetect = [Collections.Generic.List[double]]::new()
  $snapshotCount = 0
  $snapshotWeighted = 0.0
  $snapshotMax = 0.0
  $snapshotProcesses = $null
  $perfLines = 0

  foreach ($file in $logFiles) {
    foreach ($line in [IO.File]::ReadLines($file.FullName)) {
      $m = [regex]::Match($line, $pattern)
      if (-not $m.Success) { continue }
      $stamp = [datetime]::ParseExact("$($m.Groups['day'].Value) $($m.Groups['time'].Value)", 'yyyy-MM-dd HH:mm:ss', $invariant)
      $stampUtc = if ($LogTime -eq 'Utc') { [datetime]::SpecifyKind($stamp, [DateTimeKind]::Utc) } else { [datetime]::SpecifyKind($stamp, [DateTimeKind]::Local).ToUniversalTime() }
      if ($stampUtc -lt $sinceUtc) { continue }
      $perfLines++

      $f = @{}
      foreach ($pair in $m.Groups['fields'].Value.Trim().Split(' ', [StringSplitOptions]::RemoveEmptyEntries)) {
        $key, $value = $pair.Split('=', 2)
        $f[$key] = $value
      }
      $mono = $null
      if ($f.ContainsKey('mono_ms')) { $mono = [double]::Parse($f['mono_ms'], $invariant) }

      switch ($m.Groups['event'].Value) {
        'app.started' {
          # A new process run starts its monotonic clock again, so older matches cannot pair.
          $lastMatched.Clear()
        }
        'watch.matched' {
          if ($f.ContainsKey('profile') -and $null -ne $mono) { $lastMatched[$f['profile']] = $mono }
        }
        'apply.received' {
          if ($f.ContainsKey('change') -and $null -ne $mono) {
            $received[$f['change']] = @{
              mono = $mono
              source = $(if ($f.ContainsKey('source')) { $f['source'] } else { '' })
              profile = $(if ($f.ContainsKey('profile')) { $f['profile'] } else { '' })
            }
          }
        }
        'apply.verified' {
          if ($f.ContainsKey('change') -and $null -ne $mono -and $received.ContainsKey($f['change'])) {
            $entry = $received[$f['change']]
            if ($mono -ge $entry.mono) {
              if ($entry.source -ne 'watcher') {
                $modeSwitch.Add($mono - $entry.mono)
              } elseif ($entry.profile -ne '' -and $lastMatched.ContainsKey($entry.profile) -and $mono -ge $lastMatched[$entry.profile]) {
                $autoDetect.Add($mono - $lastMatched[$entry.profile])
              }
            }
          }
        }
        'watch.snapshots' {
          if ($f.ContainsKey('count') -and $f.ContainsKey('mean_us') -and $f.ContainsKey('max_us')) {
            $count = [int]::Parse($f['count'], $invariant)
            $snapshotCount += $count
            $snapshotWeighted += $count * [double]::Parse($f['mean_us'], $invariant)
            $snapshotMax = [math]::Max($snapshotMax, [double]::Parse($f['max_us'], $invariant))
            if ($f.ContainsKey('processes')) { $snapshotProcesses = [int]::Parse($f['processes'], $invariant) }
          }
        }
      }
    }
  }

  if ($perfLines -eq 0) {
    $message = "No PERF line at or after -Since in $($logFiles.Count) log file(s)."
    if ($What -eq 'Log') { throw $message }
    Write-Output $message
  }

  $timings = [ordered]@{}
  if ($modeSwitch.Count -gt 0) { $timings.modeSwitchMs = Get-Summary $modeSwitch.ToArray() }
  if ($autoDetect.Count -gt 0) { $timings.autoSwitchDetectMs = Get-Summary $autoDetect.ToArray() }
  if ($timings.Count -gt 0) { $data.timings = $timings }
  if ($snapshotCount -gt 0) {
    $data.watcher = [ordered]@{
      snapshots = $snapshotCount
      meanUs = [math]::Round($snapshotWeighted / $snapshotCount, 1)
      maxUs = $snapshotMax
      processes = $snapshotProcesses
    }
  }
}

# Write the result.
New-Item -ItemType Directory -Force -Path $dayFolder | Out-Null
# LF line endings and no byte order mark, the same as every other text file in the repository.
$json = ($data | ConvertTo-Json -Depth 6) -replace "`r`n", "`n"
[IO.File]::WriteAllText($target, $json + "`n", [Text.UTF8Encoding]::new($false))
Write-Output "Wrote $shownTarget"
