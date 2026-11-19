#Requires -Version 7.0
<#
.SYNOPSIS
  Measure how expensive a DM Ultra GUI frame is while the window is being
  dragged, compared with a normal frame.
.DESCRIPTION
  Windows performs a window drag inside a modal message loop that runs on the
  app's GUI thread, so a drag is only smooth while a single frame is cheap.
  This script launches the deployed binary with the frame profiler enabled
  (DMULTRA_PROFILE=1), drags the main window's title bar with real mouse
  input, and prints the measured frame cost split by mode:

    idle - no transfers, no child window
    ui   - transfers running and/or a progress/dialog window open
    drag - inside the move loop (WM_ENTERSIZEMOVE .. WM_EXITSIZEMOVE)

  With -ChildUrl a download is queued first through the pending queue the
  browser extension uses, so the drag is also measured with a progress window
  open - the situation the user reports.
.EXAMPLE
  .\test-window-drag-profile.ps1
  .\test-window-drag-profile.ps1 -ChildUrl https://example.com/dragtest.zip
#>
param(
    [string] $Exe = 'C:\dmultra\dmultra.exe',
    [string] $ChildUrl = '',
    [int] $Steps = 45,
    [int] $StepDelayMs = 15,
    [switch] $LeaveRunning
)

$ErrorActionPreference = 'Stop'

$version = (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'VERSION') -Raw).Trim()
$title = "DM Ultra v$version"
$logs = Join-Path $env:APPDATA 'dmultra\logs'
$profile = Join-Path $logs 'profile.log'
$pending = Join-Path $env:APPDATA 'dmultra\pending'

Add-Type -Namespace DmDrag -Name Win -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern IntPtr FindWindowW(System.IntPtr cls, [System.Runtime.InteropServices.MarshalAs(System.Runtime.InteropServices.UnmanagedType.LPWStr)] string title);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT pt);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp);
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct RECT { public int Left, Top, Right, Bottom; }
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct POINT { public int X, Y; }
'@

function Get-MainWindow {
    for ($i = 0; $i -lt 100; $i++) {
        $hwnd = [DmDrag.Win]::FindWindowW([IntPtr]::Zero, $title)
        if ($hwnd -ne 0 -and [DmDrag.Win]::IsWindowVisible($hwnd) -and -not [DmDrag.Win]::IsIconic($hwnd)) {
            return $hwnd
        }
        Start-Sleep -Milliseconds 100
    }
    throw "could not find a visible '$title' window"
}

function Get-FrameStats([string] $path) {
    if (-not (Test-Path -LiteralPath $path)) {
        Write-Host 'no profile.log written (did DMULTRA_PROFILE reach the process?)'
        return
    }
    $lines = Get-Content -LiteralPath $path | Where-Object { $_ -match 'FRAME' }
    foreach ($mode in 'idle', 'ui', 'drag') {
        $values = @($lines | Where-Object { $_ -match " FRAME $mode " } | ForEach-Object {
            [double] (($_ -split '\s+')[3])
        })
        if ($values.Count -eq 0) {
            Write-Host ("{0,-5}: no frames" -f $mode)
            continue
        }
        $sorted = $values | Sort-Object
        $median = $sorted[[int] ($sorted.Count / 2)]
        $stats = $values | Measure-Object -Average -Maximum -Minimum
        Write-Host ("{0,-5}: n={1,-5} avg={2:N1}ms median={3:N1}ms min={4:N1}ms max={5:N1}ms" -f $mode, $values.Count, $stats.Average, $median, $stats.Minimum, $stats.Maximum)
    }
}

if (-not (Test-Path -LiteralPath $Exe)) { throw "binary not found: $Exe" }

if (Get-Process | Where-Object { $_.ProcessName -eq 'dmultra' }) {
    throw 'a DM Ultra instance is already running; close it (tray -> Exit) first'
}

New-Item -ItemType Directory -Force -Path $logs | Out-Null
Remove-Item -LiteralPath $profile -Force -ErrorAction SilentlyContinue

if ($ChildUrl) {
    New-Item -ItemType Directory -Force -Path $pending | Out-Null
    $stamp = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() * 1000000
    $payload = @{ url = $ChildUrl; referer = '' } | ConvertTo-Json -Compress
    [IO.File]::WriteAllText((Join-Path $pending "$stamp.json"), $payload)
    Write-Host "queued child download: $ChildUrl"
}

$env:DMULTRA_PROFILE = '1'
$proc = Start-Process -FilePath $Exe -PassThru
Write-Host "started pid $($proc.Id)"
Start-Sleep -Seconds 3

$hwnd = Get-MainWindow
$rect = New-Object DmDrag.Win+RECT
if (-not [DmDrag.Win]::GetWindowRect($hwnd, [ref] $rect)) { throw 'GetWindowRect failed' }
$originX = $rect.Left
$originY = $rect.Top
Write-Host ("main window at ({0},{1}) {2}x{3}" -f $originX, $originY, ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top))

# Let the idle loop settle so normal frames are measured before the drag too.
Start-Sleep -Seconds 2

# ---------------------------------------------------------------------------
# Grab the caption with real mouse input: Windows then runs its modal move
# loop on the app's GUI thread, exactly like a human dragging the title bar.
#
# The grab point is found by asking the window itself (WM_NCHITTEST) rather
# than guessing an offset from GetWindowRect. GetWindowRect includes the
# invisible DWM resize border, and the caption height moves with the DPI
# scale and the theme, so a guessed offset can silently land in the client
# area - the drag then does nothing and the run reports no drag frames at all.
# ---------------------------------------------------------------------------
function Get-HitTest([IntPtr] $h, [int] $x, [int] $y) {
    $lparam = ((($y -band 0xFFFF) -shl 16) -bor ($x -band 0xFFFF))
    [int] [DmDrag.Win]::SendMessageW($h, 0x0084, [IntPtr]::Zero, [IntPtr] ([long] $lparam))
}

function Get-RootUnder([int] $x, [int] $y) {
    $pt = New-Object DmDrag.Win+POINT
    $pt.X = $x
    $pt.Y = $y
    # GA_ROOT: the top-level window the point actually belongs to.
    [DmDrag.Win]::GetAncestor([DmDrag.Win]::WindowFromPoint($pt), 2)
}

$grabX = $rect.Left + [int] (($rect.Right - $rect.Left) / 2)

# Bring our window above everything else before probing or clicking. Another
# application's window sitting over the caption takes the click instead - Excel
# did exactly that on this machine, and the drag silently did nothing.
[void] [DmDrag.Win]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 400

$grabY = $null
$method = 'physical'
Write-Host 'caption probe (HTCLIENT=1, HTCAPTION=2, HTTOP=12):'
foreach ($dy in 4, 8, 12, 16, 20, 24, 28, 32) {
    $probeY = $rect.Top + $dy
    $hit = Get-HitTest $hwnd $grabX $probeY
    $root = Get-RootUnder $grabX $probeY
    Write-Host ("  ({0},{1}): hit-test={2} root-under-point={3} (want {4})" -f $grabX, $probeY, $hit, $root, $hwnd)
    if ($hit -eq 2 -and $root -eq $hwnd) { $grabY = $probeY; break }
}
if ($null -eq $grabY) {
    # The window is on top of nothing we control: fall back to posting the
    # caption press straight to our window. The modal move loop it starts
    # captures the mouse, so the Z-order under the cursor stops mattering.
    $grabY = $rect.Top + 20
    $method = 'posted WM_NCLBUTTONDOWN'
    Write-Host ("our window is not topmost at the caption; using {0} at ({1},{2})" -f $method, $grabX, $grabY)
} else {
    Write-Host ("grabbing the caption at ({0},{1}) with a physical click" -f $grabX, $grabY)
}

$lparam = ((($grabY -band 0xFFFF) -shl 16) -bor ($grabX -band 0xFFFF))
[void] [DmDrag.Win]::SetCursorPos($grabX, $grabY)
Start-Sleep -Milliseconds 250
if ($method -eq 'physical') {
    [DmDrag.Win]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
} else {
    [void] [DmDrag.Win]::PostMessageW($hwnd, 0x00A1, [IntPtr] 2, [IntPtr] ([long] $lparam))   # WM_NCLBUTTONDOWN, HTCAPTION
}
Start-Sleep -Milliseconds 250

for ($i = 1; $i -le $Steps; $i++) {
    [void] [DmDrag.Win]::SetCursorPos(($grabX + $i * 12), ($grabY + $i * 4))
    Start-Sleep -Milliseconds $StepDelayMs
}

[DmDrag.Win]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
Start-Sleep -Seconds 2

Write-Host ''
Write-Host 'frame cost by mode:'
Get-FrameStats $profile

$endX = $originX
$endY = $originY
if ([DmDrag.Win]::GetWindowRect($hwnd, [ref] $rect)) {
    $endX = $rect.Left
    $endY = $rect.Top
}
Write-Host ("window ended at ({0},{1}) (started at ({2},{3}))" -f $endX, $endY, $originX, $originY)
if ($endX -eq $originX -and $endY -eq $originY) {
    Write-Host 'WARNING: the window never moved - the click did not engage the modal move loop'
}

if (-not $LeaveRunning) {
    if (($endX -ne $originX -or $endY -ne $originY)) {
        [void] [DmDrag.Win]::SetWindowPos($hwnd, [IntPtr]::Zero, $originX, $originY, 0, 0, 0x15)
        Start-Sleep -Milliseconds 400
    }
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Write-Host 'stopped the app'
}

