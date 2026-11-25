# Regression test: changing Minimize behavior to "Tray only" from the real
# Options dialog must remove the *actual shell taskbar button* immediately,
# not merely set WS_EX_TOOLWINDOW. Windows 11 keeps an existing button until
# ITaskbarList::DeleteTab is called, so the style bit is not a sufficient check.
#
# Usage: pwsh -NoProfile -File .\test-tray-only-transition.ps1
#        Run in a fresh shell (Add-Type names collide in a reused one), and
#        stop any running dmultra instance first.

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @"
using System;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;

public class TrayTransition {
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr h, int idx);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);

    [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left, Top, Right, Bottom; }
    const int GWL_EXSTYLE = -20;
    const int WS_EX_TOOLWINDOW = 0x80;
    delegate bool EnumProc(IntPtr h, IntPtr l);

    static IntPtr Found;
    static uint Pid;

    public static IntPtr FindMain(uint pid) {
        Found = IntPtr.Zero; Pid = pid;
        EnumWindows(new EnumProc(Cb), IntPtr.Zero);
        return Found;
    }
    static bool Cb(IntPtr h, IntPtr l) {
        uint owner; GetWindowThreadProcessId(h, out owner);
        if (owner != Pid) return true;
        var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256);
        if (sb.ToString().StartsWith("DM Ultra")) { Found = h; return false; }
        return true;
    }
    public static bool IsParkedOffScreen(IntPtr h) {
        RECT r; if (!GetWindowRect(h, out r)) return false;
        return r.Left <= -10000 || r.Right <= -10000;
    }
    public static string Describe(IntPtr h) {
        RECT r; string pos = GetWindowRect(h, out r)
            ? string.Format("({0},{1})", r.Left, r.Top) : "(no rect)";
        long ex = IsWindow(h) ? GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64() : 0;
        return string.Format("iconic={0} visible={1} at={2} tool={3}",
            IsIconic(h), IsWindowVisible(h), pos, (ex & WS_EX_TOOLWINDOW) != 0);
    }
    public static void Click(int x, int y) {
        // egui needs a painted hover frame before the press; firing the move
        // and both button events in one tick makes combo selections flaky.
        SetCursorPos(x, y);
        Thread.Sleep(180);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero);
        Thread.Sleep(60);
        mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }
}
"@

$AE = [System.Windows.Automation.AutomationElement]
$Ids = [System.Windows.Automation.AutomationElementIdentifiers]

function TaskbarButtonNames {
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

function TaskbarButtonCount {
    return @(TaskbarButtonNames).Count
}

$exe = 'C:\dmultra\dmultra.exe'
$statePath = Join-Path $env:APPDATA 'dmultra\state.json'
$logPath = Join-Path $env:APPDATA 'dmultra\logs\dmultra.log'
$stateBackup = $null
$logBefore = ''
$failures = 0

function Check($label, [bool]$ok, $detail) {
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    if (-not $ok) { $script:failures++ }
    Write-Host ("[{0}] {1}  {2}" -f $mark, $label, $detail)
}

function Wait-For([scriptblock]$predicate, [int]$tries = 40) {
    for ($i = 0; $i -lt $tries; $i++) {
        Start-Sleep -Milliseconds 150
        if (& $predicate) { return $true }
    }
    return $false
}

function Find-Element($root, [string]$name) {
    $cond = New-Object Windows.Automation.PropertyCondition(
        [Windows.Automation.AutomationElement]::NameProperty, $name)
    return $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $cond)
}

function Wait-Element($root, [string]$name, [int]$tries = 20) {
    for ($i = 0; $i -lt $tries; $i++) {
        $found = Find-Element $root $name
        if ($null -ne $found) { return $found }
        Start-Sleep -Milliseconds 150
    }
    return $null
}

try {
    if (Get-Process dmultra -ErrorAction SilentlyContinue) {
        throw 'dmultra is running; stop it before running this test'
    }
    if (Test-Path $statePath) {
        $stateBackup = [System.IO.File]::ReadAllText($statePath)
    }
    if (Test-Path $logPath) {
        $logBefore = Get-Content $logPath -Raw
    }

    # Start in the mode that keeps the taskbar button while parked.
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    $state.settings | Add-Member -NotePropertyName minimize_mode -NotePropertyValue 'taskbar_tray' -Force
    [System.IO.File]::WriteAllText($statePath, ($state | ConvertTo-Json -Depth 30))

    $env:DMULTRA_DEBUG = '1'
    Start-Process -FilePath $exe -WorkingDirectory 'C:\dmultra'
    $proc = $null
    $hwnd = [IntPtr]::Zero
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 300
        $candidate = Get-Process dmultra -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $candidate) { continue }
        $h = [TrayTransition]::FindMain([uint32]$candidate.Id)
        if ($h -ne [IntPtr]::Zero) { $proc = $candidate; $hwnd = $h; break }
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw 'main window did not appear' }

    $root = $AE::RootElement
    $pidCond = New-Object Windows.Automation.PropertyCondition($AE::ProcessIdProperty, [int]$proc.Id)
    # The Win32 hwnd exists before winit has published the title to UIA, so
    # enumerating root children here can see only the invisible event-target
    # pane. Bind straight to the handle and give the title a few frames to land.
    $mainEl = $null
    for ($i = 0; $i -lt 20; $i++) {
        $candidateEl = $AE::FromHandle($hwnd)
        if ($null -ne $candidateEl -and $candidateEl.Current.Name -like 'DM Ultra*') {
            $mainEl = $candidateEl
            break
        }
        Start-Sleep -Milliseconds 150
    }
    if ($null -eq $mainEl) { throw 'main UI Automation element not found' }

    # Open Options from Downloads -> Options... (a separate OS window).
    $downloads = Wait-Element $mainEl 'Downloads'
    if ($null -eq $downloads) { throw 'Downloads menu not found' }
    $downloads.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    $optionsMenu = Wait-Element $mainEl 'Options...'
    if ($null -eq $optionsMenu) { throw 'Options... menu item not found' }
    $optionsMenu.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Start-Sleep -Milliseconds 900

    $optionsEl = $root.FindFirst(
        [Windows.Automation.TreeScope]::Children,
        (New-Object Windows.Automation.AndCondition(
            $pidCond,
            (New-Object Windows.Automation.PropertyCondition($AE::NameProperty, 'Options')))))
    if ($null -eq $optionsEl) { throw 'Options window did not open' }
    $r = $optionsEl.Current.BoundingRectangle
    $ox = [int]$r.X
    $oy = [int]$r.Y

    # Park the main window while Options stays open. SC_MINIMIZE is the real
    # taskbar_tray path; WM_CLOSE always hides regardless of the selected mode.
    [TrayTransition]::PostMessageW($hwnd, 0x0112, [IntPtr]0xF020, [IntPtr]::Zero) | Out-Null
    $parked = Wait-For { [TrayTransition]::IsParkedOffScreen($hwnd) }
    Start-Sleep -Milliseconds 900
    Check 'main parked before changing the option' $parked ([TrayTransition]::Describe($hwnd))
    # With the main window in taskbar_tray mode and Options open, Windows shows
    # one grouped button for both windows. This is the real shell button, not
    # the WS_EX_TOOLWINDOW proxy the older suites used.
    $beforeNames = (TaskbarButtonNames) -join '|'
    Check 'main window in the grouped taskbar button before the change' ($beforeNames -match '2 running windows') "buttons=$beforeNames"

    # Click the Minimize behavior dropdown, then "Tray only (hide from
    # taskbar)" and Apply. Coordinates are relative to the Options viewport and
    # were measured from the real dialog (see PROGRESS.md v0.079).
    [TrayTransition]::Click(($ox + 590), ($oy + 214))
    Start-Sleep -Milliseconds 700
    [TrayTransition]::Click(($ox + 590), ($oy + 288))
    Start-Sleep -Milliseconds 700
    [TrayTransition]::Click(($ox + 660), ($oy + 587))  # Apply
    Start-Sleep -Milliseconds 900

    $saved = Get-Content $statePath -Raw | ConvertFrom-Json
    Check 'settings saved as tray' ($saved.settings.minimize_mode -eq 'tray') "minimize_mode=$($saved.settings.minimize_mode)"
    # The Options window is a separate OS window, so the group remains until
    # it closes. What must change at Apply is that the main window leaves the
    # group: two windows -> one.
    $afterApply = (TaskbarButtonNames) -join '|'
    Check 'main window leaves the taskbar group as soon as Apply is pressed' ($afterApply -match '1 running window') "buttons=$afterApply $([TrayTransition]::Describe($hwnd))"

    [TrayTransition]::Click(($ox + 780), ($oy + 587))  # OK
    Start-Sleep -Milliseconds 900
    $afterOk = TaskbarButtonCount
    Check 'taskbar button stays gone after closing Options' ($afterOk -eq 0) "buttons=$afterOk $([TrayTransition]::Describe($hwnd))"
    Check 'process alive after changing the option' ([bool](Get-Process dmultra -ErrorAction SilentlyContinue)) ''

    # The real close path must not resurrect the button.
    [TrayTransition]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Milliseconds 900
    $afterClose = TaskbarButtonCount
    Check 'close keeps the taskbar button gone' ($afterClose -eq 0) "buttons=$afterClose $([TrayTransition]::Describe($hwnd))"
    Check 'main window parked' ([TrayTransition]::IsParkedOffScreen($hwnd)) ([TrayTransition]::Describe($hwnd))
    Check 'process still alive after close in tray only' ([bool](Get-Process dmultra -ErrorAction SilentlyContinue)) ''

    $logAfter = if (Test-Path $logPath) { Get-Content $logPath -Raw } else { '' }
    $newLog = $logAfter.Substring([Math]::Min($logBefore.Length, $logAfter.Length))
    Check 'explicit taskbar tab deletion logged' ($newLog -match 'taskbar tab deleted') ''
    Check 'mode-change hide logged' ($newLog -match 'minimize mode changed to tray only') ''
    if ($env:DMULTRA_TRANSITION_DEBUG -eq '1') {
        ($newLog -split "`n") | Select-String -Pattern 'TRAY|WIN' | Select-Object -Last 20 | ForEach-Object { Write-Host "  LOG $($_.Line.Trim())" }
    }
} catch {
    Check 'script completed' $false ($_.Exception.Message + ' | ' + $_.ScriptStackTrace)
} finally {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    if ($null -ne $stateBackup) { [System.IO.File]::WriteAllText($statePath, $stateBackup) }
    Remove-Item Env:DMULTRA_DEBUG -ErrorAction SilentlyContinue
}

if ($failures -eq 0) {
    Write-Host 'PASS tray-only transition removed the real taskbar button'
} else {
    Write-Host "FAIL $failures check(s)"
}
exit $failures
