# Minimize behavior test (taskbar / taskbar + tray / tray only)
#
# IDM parity: the minimize button must be configurable. The default keeps the
# taskbar button and shows a tray icon; "tray only" hides the window (and its
# taskbar button) so only the notification-area icon remains.
#
# Usage: .\test-minimize-modes.ps1

$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;

public class MinMode {
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    delegate bool EnumProc(IntPtr h, IntPtr l);

    static IntPtr Found;
    static uint Pid;

    public static IntPtr Find(uint pid) {
        Found = IntPtr.Zero;
        Pid = pid;
        EnumWindows(new EnumProc(Cb), IntPtr.Zero);
        return Found;
    }

    static bool Cb(IntPtr h, IntPtr l) {
        uint owner;
        GetWindowThreadProcessId(h, out owner);
        if (owner != Pid) return true;
        var sb = new StringBuilder(256);
        GetWindowTextW(h, sb, 256);
        string t = sb.ToString();
        if (t.StartsWith("DM Ultra")) { Found = h; return false; }
        return true;
    }
}
"@

$exe = 'C:\dmultra\dmultra.exe'
$statePath = Join-Path $env:APPDATA 'dmultra\state.json'
$logPath = Join-Path $env:APPDATA 'dmultra\logs\dmultra.log'
$stateBackup = $null

function Show-Check($label, $ok, $detail) {
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1}  {2}" -f $mark, $label, $detail)
    return $ok
}

function Stop-App {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 900
}

function Set-Mode([string]$mode) {
    Stop-App
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    # Older state files predate the option, so add the property if missing.
    $state.settings | Add-Member -NotePropertyName minimize_mode -NotePropertyValue $mode -Force
    [System.IO.File]::WriteAllText($statePath, ($state | ConvertTo-Json -Depth 30))
}

function Start-App {
    Start-Process -FilePath $exe -WorkingDirectory 'C:\dmultra'
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 300
        $proc = Get-Process dmultra -ErrorAction SilentlyContinue
        if (-not $proc) { continue }
        $h = [MinMode]::Find([uint32]$proc[0].Id)
        if ($h -ne [IntPtr]::Zero) {
            Start-Sleep -Milliseconds 900
            return @{ Proc = $proc[0]; Hwnd = $h }
        }
    }
    throw 'main window did not appear'
}

function Log-Text {
    if (-not (Test-Path $logPath)) { return '' }
    return Get-Content $logPath -Raw -ErrorAction SilentlyContinue
}

$results = @()
$current = $null

try {
    if (Get-Process dmultra -ErrorAction SilentlyContinue) {
        throw 'dmultra is running; stop it before running this test'
    }
    if (Test-Path $statePath) {
        $stateBackup = [System.IO.File]::ReadAllText($statePath)
    }

    # ------------------------------------------------- 1. taskbar only
    Set-Mode 'taskbar'
    $current = Start-App
    $h = $current.Hwnd
    $results += Show-Check 'taskbar: window starts visible' ([MinMode]::IsWindowVisible($h)) ''
    [MinMode]::ShowWindow($h, 6) | Out-Null       # SW_MINIMIZE
    Start-Sleep -Milliseconds 1200
    $results += Show-Check 'taskbar: minimize keeps the window iconic' ([MinMode]::IsIconic($h)) ''
    $results += Show-Check 'taskbar: window stays on the taskbar' ([MinMode]::IsWindowVisible($h)) ''
    $log = Log-Text
    $results += Show-Check 'taskbar: tray icon was created' ($log -match 'tray icon created') ''
    $results += Show-Check 'taskbar: no tray visibility failure' ($log -notmatch 'could not change tray icon visibility') ''

    # ------------------------------------------- 2. taskbar + tray icon
    Set-Mode 'taskbar_tray'
    $current = Start-App
    $h = $current.Hwnd
    [MinMode]::ShowWindow($h, 6) | Out-Null
    Start-Sleep -Milliseconds 1200
    $results += Show-Check 'taskbar+tray: minimize keeps the taskbar button' `
        ([MinMode]::IsIconic($h) -and [MinMode]::IsWindowVisible($h)) ''
    $results += Show-Check 'taskbar+tray: tray icon present' `
        ((Log-Text) -match 'tray icon created' -and (Log-Text) -notmatch 'tray icon unavailable') ''

    # ------------------------------------------------ 3. tray only
    Set-Mode 'tray'
    $current = Start-App
    $h = $current.Hwnd
    [MinMode]::ShowWindow($h, 6) | Out-Null
    Start-Sleep -Milliseconds 1400
    $results += Show-Check 'tray only: window hidden' (-not [MinMode]::IsWindowVisible($h)) ''
    $results += Show-Check 'tray only: not left iconic (taskbar button gone)' (-not [MinMode]::IsIconic($h)) ''
    $results += Show-Check 'tray only: logged the tray minimize' `
        ((Log-Text) -match 'minimize button hid the main window to the tray') ''

    # ------------------------------------- 4. close button hides to tray
    [MinMode]::PostMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # WM_CLOSE
    Start-Sleep -Milliseconds 1500
    $alive = $null -ne (Get-Process dmultra -ErrorAction SilentlyContinue)
    $results += Show-Check 'tray only: close hides instead of quitting' $alive ("process alive={0}" -f $alive)
    $results += Show-Check 'tray only: close keeps the window hidden' `
        ($alive -and -not [MinMode]::IsWindowVisible($h)) ''
}
finally {
    Stop-App
    if ($null -ne $stateBackup) {
        [System.IO.File]::WriteAllText($statePath, $stateBackup)
    }
}

Write-Host ''
if ($results -contains $false) { exit 1 } else { exit 0 }
