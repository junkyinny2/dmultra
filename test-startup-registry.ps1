# "Launch at Startup" registry test
#
# Regression: the Options toggle existed and saved to state.json, but
# nothing ever wrote the HKCU Run entry, so "Start with Windows" did nothing.
#
# The test drives the real CLI (`config auto-startup true|false`), checks the
# Run value, and restores both the previous Run entry and state.json.
#
# Usage: .\test-startup-registry.ps1

$ErrorActionPreference = 'Stop'

$exe = 'C:\dmultra\dmultra.exe'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$valueName = 'DM Ultra'
$statePath = Join-Path $env:APPDATA 'dmultra\state.json'
$expected = '"C:\dmultra\dmultra.exe" --background'

function Show-Check($label, $ok, $detail) {
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1}  {2}" -f $mark, $label, $detail)
    return $ok
}

function Get-RunValue {
    $props = Get-ItemProperty -Path $runKey -ErrorAction SilentlyContinue
    if ($null -eq $props) { return $null }
    if ($props.PSObject.Properties.Name -notcontains $valueName) { return $null }
    return $props.$valueName
}

function Invoke-Config([string]$flag) {
    $p = Start-Process -FilePath $exe -ArgumentList @('config', 'auto-startup', $flag) `
        -WorkingDirectory 'C:\dmultra' -Wait -PassThru -WindowStyle Hidden
    if ($p.ExitCode -ne 0) { throw "config auto-startup $flag exited with $($p.ExitCode)" }
}

function Stop-App {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 700
}

$results = @()
$stateBackup = $null
$hadValue = $false
$oldValue = $null

try {
    if (Get-Process dmultra -ErrorAction SilentlyContinue) {
        throw 'dmultra is running; stop it before running this test'
    }
    if (Test-Path $statePath) {
        $stateBackup = [System.IO.File]::ReadAllText($statePath)
    }
    $oldValue = Get-RunValue
    $hadValue = $null -ne $oldValue

    # ------------------------------------------------------------- disabled
    Invoke-Config 'false'
    $value = Get-RunValue
    $results += Show-Check 'off -> no Run value' ($null -eq $value) ("value={0}" -f $value)

    # -------------------------------------------------------------- enabled
    Invoke-Config 'true'
    $value = Get-RunValue
    $results += Show-Check 'on -> Run value exists' ($null -ne $value) ("value={0}" -f $value)
    $results += Show-Check 'Run value points at dmultra --background' `
        ($null -ne $value -and $value.Trim().Equals($expected, [StringComparison]::OrdinalIgnoreCase)) `
        ("expected={0}" -f $expected)

    $saved = (Get-Content $statePath -Raw | ConvertFrom-Json).settings
    $results += Show-Check 'option persisted in state.json' ($saved.auto_startup -eq $true) `
        ("auto_startup={0}" -f $saved.auto_startup)

    # ------------------------------------------------------- off removes it
    Invoke-Config 'false'
    $value = Get-RunValue
    $results += Show-Check 'off again -> Run value removed' ($null -eq $value) ("value={0}" -f $value)
    $saved = (Get-Content $statePath -Raw | ConvertFrom-Json).settings
    $results += Show-Check 'off persisted in state.json' ($saved.auto_startup -eq $false) `
        ("auto_startup={0}" -f $saved.auto_startup)
}
finally {
    Stop-App
    if ($null -ne $stateBackup) {
        [System.IO.File]::WriteAllText($statePath, $stateBackup)
    }
    if ($hadValue) {
        New-Item -Path $runKey -Force | Out-Null
        Set-ItemProperty -Path $runKey -Name $valueName -Value $oldValue
    } else {
        Remove-ItemProperty -Path $runKey -Name $valueName -ErrorAction SilentlyContinue
    }
}

Write-Host ''
if ($results -contains $false) { exit 1 } else { exit 0 }
