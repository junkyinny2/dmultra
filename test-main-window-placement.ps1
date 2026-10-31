# Main-window placement test
#
# 1. No saved position -> the window opens centred on the screen work area.
# 2. "Remember window positions" on -> the last position/size is restored.
# 3. "Remember window positions" off -> the window opens centred again.
#
# Usage: .\test-main-window-placement.ps1

$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class MW {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SystemParametersInfoW(uint a, uint p, out RECT r, uint w);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    delegate bool EnumProc(IntPtr h, IntPtr l);

    static IntPtr Found;
    static uint TargetPid;

    public static IntPtr FindMain(uint pid) {
        Found = IntPtr.Zero;
        TargetPid = pid;
        EnumWindows(new EnumProc(Cb), IntPtr.Zero);
        return Found;
    }
    static bool Cb(IntPtr h, IntPtr l) {
        uint owner;
        GetWindowThreadProcessId(h, out owner);
        if (owner != TargetPid) return true;
        var sb = new StringBuilder(256);
        GetWindowTextW(h, sb, 256);
        string t = sb.ToString();
        if (t == "DM Ultra" || t.StartsWith("DM Ultra v")) { Found = h; return false; }
        return true;
    }
    public static RECT WorkArea() {
        RECT r;
        SystemParametersInfoW(0x0030, 0, out r, 0);
        return r;
    }
}
"@

$statePath = Join-Path $env:APPDATA 'dmultra\state.json'

function Stop-App {
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 900
}

function Start-App {
    Start-Process 'C:\dmultra\dmultra.exe' -WorkingDirectory 'C:\dmultra'
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 300
        $proc = Get-Process dmultra -ErrorAction SilentlyContinue
        if (-not $proc) { continue }
        $h = [MW]::FindMain([uint32]$proc[0].Id)
        if ($h -ne [IntPtr]::Zero) {
            Start-Sleep -Milliseconds 800
            return $h
        }
    }
    throw 'window did not appear'
}

function Get-Rect($h) {
    $r = New-Object MW+RECT
    [MW]::GetWindowRect($h, [ref]$r) | Out-Null
    return $r
}

function Set-State([scriptblock]$mutate) {
    $s = Get-Content $statePath -Raw | ConvertFrom-Json
    & $mutate $s.settings
    ($s | ConvertTo-Json -Depth 100) | Set-Content $statePath -Encoding UTF8
}

function Show-Check($label, $ok, $detail) {
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1}  {2}" -f $mark, $label, $detail)
    return $ok
}

$results = @()

# ---------------------------------------------------------------- 1. centred
Stop-App
Set-State { param($s) $s.remember_window_pos = $true; $s.window_pos = $null; $s.window_size = $null }
$h = Start-App
$r = Get-Rect $h
$wa = [MW]::WorkArea()
$expectedX = $wa.L + [int](($wa.R - $wa.L - ($r.R - $r.L)) / 2)
$expectedY = $wa.T + [int](($wa.B - $wa.T - ($r.B - $r.T)) / 2)
$dx = [Math]::Abs($r.L - $expectedX)
$dy = [Math]::Abs($r.T - $expectedY)
$results += Show-Check 'first run is centred' ($dx -le 12 -and $dy -le 12) ("pos=({0},{1}) expected=({2},{3})" -f $r.L, $r.T, $expectedX, $expectedY)

# ------------------------------------------------------------- 2. remembered
[MW]::SetWindowPos($h, [IntPtr]::Zero, 220, 160, 0, 0, 0x0001 -bor 0x0004) | Out-Null
Start-Sleep -Milliseconds 1800
$saved = (Get-Content $statePath -Raw | ConvertFrom-Json).settings
$savedPos = $saved.window_pos
$results += Show-Check 'position was stored' ($null -ne $savedPos) ("window_pos={0}" -f ($savedPos -join ','))

Stop-App
$h = Start-App
$r = Get-Rect $h
$results += Show-Check 'saved position restored' ([Math]::Abs($r.L - 220) -le 12 -and [Math]::Abs($r.T - 160) -le 12) ("pos=({0},{1}) expected=(220,160)" -f $r.L, $r.T)

# ----------------------------------------------------------------- 3. opted out
[MW]::SetWindowPos($h, [IntPtr]::Zero, 600, 400, 0, 0, 0x0001 -bor 0x0004) | Out-Null
Start-Sleep -Milliseconds 1500
Stop-App
Set-State { param($s) $s.remember_window_pos = $false }
$h = Start-App
$r = Get-Rect $h
$expectedX = $wa.L + [int](($wa.R - $wa.L - ($r.R - $r.L)) / 2)
$expectedY = $wa.T + [int](($wa.B - $wa.T - ($r.B - $r.T)) / 2)
$dx = [Math]::Abs($r.L - $expectedX)
$dy = [Math]::Abs($r.T - $expectedY)
$results += Show-Check 'option off -> centred again' ($dx -le 12 -and $dy -le 12) ("pos=({0},{1}) expected=({2},{3})" -f $r.L, $r.T, $expectedX, $expectedY)
$stored = (Get-Content $statePath -Raw | ConvertFrom-Json).settings
$results += Show-Check 'option off -> stored position cleared' ($null -eq $stored.window_pos) ("window_pos={0}" -f ($stored.window_pos -join ','))

# ------------------------------------------- 4. re-enabling records again
Set-State { param($s) $s.remember_window_pos = $true }
Stop-App
$h = Start-App
Start-Sleep -Milliseconds 1200
$again = (Get-Content $statePath -Raw | ConvertFrom-Json).settings
$results += Show-Check 're-enabled -> position recorded' ($null -ne $again.window_pos) ("window_pos={0}" -f ($again.window_pos -join ','))
$results += Show-Check 're-enabled -> size recorded' ($null -ne $again.window_size) ("window_size={0}" -f ($again.window_size -join ','))

Write-Host ''
if ($results -contains $false) { exit 1 } else { exit 0 }
