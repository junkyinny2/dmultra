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
    $hidden = Wait-For {
        $hwnd = Get-MainWindow
        $hwnd -ne [IntPtr]::Zero -and
            -not [SingleWin]::IsWindowVisible($hwnd) -and
            -not [SingleWin]::IsIconic($hwnd)
    }
    Check $hidden 'background launch created a hidden main window'

    Start-Process $exe -WorkingDirectory $root
    $shown = Wait-For {
        $hwnd = Get-MainWindow
        $hwnd -ne [IntPtr]::Zero -and
            [SingleWin]::IsWindowVisible($hwnd) -and
            -not [SingleWin]::IsIconic($hwnd)
    }
    Check $shown 'second launch revealed the running window'

    $count = @(Get-Process dmultra -ErrorAction SilentlyContinue).Count
    Check ($count -eq 1) 'second launch did not start a duplicate process' "count=$count"
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
