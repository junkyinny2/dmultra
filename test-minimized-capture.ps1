# Minimized-window capture test
#
# Reproduces the report: main window minimized, a browser link arrives ->
# the download must start and its progress window must appear without the
# main window being restored.
#
# "Minimized" means out of sight, not iconic: since v0.073 (D-007) the app parks
# the main window off-screen and keeps it WS_VISIBLE in the tray modes (the
# default is "taskbar + tray"), because eframe paints the progress popout from
# the root frame. Throwing on IsIconic was the stale assumption that made this
# script abort at its minimize step against v0.073-v0.076.
#
# Usage: .\test-minimized-capture.ps1

$ErrorActionPreference = 'Stop'

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win {
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int max);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int idx);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    const int GWL_EXSTYLE = -20;
    const int WS_EX_TOOLWINDOW = 0x80;
    delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);

    static IntPtr Found;
    static uint TargetPid;

    public static IntPtr FindMainWindow(uint pid) {
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
        if (t == "DM Ultra" || t.StartsWith("DM Ultra v")) {
            Found = h;
            return false;
        }
        return true;
    }

    // The park position from browser.rs park(). Note Windows also reports
    // (-32000,-32000) for any iconic window, so this is only ever used inside
    // IsOutOfSight as one of the three ways a window can be off the desktop.
    public static bool IsParkedOffScreen(IntPtr h) {
        RECT r;
        if (!GetWindowRect(h, out r)) return false;
        return r.Left <= -10000 || r.Right <= -10000;
    }

    public static bool IsOutOfSight(IntPtr h) {
        if (!IsWindow(h)) return true;
        return !IsWindowVisible(h) || IsIconic(h) || IsParkedOffScreen(h);
    }

    public static bool HasTaskbarButton(IntPtr h) {
        if (!IsWindow(h)) return false;
        if (IsIconic(h)) return true;
        if (!IsWindowVisible(h)) return false;
        long ex = GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64();
        return (ex & WS_EX_TOOLWINDOW) == 0;
    }

    public static string Describe(IntPtr h) {
        RECT r;
        string pos = GetWindowRect(h, out r)
            ? string.Format("({0},{1})", r.Left, r.Top) : "(no rect)";
        return string.Format("iconic={0} visible={1} at={2} taskbar={3}",
            IsIconic(h), IsWindowVisible(h), pos, HasTaskbarButton(h));
    }
}
"@

$pending = Join-Path $env:APPDATA 'dmultra\pending'
$log = Join-Path $env:APPDATA 'dmultra\logs\dmultra.log'
$stateFile = Join-Path $env:APPDATA 'dmultra\state.json'

function Get-MainWindow {
    $procId = [uint32](Get-Process dmultra -ErrorAction Stop)[0].Id
    return [Win]::FindMainWindow($procId)
}

if (-not (Get-Process dmultra -ErrorAction SilentlyContinue)) {
    Start-Process 'C:\dmultra\dmultra.exe'
    Start-Sleep -Seconds 5
}

$main = Get-MainWindow
if ($main -eq [IntPtr]::Zero) { throw 'DM Ultra main window not found' }

# Record the download count before the test.
$before = (Get-Content $stateFile -Raw | ConvertFrom-Json).downloads.Count
$logStart = (Get-Item $log).Length

[Win]::ShowWindow($main, 6) | Out-Null   # SW_MINIMIZE
Start-Sleep -Milliseconds 800
# Accept either real minimize or the D-007 park - that is the state a browser
# capture actually arrives in under the default "taskbar + tray" mode.
if (-not [Win]::IsOutOfSight($main)) {
    throw ('main window did not leave the screen: ' + [Win]::Describe($main))
}
Write-Host ('main window is out of sight: ' + [Win]::Describe($main))

# Emulate a browser capture exactly like the native messaging host does.
$url = 'https://www.example.com/'
$payload = "{`"url`":`"$url`",`"referer`":`"https://www.example.com/`"}"
$temp = Join-Path $pending 'toast.tmp'
Set-Content -Path $temp -Value $payload -NoNewline -Encoding ascii
Move-Item -Path $temp -Destination (Join-Path $pending '9999999999999999999.json') -Force
Write-Host 'queued a browser link'

$ok = $false
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 500
    $after = (Get-Content $stateFile -Raw | ConvertFrom-Json).downloads.Count
    if ($after -gt $before) { $ok = $true; break }
}

$stillHidden = [Win]::IsOutOfSight($main)

Write-Host ''
Write-Host ("download created:   {0}" -f $ok)
Write-Host ("main still out of sight: {0}  {1}" -f $stillHidden, [Win]::Describe($main))
Write-Host ''
Write-Host '--- new log lines ---'
$fs = [System.IO.File]::Open($log, 'Open', 'Read', 'ReadWrite')
$fs.Position = $logStart
$reader = New-Object System.IO.StreamReader($fs)
$text = $reader.ReadToEnd()
$reader.Close()
Write-Host $text

# Leave the main window as it was found.
[Win]::ShowWindow($main, 9) | Out-Null   # SW_RESTORE
if ($ok -and $stillHidden) { exit 0 } else { exit 1 }
