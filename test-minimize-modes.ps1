# Minimize behavior test (taskbar / taskbar + tray / tray only)
#
# IDM parity: the minimize button must be configurable. The default keeps the
# taskbar button and shows a tray icon; "tray only" hides the window (and its
# taskbar button) so only the notification-area icon remains.
#
# Since v0.073 (DECISIONS.md D-007) "hidden" is NOT an iconic minimize: the
# main window is parked off-screen at (-32000,-32000) while staying WS_VISIBLE,
# because eframe paints the download popouts from the root frame and stops
# scheduling frames for an iconic root. "tray only" additionally sets
# WS_EX_TOOLWINDOW so the taskbar button goes away; "taskbar + tray" keeps it.
# Only plain "taskbar" mode (with no popout open) is allowed to become iconic.
# So these checks assert "out of sight" (iconic OR hidden OR off-screen) plus
# the shell's ACTUAL taskbar button (queried through UI Automation), never
# IsIconic or WS_EX_TOOLWINDOW alone - both stale assumptions made suites pass
# while the user could still see the button. Windows 11 keeps an existing
# button until ITaskbarList::DeleteTab is called (v0.079, D-025).
#
# Usage: .\test-minimize-modes.ps1
#        Run it in a fresh shell (pwsh -NoProfile -File .\test-minimize-modes.ps1);
#        a reused shell already has the MinMode type loaded and Add-Type collides.

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type @"
using System;
using System.Collections.Generic;
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
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr h, int idx);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    const int GWL_EXSTYLE = -20;
    const int WS_EX_TOOLWINDOW = 0x80;
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

    // True when the window sits at the park position (browser.rs park()).
    // Windows also reports (-32000,-32000) for any iconic window, so this is
    // only meaningful inside IsOutOfSight, as one of the three ways a window
    // can be off the desktop.
    public static bool IsParkedOffScreen(IntPtr h) {
        RECT r;
        if (!GetWindowRect(h, out r)) return false;
        return r.Left <= -10000 || r.Right <= -10000;
    }

    // The user-visible contract of D-007: the window is gone from the desktop.
    // Iconic, hidden and parked all qualify; only an on-screen window does not.
    public static bool IsOutOfSight(IntPtr h) {
        if (!IsWindow(h)) return true;
        return !IsWindowVisible(h) || IsIconic(h) || IsParkedOffScreen(h);
    }

    public static string Describe(IntPtr h) {
        RECT r;
        string pos = GetWindowRect(h, out r)
            ? string.Format("({0},{1})", r.Left, r.Top) : "(no rect)";
        long ex = IsWindow(h) ? GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64() : 0;
        return string.Format("iconic={0} visible={1} at={2} tool={3}",
            IsIconic(h), IsWindowVisible(h), pos, (ex & WS_EX_TOOLWINDOW) != 0);
    }

    static IntPtr MainHwnd;
    static List<IntPtr> Others = new List<IntPtr>();
    static bool CbOther(IntPtr h, IntPtr l) {
        uint owner;
        GetWindowThreadProcessId(h, out owner);
        if (owner != Pid || h == MainHwnd) return true;
        var sb = new StringBuilder(256);
        GetWindowTextW(h, sb, 256);
        // Skip the untitled helper windows every GL/IME process owns.
        if (sb.Length == 0) return true;
        string t = sb.ToString();
        if (t == "__wglDummyWindowFodder" || t == "NVOGLDC invisible"
            || t == "MSCTFIME UI" || t == "Default IME") return true;
        Others.Add(h);
        return true;
    }

    // Every other titled top-level window of this process = the popouts that
    // make gui.rs park the main window even in plain "taskbar" mode.
    public static IntPtr[] OtherTopWindows(uint pid, IntPtr main) {
        Pid = pid; MainHwnd = main; Others = new List<IntPtr>();
        EnumWindows(new EnumProc(CbOther), IntPtr.Zero);
        return Others.ToArray();
    }

    public static string Title(IntPtr h) {
        var sb = new StringBuilder(256);
        GetWindowTextW(h, sb, 256);
        return sb.ToString();
    }
}
"@

$exe = 'C:\dmultra\dmultra.exe'
$statePath = Join-Path $env:APPDATA 'dmultra\state.json'
$logPath = Join-Path $env:APPDATA 'dmultra\logs\dmultra.log'
$stateBackup = $null

# The shell's own taskbar buttons, not WS_EX_TOOLWINDOW. Windows 11 keeps a
# grouped button after the style bit changes until ITaskbarList::DeleteTab is
# called, so the style flag cannot answer "is it still on the taskbar?".
# The button belongs to Explorer and is named "dmultra - N running windows".
$AE = [System.Windows.Automation.AutomationElement]
$Ids = [System.Windows.Automation.AutomationElementIdentifiers]
function Taskbar-ButtonNames {
    $taskbar = $AE::RootElement.FindFirst(
        [System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition($Ids::ClassNameProperty, 'Shell_TrayWnd')))
    if ($null -eq $taskbar) { throw 'Shell_TrayWnd not found' }
    $buttons = $taskbar.FindAll(
        [System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition($Ids::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::Button)))
    return @($buttons | Where-Object { $_.Current.Name -match 'dmultra' } | ForEach-Object { $_.Current.Name })
}
function Taskbar-Note {
    $names = @(Taskbar-ButtonNames)
    return ("taskbarButtons={0}:{1}" -f $names.Count, ($names -join '|'))
}

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

function Wait-WindowShown($hwnd) {
    for ($i = 0; $i -lt 25; $i++) {
        Start-Sleep -Milliseconds 100
        if (-not [MinMode]::IsOutOfSight($hwnd)) { return $true }
    }
    return $false
}

function Wait-WindowHidden($hwnd) {
    for ($i = 0; $i -lt 25; $i++) {
        Start-Sleep -Milliseconds 100
        if ([MinMode]::IsOutOfSight($hwnd)) { return $true }
    }
    return $false
}

# The message the shell sends when its taskbar button is clicked for a window
# it believes is open (WM_SYSCOMMAND / SC_MINIMIZE). A parked window is exactly
# that: still WS_VISIBLE and non-iconic, so this is what a real click produces.
function Invoke-TaskbarClick($hwnd) {
    [MinMode]::PostMessageW($hwnd, 0x0112, [IntPtr]0xF020, [IntPtr]::Zero) | Out-Null
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
    # gui.rs:2348 parks the window even in this mode while a popout is open
    # (has_child), because the popout is painted by the root frame. Measure the
    # app's own popout windows instead of assuming there are none - a leftover
    # progress window in the state file otherwise silently changes the expected
    # outcome here (that is what made this check flip between two runs).
    $popouts = @([MinMode]::OtherTopWindows([uint32]$current.Proc.Id, $h))
    $popoutNote = if ($popouts.Count) { ': ' + ((($popouts | ForEach-Object { [MinMode]::Title($_) })) -join '; ') } else { ' none' }
    Write-Host ("  popouts open before minimize: {0}{1}" -f $popouts.Count, $popoutNote)
    $results += Show-Check 'taskbar: window starts visible' ([MinMode]::IsWindowVisible($h)) ''
    [MinMode]::ShowWindow($h, 6) | Out-Null       # SW_MINIMIZE
    Start-Sleep -Milliseconds 1200
    # No popout -> a real iconic minimize. Popout open -> parked off-screen,
    # still on the taskbar. Anything else is a regression.
    $minimized = if ($popouts.Count -eq 0) { [MinMode]::IsIconic($h) } else { [MinMode]::IsOutOfSight($h) }
    $results += Show-Check 'taskbar: minimize leaves the window off the desktop' $minimized `
        ("popouts={0} {1}" -f $popouts.Count, [MinMode]::Describe($h))
    $results += Show-Check 'taskbar: window stays on the taskbar' `
        ((@(Taskbar-ButtonNames)).Count -ge 1) ("{0} {1}" -f (Taskbar-Note), [MinMode]::Describe($h))
    $log = Log-Text
    $results += Show-Check 'taskbar: tray icon was created' ($log -match 'tray icon created') ''
    $results += Show-Check 'taskbar: no tray visibility failure' ($log -notmatch 'could not change tray icon visibility') ''

    # ------------------------------------------- 2. taskbar + tray icon
    Set-Mode 'taskbar_tray'
    $current = Start-App
    $h = $current.Hwnd
    [MinMode]::ShowWindow($h, 6) | Out-Null
    Start-Sleep -Milliseconds 1200
    # D-007: this mode parks the window off-screen instead of minimizing it, so
    # it is out of sight but still owns its taskbar button (WS_VISIBLE, no
    # WS_EX_TOOLWINDOW). Clicking that button unparks it.
    $results += Show-Check 'taskbar+tray: minimize keeps the taskbar button' `
        ((@(Taskbar-ButtonNames)).Count -ge 1 -and [MinMode]::IsOutOfSight($h)) `
        ("{0} {1}" -f (Taskbar-Note), [MinMode]::Describe($h))
    $results += Show-Check 'taskbar+tray: tray icon present' `
        ((Log-Text) -match 'tray icon created' -and (Log-Text) -notmatch 'tray icon unavailable') ''

    # The reported bug: clicking the taskbar icon of the parked window did
    # nothing. A parked window is still WS_VISIBLE and non-iconic, so the shell
    # sends SC_MINIMIZE (its idea of minimizing an open window) rather than
    # SC_RESTORE, and the hook used to intercept that and park it again. The
    # click must show the window, and the next click must hide it again.
    Invoke-TaskbarClick $h
    $results += Show-Check 'taskbar+tray: taskbar click shows the parked window' `
        (Wait-WindowShown $h) ([MinMode]::Describe($h))
    Invoke-TaskbarClick $h
    $results += Show-Check 'taskbar+tray: clicking the icon again parks it again' `
        (Wait-WindowHidden $h) ([MinMode]::Describe($h))

    # ------------------------------------------------ 3. tray only
    Set-Mode 'tray'
    $current = Start-App
    $h = $current.Hwnd
    [MinMode]::ShowWindow($h, 6) | Out-Null
    Start-Sleep -Milliseconds 1400
    $results += Show-Check 'tray only: window out of sight' ([MinMode]::IsOutOfSight($h)) ([MinMode]::Describe($h))
    $results += Show-Check 'tray only: parked, not iconic' (-not [MinMode]::IsIconic($h)) ([MinMode]::Describe($h))
    $results += Show-Check 'tray only: taskbar button gone' `
        ((@(Taskbar-ButtonNames)).Count -eq 0) ("{0} {1}" -f (Taskbar-Note), [MinMode]::Describe($h))
    $results += Show-Check 'tray only: logged the tray minimize' `
        ((Log-Text) -match 'minimize button hid the main window to the tray') ''

    # ------------------------------------- 4. close button hides to tray
    [MinMode]::PostMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # WM_CLOSE
    Start-Sleep -Milliseconds 1500
    $alive = $null -ne (Get-Process dmultra -ErrorAction SilentlyContinue)
    $results += Show-Check 'tray only: close hides instead of quitting' $alive ("process alive={0}" -f $alive)
    $results += Show-Check 'tray only: close keeps the window out of sight' `
        ($alive -and [MinMode]::IsOutOfSight($h) -and (@(Taskbar-ButtonNames)).Count -eq 0) `
        ("{0} {1}" -f (Taskbar-Note), [MinMode]::Describe($h))
}
finally {
    Stop-App
    if ($null -ne $stateBackup) {
        [System.IO.File]::WriteAllText($statePath, $stateBackup)
    }
}

Write-Host ''
if ($results -contains $false) { exit 1 } else { exit 0 }
