# Regression test: change Minimize behavior to "Tray only" from the real
# Options dialog while the main window is parked, then close it and assert
# that no taskbar button remains.
#
# test-minimize-modes.ps1 starts a fresh process for each mode, so it cannot
# cover this transition. Changing the option activates the parked main window
# (D-023 restores on activation), so the button legitimately comes back until
# the next close/minimize. The contract checked here is that the following
# close hides the window from the taskbar - the path v0.078 repaired by making
# park() refresh the shell after changing WS_EX_TOOLWINDOW.
#
# Usage: pwsh -NoProfile -File .\test-tray-only-transition.ps1
#        Run in a fresh shell (Add-Type names collide in a reused shell), and
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
    public static bool HasTaskbarButton(IntPtr h) {
        if (!IsWindow(h) || IsIconic(h) || !IsWindowVisible(h)) return false;
        long ex = GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64();
        return (ex & WS_EX_TOOLWINDOW) == 0;
    }
    public static string Describe(IntPtr h) {
        RECT r; string pos = GetWindowRect(h, out r)
            ? string.Format("({0},{1})", r.Left, r.Top) : "(no rect)";
        return string.Format("iconic={0} visible={1} at={2} taskbar={3}",
            IsIconic(h), IsWindowVisible(h), pos, HasTaskbarButton(h));
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

function Stop-App {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 700
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
    Start-Sleep -Milliseconds 900

    $root = [Windows.Automation.AutomationElement]::RootElement
    $pidCond = New-Object Windows.Automation.PropertyCondition(
        [Windows.Automation.AutomationElement]::ProcessIdProperty, [int]$proc.Id)
    # Match the title by prefix so a VERSION bump does not break the test.
    $appWindows = $root.FindAll([Windows.Automation.TreeScope]::Children, $pidCond)
    $mainEl = $appWindows | Where-Object { $_.Current.Name -like 'DM Ultra*' } | Select-Object -First 1
    if ($null -eq $mainEl) { throw 'main UI Automation element not found' }

    # Open Options from Downloads -> Options... (a separate OS window).
    $downloads = Find-Element $mainEl 'Downloads'
    if ($null -eq $downloads) { throw 'Downloads menu not found' }
    $downloads.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Start-Sleep -Milliseconds 400
    $optionsMenu = Find-Element $mainEl 'Options...'
    if ($null -eq $optionsMenu) { throw 'Options... menu item not found' }
    $optionsMenu.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Start-Sleep -Milliseconds 900

    $optionsEl = $root.FindFirst(
        [Windows.Automation.TreeScope]::Children,
        (New-Object Windows.Automation.AndCondition(
            $pidCond,
            (New-Object Windows.Automation.PropertyCondition(
                [Windows.Automation.AutomationElement]::NameProperty, 'Options')))))
    if ($null -eq $optionsEl) { throw 'Options window did not open' }
    $r = $optionsEl.Current.BoundingRectangle
    $ox = [int]$r.X
    $oy = [int]$r.Y

    # Park the main window while Options stays open. SC_MINIMIZE is the real
    # path for taskbar_tray: WM_CLOSE always calls hide_main_window(), which
    # deliberately hides from the taskbar regardless of the selected mode.
    # In taskbar_tray mode the taskbar button must remain - this is the
    # starting state of the bug.
    [TrayTransition]::PostMessageW($hwnd, 0x0112, [IntPtr]0xF020, [IntPtr]::Zero) | Out-Null
    $parked = Wait-For { [TrayTransition]::IsParkedOffScreen($hwnd) }
    # Give the parked window and the Options viewport time to settle: clicks
    # sent during the first repaint after SC_MINIMIZE are swallowed.
    Start-Sleep -Milliseconds 900
    Check 'main parked before changing the option' $parked ([TrayTransition]::Describe($hwnd))
    Check 'taskbar button present before the change' ([TrayTransition]::HasTaskbarButton($hwnd)) ([TrayTransition]::Describe($hwnd))

    # Click the Minimize behavior dropdown, then "Tray only (hide from
    # taskbar)". Offsets are relative to the observed 816x619 Options window.
    [TrayTransition]::Click(($ox + 590), ($oy + 214))
    Start-Sleep -Milliseconds 500
    [TrayTransition]::Click(($ox + 590), ($oy + 288))
    Start-Sleep -Milliseconds 400
    [TrayTransition]::Click(($ox + 660), ($oy + 587))  # Apply
    Start-Sleep -Milliseconds 900

    $saved = Get-Content $statePath -Raw | ConvertFrom-Json
    Check 'settings saved as tray' ($saved.settings.minimize_mode -eq 'tray') "minimize_mode=$($saved.settings.minimize_mode)"
    Check 'process alive after changing the option' ([bool](Get-Process dmultra -ErrorAction SilentlyContinue)) ''

    # Close the dialog as a user would. OK closes it; leaving Options open
    # keeps activating the parked main window and would hide the result.
    [TrayTransition]::Click(($ox + 780), ($oy + 587))  # OK
    Start-Sleep -Milliseconds 900

    # The Options interaction restores the parked window by design (D-023).
    # Now use the real close path: in tray-only mode it must park off-screen
    # and take the taskbar button with it.
    [TrayTransition]::PostMessageW($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    $hidden = Wait-For {
        [TrayTransition]::IsParkedOffScreen($hwnd) -and -not [TrayTransition]::HasTaskbarButton($hwnd)
    }
    Check 'close hides from the taskbar after switching to tray only' $hidden ([TrayTransition]::Describe($hwnd))
    Check 'process still alive after close in tray only' ([bool](Get-Process dmultra -ErrorAction SilentlyContinue)) ''
    Check 'main window parked' ([TrayTransition]::IsParkedOffScreen($hwnd)) ([TrayTransition]::Describe($hwnd))

    $logAfter = if (Test-Path $logPath) { Get-Content $logPath -Raw } else { '' }
    $newLog = $logAfter.Substring([Math]::Min($logBefore.Length, $logAfter.Length))
    Check 'tray close logged' ($newLog -match 'close button hid the main window to the tray') ''
    if ($env:DMULTRA_TRANSITION_DEBUG -eq '1') {
        ($newLog -split "`n") | Select-String -Pattern 'TRAY|WIN' | Select-Object -Last 20 | ForEach-Object { Write-Host "  LOG $($_.Line.Trim())" }
    }
}
catch {
    Check 'script completed' $false ($_.Exception.Message + ' | ' + $_.ScriptStackTrace)
}
finally {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    if ($null -ne $stateBackup) {
        [System.IO.File]::WriteAllText($statePath, $stateBackup)
    }
    Remove-Item Env:DMULTRA_DEBUG -ErrorAction SilentlyContinue
}

if ($failures -eq 0) {
    Write-Host 'PASS tray-only transition removed the taskbar button'
    exit 0
}
Write-Host "FAIL $failures check(s)"
exit 1
