# Verifies that a second launch reveals the already-running instance's
# hidden window instead of starting a duplicate process.
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
# The repo tree holds no binaries (AGENTS.md: build output stays ignored); the
# deployed binary is the one under test.
$exe = 'C:\dmultra\dmultra.exe'
$stateDir = Join-Path $env:APPDATA 'dmultra'
$statePath = Join-Path $stateDir 'state.json'
$showRequest = Join-Path $stateDir 'show.request'

if (Get-Process dmultra -ErrorAction SilentlyContinue) {
    throw 'dmultra is running; stop it before running this test'
}

$backup = if (Test-Path $statePath) { Get-Content $statePath -Raw } else { $null }

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class SingleWin {
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    delegate bool EnumProc(IntPtr h, IntPtr l);
    static IntPtr Found;
    static uint Wanted;
    public static IntPtr Find(uint pid) {
        Found = IntPtr.Zero;
        Wanted = pid;
        EnumWindows(new EnumProc(Cb), IntPtr.Zero);
        return Found;
    }
    static bool Cb(IntPtr h, IntPtr l) {
        uint owner;
        GetWindowThreadProcessId(h, out owner);
        if (owner != Wanted) { return true; }
        var text = new StringBuilder(256);
        GetWindowTextW(h, text, 256);
        if (text.ToString().StartsWith("DM Ultra")) { Found = h; return false; }
        return true;
    }
}
"@

function Get-MainWindow {
    $process = Get-Process dmultra -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $process) { return [IntPtr]::Zero }
    return [SingleWin]::Find([uint32]$process.Id)
}

function Get-WindowState([IntPtr]$hwnd) {
    if ($hwnd -eq [IntPtr]::Zero) {
        return [pscustomobject]@{ Visible = $false; Iconic = $false; OffScreen = $true
                                  Left = 0; Top = 0; OutOfSight = $false; Window = $false }
    }
    $r = New-Object SingleWin+RECT
    [void][SingleWin]::GetWindowRect($hwnd, [ref]$r)
    $vis = [SingleWin]::IsWindowVisible($hwnd)
    $ico = [SingleWin]::IsIconic($hwnd)
    $off = $r.Left -lt -10000
    return [pscustomobject]@{
        Visible    = $vis
        Iconic     = $ico
        OffScreen  = $off
        Left       = $r.Left
        Top        = $r.Top
        Window     = $true
        OutOfSight = ($ico -or -not $vis -or $off)
    }
}

function Format-WindowState([IntPtr]$hwnd) {
    $s = Get-WindowState $hwnd
    if (-not $s.Window) { return 'no window' }
    return ('visible={0} iconic={1} at=({2},{3}) off-screen={4}' -f `
        $s.Visible, $s.Iconic, $s.Left, $s.Top, $s.OffScreen)
}

# D-007: a hidden main window is a park - off-screen at (-32000,-32000) while it
# stays WS_VISIBLE and non-iconic, because eframe only paints the progress
# popouts from a visible root. A window is also briefly not-visible right after
# it is created (eframe's anti-flash), so one sample proves nothing: require the
# out-of-sight state to hold so only the settled park counts.
function Test-SettledOutOfSight([IntPtr]$hwnd, [int]$holdMs = 1500) {
    $deadline = (Get-Date).AddMilliseconds($holdMs)
    while ((Get-Date) -lt $deadline) {
        if (-not (Get-WindowState $hwnd).OutOfSight) { return $false }
        Start-Sleep -Milliseconds 100
    }
    return $true
}

function Test-SettledOnScreen([IntPtr]$hwnd, [int]$holdMs = 800) {
    $deadline = (Get-Date).AddMilliseconds($holdMs)
    while ((Get-Date) -lt $deadline) {
        $s = Get-WindowState $hwnd
        if (-not ($s.Visible -and -not $s.Iconic -and -not $s.OffScreen)) { return $false }
        Start-Sleep -Milliseconds 100
    }
    return $true
}

function Wait-For([scriptblock]$condition, [int]$seconds = 15) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if (& $condition) { return $true }
        Start-Sleep -Milliseconds 150
    }
    return $false
}

$failed = $false
function Check([bool]$ok, [string]$label, [string]$detail = '') {
    if ($ok) {
        Write-Host "[PASS] $label  $detail"
    } else {
        Write-Host "[FAIL] $label  $detail"
        $script:failed = $true
    }
}

try {
    Remove-Item $showRequest -Force -ErrorAction SilentlyContinue

    Start-Process $exe -ArgumentList '--background' -WorkingDirectory $root
    $found = Wait-For { (Get-MainWindow) -ne [IntPtr]::Zero }
    $parked = $found -and (Test-SettledOutOfSight (Get-MainWindow))
    Check $parked 'background launch parks the main window out of sight' (Format-WindowState (Get-MainWindow))

    Start-Process $exe -WorkingDirectory $root
    # A parked window is still WS_VISIBLE, so "visible" alone says nothing about
    # a reveal: wait until it is genuinely back on screen, then require it to hold.
    $revealed = Wait-For {
        $s = Get-WindowState (Get-MainWindow)
        $s.Visible -and -not $s.Iconic -and -not $s.OffScreen
    }
    $shown = $revealed -and (Test-SettledOnScreen (Get-MainWindow))
    Check $shown 'second launch revealed the running window' (Format-WindowState (Get-MainWindow))

    # The second launch hands its request to the running instance and returns, so
    # wait for it to exit instead of racing its shutdown: the contract is that
    # one process remains, not that one is seen at one arbitrary instant.
    $alone = Wait-For { (@(Get-Process dmultra -ErrorAction SilentlyContinue).Count -eq 1) } 10
    $count = @(Get-Process dmultra -ErrorAction SilentlyContinue).Count
    Check $alone 'second launch did not start a duplicate process' "count=$count"
    Check (-not (Test-Path $showRequest)) 'show request was consumed'
} finally {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    if ($null -ne $backup) {
        [System.IO.File]::WriteAllText($statePath, $backup)
    }
    Remove-Item $showRequest -Force -ErrorAction SilentlyContinue
}

if ($failed) { exit 1 }
