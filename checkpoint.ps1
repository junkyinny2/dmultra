#Requires -Version 7.0
<#
.SYNOPSIS
  DM Ultra project-memory checkpoint tool.

.DESCRIPTION
  Keeps the project memory files honest so a new conversation can resume without
  re-explaining anything.

  status  (default)  print the resume briefing: versions, git, last checkpoint,
                     open task count, the commands that prove the tree is green.
  verify             fail (exit code = number of problems) if a memory file is
                     missing, thin, missing a required section, or older than
                     -StaleHours; warns when source files changed after the last
                     PROJECT_STATUS.md write.
  save                 stamp a checkpoint: rewrites the "Last checkpoint:" line near
                     the top of PROJECT_STATUS.md and pushes a new entry into the
                     ledger between the CHECKPOINT:BEGIN / CHECKPOINT:END markers
                     (newest first, 20 entries kept).

.EXAMPLE
  checkpoint.ps1 verify
  checkpoint.ps1 save -Note "fixed the extension version drift"
  checkpoint.ps1

.NOTES
  Run it from C:\dmultra-base (it uses its own folder as the project root).
  Never edits source code - only PROJECT_STATUS.md, and only its two checkpoint spots.
#>
param(
    [Parameter(Position = 0)]
    [ValidateSet('status', 'verify', 'save')]
    [string]$Mode = 'status',

    [string]$Note = '',

    [int]$StaleHours = 48
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Root = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }

# Each memory file and the sections that make it a valid, complete document.
$Memory = [ordered]@{
    'PROJECT_STATUS.md' = @(
        '## 1. What we are building',
        '## 3. Current state',
        '## 8. Exact next steps',
        '<!-- CHECKPOINT:BEGIN -->'
    )
    'TASKS.md'     = @('## Done', '## Doing', '## Todo')
    'DECISIONS.md' = @('### D-001', '### D-018', '## How to add a decision')
    'CHANGELOG.md' = @('## Unreleased', '## Released history', '## How to add an entry')
}
$MinChars = 1500

# Anything newer than PROJECT_STATUS.md means the status file is behind reality.
$Watch = @(
    'src', 'browser-extension', 'Cargo.toml', 'Cargo.lock', 'VERSION',
    'build.rs', 'resource.rc', 'build.ps1', 'AGENTS.md', 'PROGRESS.md'
)

function Get-GitInfo {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        Push-Location $Root
        $head = ((git rev-parse --short HEAD 2>$null) | Select-Object -First 1)
        $ref = ((git rev-parse --abbrev-ref HEAD 2>$null) | Select-Object -First 1)
        $tag = ((git describe --tags --abbrev=0 2>$null) | Select-Object -First 1)
        $dirty = @((git status --porcelain 2>$null) | Where-Object { $_.Trim() -ne '' }).Count
        if (-not $head) { $head = 'no-git' }
        if (-not $ref) { $ref = 'unknown' }
        if ($ref -eq 'HEAD') { $ref = 'detached' }
        if (-not $tag) { $tag = 'no-tag' }
        [pscustomobject]@{ Head = $head; Ref = $ref; Dirty = $dirty; Tag = $tag }
    }
    catch {
        [pscustomobject]@{ Head = 'no-git'; Ref = 'unknown'; Dirty = -1; Tag = 'no-tag' }
    }
    finally {
        $ErrorActionPreference = $prev
        Pop-Location
    }
}

function Get-Versions {
    $file = 'n/a'; $cargo = 'n/a'; $ext = 'n/a'
    $p = Join-Path $Root 'VERSION'
    if (Test-Path $p) { $file = (Get-Content -LiteralPath $p -Raw).Trim() }
    $p = Join-Path $Root 'Cargo.toml'
    if (Test-Path $p) {
        $m = Select-String -LiteralPath $p -Pattern '^\s*version\s*=\s*"([^"]+)"' |
            Select-Object -First 1
        if ($m) { $cargo = $m.Matches[0].Groups[1].Value }
    }
    # browser.rs is where the generated extension manifest version lives (D-014)
    $p = Join-Path (Join-Path $Root 'src') 'browser.rs'
    if (Test-Path $p) {
        $m = Select-String -LiteralPath $p -Pattern 'EXT_VERSION[^"]*"([^"]+)"' |
            Select-Object -First 1
        if ($m) { $ext = $m.Matches[0].Groups[1].Value }
    }
    [pscustomobject]@{ File = $file; Cargo = $cargo; Ext = $ext }
}

function Get-NewestSourceWrite {
    $newest = $null
    foreach ($item in $Watch) {
        $p = Join-Path $Root $item
        if (-not (Test-Path $p)) { continue }
        $files = if (Test-Path $p -PathType Container) {
            Get-ChildItem -LiteralPath $p -Recurse -File -ErrorAction SilentlyContinue
        }
        else {
            @(Get-Item -LiteralPath $p)
        }
        foreach ($f in $files) {
            if ($null -eq $newest -or $f.LastWriteTime -gt $newest) { $newest = $f.LastWriteTime }
        }
    }
    $newest
}

function Invoke-Status {
    $v = Get-Versions
    $g = Get-GitInfo
    Write-Host 'DM Ultra - checkpoint briefing'
    Write-Host ("  version (VERSION)  : {0}" -f $v.File)
    Write-Host ("  version (Cargo)    : {0}" -f $v.Cargo)
    Write-Host ("  version (extension): {0}" -f $v.Ext)
    if (($v.File -ne $v.Cargo) -or ($v.Cargo -ne $v.Ext)) {
        Write-Host '  NOTE  these three must agree - see DECISIONS.md D-014'
    }
    Write-Host ("  git                : {0} ({1}) dirty={2} newestTag={3}" -f $g.Head, $g.Ref, $g.Dirty, $g.Tag)

    $statusPath = Join-Path $Root 'PROJECT_STATUS.md'
    if (Test-Path $statusPath) {
        $line = Select-String -LiteralPath $statusPath -Pattern '^Last checkpoint:' |
            Select-Object -First 1
        if ($line) { Write-Host ("  {0}" -f $line.Line) }
        $item = Get-Item -LiteralPath $statusPath
        Write-Host ("  status file        : written {0:yyyy-MM-dd HH:mm}" -f $item.LastWriteTime)
    }

    $tasksPath = Join-Path $Root 'TASKS.md'
    if (Test-Path $tasksPath) {
        $raw = Get-Content -LiteralPath $tasksPath -Raw
        $done = ([regex]::Matches($raw, '- \[x\]')).Count
        $open = ([regex]::Matches($raw, '- \[ \]')).Count
        Write-Host ("  tasks              : {0} done, {1} open" -f $done, $open)
    }

    Write-Host '  prove the tree     : build.ps1 check / build.ps1 test /'
    Write-Host '                         node --test browser-extension/background.test.js browser-extension/content.test.js'
    Write-Host '  then read          : AGENTS.md, PROJECT_STATUS.md, TASKS.md, DECISIONS.md'
}

function Invoke-Verify {
    $problems = 0
    foreach ($name in $Memory.Keys) {
        $path = Join-Path $Root $name
        if (-not (Test-Path $path)) {
            Write-Host ("  FAIL  {0} is missing" -f $name); $problems++; continue
        }
        $raw = Get-Content -LiteralPath $path -Raw
        if ($raw.Length -lt $MinChars) {
            Write-Host ("  FAIL  {0} is only {1} chars (thin or truncated)" -f $name, $raw.Length)
            $problems++; continue
        }
        $missing = @($Memory[$name] | Where-Object { -not $raw.Contains($_) })
        if ($missing.Count -gt 0) {
            Write-Host ("  FAIL  {0} is missing section(s): {1}" -f $name, ($missing -join ' | '))
            $problems++; continue
        }
        Write-Host ("  ok    {0} ({1} chars)" -f $name, $raw.Length)
    }

    $statusPath = Join-Path $Root 'PROJECT_STATUS.md'
    if (Test-Path $statusPath) {
        $item = Get-Item -LiteralPath $statusPath
        $age = (Get-Date) - $item.LastWriteTime
        if ($age.TotalHours -gt $StaleHours) {
            Write-Host ("  FAIL  PROJECT_STATUS.md is {0:n1} h old (limit {1} h)" -f $age.TotalHours, $StaleHours)
            $problems++
        }
        $newest = Get-NewestSourceWrite
        if ($newest -and $newest -gt $item.LastWriteTime) {
            Write-Host '  warn  source or docs changed since the last checkpoint:'
            Write-Host '        checkpoint.ps1 save -Note "what just changed"'
        }
    }

    if ($problems -eq 0) {
        Write-Host 'PASS  memory system is complete and current'
    }
    else {
        Write-Host ("FAIL  {0} problem(s) with the memory system" -f $problems)
    }
    exit $problems
}

function Invoke-Save {
    if (-not $Note) {
        Write-Host 'save needs a note: checkpoint.ps1 save -Note "what just changed"'
        exit 2
    }
    $path = Join-Path $Root 'PROJECT_STATUS.md'
    if (-not (Test-Path $path)) { Write-Host ("missing {0}" -f $path); exit 2 }

    $stamp = Get-Date -Format 'yyyy-MM-dd HH:mm'
    $g = Get-GitInfo
    $v = Get-Versions
    $lines = @(Get-Content -LiteralPath $path)

    $found = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i].StartsWith('Last checkpoint:')) {
            $lines[$i] = ('Last checkpoint: {0} - {1} [git {2} {3}, dirty={4}, VERSION {5}]' -f
                $stamp, $Note, $g.Head, $g.Ref, $g.Dirty, $v.File)
            $found = $true
            break
        }
    }
    if (-not $found) { Write-Host 'no "Last checkpoint:" line found in PROJECT_STATUS.md'; exit 2 }

    $begin = -1; $end = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i].Contains('CHECKPOINT:BEGIN')) { $begin = $i }
        elseif ($lines[$i].Contains('CHECKPOINT:END')) { $end = $i; break }
    }
    if (($begin -lt 0) -or ($end -le $begin)) {
        Write-Host 'CHECKPOINT:BEGIN / CHECKPOINT:END markers missing from PROJECT_STATUS.md'; exit 2
    }

    $kept = @()
    for ($i = $begin + 1; $i -lt $end; $i++) {
        if ($lines[$i].Trim() -ne '') { $kept += $lines[$i] }
    }
    if ($kept.Count -gt 19) { $kept = $kept[0..18] }

    $entry = ('- {0} | git {1} {2} dirty={3} | {4}' -f $stamp, $g.Head, $g.Ref, $g.Dirty, $Note)
    $out = @()
    $out += $lines[0..$begin]
    $out += $entry
    $out += $kept
    $out += $lines[$end..($lines.Count - 1)]
    Set-Content -LiteralPath $path -Value $out -Encoding utf8NoBOM

    Write-Host ('checkpoint saved: {0}' -f $entry)
}

switch ($Mode) {
    'status' { Invoke-Status }
    'verify' { Invoke-Verify }
    'save'   { Invoke-Save }
}

