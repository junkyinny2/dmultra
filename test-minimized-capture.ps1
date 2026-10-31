# Minimized-window capture test
#
# Reproduces the report: main window minimized, a browser link arrives ->
# the download must start and its progress window must appear without the
# main window being restored.
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
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int max);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
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
if (-not [Win]::IsIconic($main)) { throw 'main window did not minimize' }
Write-Host 'main window is minimized'

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

$stillMinimized = [Win]::IsIconic($main)

Write-Host ''
Write-Host ("download created:   {0}" -f $ok)
Write-Host ("main still minimized: {0}" -f $stillMinimized)
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
if ($ok -and $stillMinimized) { exit 0 } else { exit 1 }
