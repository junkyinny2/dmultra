# Minimized progress-window repaint test
#
# Regression: eframe renders immediate child viewports (the "Download
# status" window) as part of the *parent* window's frame. It refuses to
# schedule frames for a minimized window, so a browser click captured while
# minimized used to open the progress window at 0% and then freeze there,
# even though the transfer itself kept running and completed.
#
# The test serves a deliberately slow 12 MB file over a local HTTP server,
# minimizes the main window, queues the URL exactly like the native
# messaging host does, waits for the transfer to finish, then screenshots
# the progress window and measures how much of the progress bar is painted.
# A frozen-at-0% window paints none of it.
#
# Usage: .\test-minimized-progress.ps1
#        .\test-minimized-progress.ps1 -Background
#
# -Background starts DM Ultra with --background (the way the browser host and
# the Windows startup entry do) and asserts the main window never appears.

param([switch]$Background)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing

# PowerShell 7 needs explicit reference assemblies when one is passed;
# use the ones shipped with the runtime plus System.Drawing.Common.
$testRefs = @(Get-ChildItem "$PSHOME\ref\*.dll" | ForEach-Object { $_.FullName })
$testRefs += [System.Drawing.Bitmap].Assembly.Location
foreach ($extra in @('System.Private.Windows.Core.dll', 'System.Private.Windows.GdiPlus.dll')) {
    $path = Join-Path $PSHOME $extra
    if (Test-Path $path) { $testRefs += $path }
}

Add-Type @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Net;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public class MinProg {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    delegate bool EnumProc(IntPtr h, IntPtr l);

    static IntPtr Found;
    static uint Pid;
    static string Prefix;

    public static IntPtr Find(uint pid, string prefix) {
        Found = IntPtr.Zero;
        Pid = pid;
        Prefix = prefix;
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
        if (t.StartsWith(Prefix)) { Found = h; return false; }
        return true;
    }

    public static Bitmap Capture(IntPtr h) {
        RECT r;
        GetClientRect(h, out r);
        POINT p = new POINT();
        ClientToScreen(h, ref p);
        int w = Math.Max(1, r.R - r.L);
        int ht = Math.Max(1, r.B - r.T);
        var bmp = new Bitmap(w, ht);
        using (var g = Graphics.FromImage(bmp)) {
            g.CopyFromScreen(p.X, p.Y, 0, 0, new Size(w, ht));
        }
        return bmp;
    }

    // Fraction of the widest row covered by progress-bar palette colors.
    public static double BarCoverage(Bitmap bmp) {
        int w = bmp.Width, h = bmp.Height;
        BitmapData data = bmp.LockBits(new Rectangle(0, 0, w, h), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        byte[] buf = new byte[data.Stride * h];
        Marshal.Copy(data.Scan0, buf, 0, buf.Length);
        bmp.UnlockBits(data);
        int[] pal = new int[] {
            57,133,245,  33,196,93,  230,165,50, 139,92,246,
            244,63,94,   6,182,212,  234,179,8,  236,72,153
        };
        int best = 0;
        for (int y = 0; y < h; y++) {
            int count = 0;
            for (int x = 0; x < w; x++) {
                int i = y * data.Stride + x * 4;
                int b = buf[i], g = buf[i + 1], r = buf[i + 2];
                for (int k = 0; k < pal.Length; k += 3) {
                    if (Math.Abs(r - pal[k]) <= 12 && Math.Abs(g - pal[k + 1]) <= 12 && Math.Abs(b - pal[k + 2]) <= 12) {
                        count++;
                        break;
                    }
                }
            }
            if (count > best) best = count;
        }
        return best / (double)w;
    }
}

public class SlowRangeServer : IDisposable {
    HttpListener listener;
    Thread thread;
    int size;

    public string Url;

    public SlowRangeServer(int port, int bytes) {
        size = bytes;
        listener = new HttpListener();
        listener.Prefixes.Add("http://localhost:" + port + "/");
        listener.Start();
        Url = "http://localhost:" + port + "/minimized-progress-test.bin";
        thread = new Thread(Loop);
        thread.IsBackground = true;
        thread.Start();
    }

    void Loop() {
        while (listener.IsListening) {
            HttpListenerContext ctx;
            try { ctx = listener.GetContext(); }
            catch { break; }
            ThreadPool.QueueUserWorkItem(delegate(object state) { Handle((HttpListenerContext)state); }, ctx);
        }
    }

    void Handle(HttpListenerContext ctx) {
        try {
            long start = 0;
            long end = size - 1;
            string range = ctx.Request.Headers["Range"];
            if (range != null && range.StartsWith("bytes=")) {
                string spec = range.Substring(6);
                int dash = spec.IndexOf('-');
                string first = dash < 0 ? spec : spec.Substring(0, dash);
                string last = dash < 0 ? "" : spec.Substring(dash + 1);
                if (first.Length > 0) start = long.Parse(first);
                end = last.Length > 0 ? Math.Min(size - 1, long.Parse(last)) : size - 1;
                if (start > end) { ctx.Response.StatusCode = 416; ctx.Response.Close(); return; }
                ctx.Response.StatusCode = 206;
                ctx.Response.AddHeader("Content-Range", "bytes " + start + "-" + end + "/" + size);
            } else {
                ctx.Response.StatusCode = 200;
            }
            ctx.Response.AddHeader("Accept-Ranges", "bytes");
            ctx.Response.ContentType = "application/octet-stream";
            long len = end - start + 1;
            ctx.Response.ContentLength64 = len;
            byte[] chunk = new byte[32768];
            long pos = start;
            while (pos <= end) {
                int n = (int)Math.Min(chunk.Length, end - pos + 1);
                ctx.Response.OutputStream.Write(chunk, 0, n);
                ctx.Response.OutputStream.Flush();
                pos += n;
                Thread.Sleep(25);
            }
            ctx.Response.Close();
        } catch {
            try { ctx.Response.Abort(); } catch { }
        }
    }

    public void Dispose() {
        try { listener.Stop(); listener.Close(); } catch { }
    }
}
"@ -ReferencedAssemblies $testRefs

$statePath = Join-Path $env:APPDATA 'dmultra\state.json'
$pendingDir = Join-Path $env:APPDATA 'dmultra\pending'
$port = 47831
$url = "http://localhost:$port/minimized-progress-test.bin"
$server = $null

function Show-Check($label, $ok, $detail) {
    $mark = if ($ok) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1}  {2}" -f $mark, $label, $detail)
    return $ok
}

$results = @()
$progressWindow = [IntPtr]::Zero
$mainWindow = [IntPtr]::Zero
$savePath = $null
$proc = $null

try {
    # --------------------------------------------------------------- setup
    if (Get-Process dmultra -ErrorAction SilentlyContinue) {
        throw 'dmultra is running; stop it before running this test'
    }
    New-Item -ItemType Directory -Force -Path $pendingDir | Out-Null
    Get-ChildItem $pendingDir -Filter '*.json' -ErrorAction SilentlyContinue | Remove-Item -Force

    $server = [SlowRangeServer]::new($port, 12582912)
    $startArgs = @()
    if ($Background) { $startArgs += '--background' }
    Start-Process 'C:\dmultra\dmultra.exe' -ArgumentList $startArgs -WorkingDirectory 'C:\dmultra'
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 300
        $proc = Get-Process dmultra -ErrorAction SilentlyContinue
        if (-not $proc) { continue }
        $mainWindow = [MinProg]::Find([uint32]$proc[0].Id, 'DM Ultra')
        if ($mainWindow -ne [IntPtr]::Zero) { break }
    }
    if ($mainWindow -eq [IntPtr]::Zero) { throw 'main window did not appear' }
    Start-Sleep -Milliseconds 900

    if ($Background) {
        $results += Show-Check 'background launch keeps the main window hidden' `
            (-not [MinProg]::IsWindowVisible($mainWindow)) ''
    } else {
        # Minimized and parked the way a browser click typically finds it.
        [MinProg]::ShowWindow($mainWindow, 6) | Out-Null   # SW_MINIMIZE
        Start-Sleep -Milliseconds 600
        $results += Show-Check 'main window minimized' ([MinProg]::IsIconic($mainWindow)) ''
    }

    # Queue the link exactly like dmultra-nmh does.
    $stamp = [DateTime]::UtcNow.Ticks
    $payload = '{"url":"' + $url + '","referer":""}'
    [System.IO.File]::WriteAllText((Join-Path $pendingDir "$stamp.json"), $payload, (New-Object System.Text.UTF8Encoding($false)))

    # ------------------------------------------------- wait for progress box
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 300
        $progressWindow = [MinProg]::Find([uint32]$proc[0].Id, 'Download status - ')
        if ($progressWindow -ne [IntPtr]::Zero) { break }
    }
    $results += Show-Check 'progress window opened while minimized' ($progressWindow -ne [IntPtr]::Zero) ("hwnd=$progressWindow")

    # ------------------------------------------------- wait for completion
    $done = $false
    $deadline = (Get-Date).AddSeconds(45)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 400
        if (-not (Test-Path $statePath)) { continue }
        try {
            $state = Get-Content $statePath -Raw | ConvertFrom-Json
        } catch { continue }
        $rec = $state.downloads | Where-Object { $_.url -eq $url } | Select-Object -First 1
        if ($rec) {
            $savePath = $rec.save_path
            if ($rec.status -eq 'complete') { $done = $true; break }
        }
    }
    $results += Show-Check 'download completed in the background' $done ("status={0}" -f ($rec.status))

    # The UI must have caught up even though the main window never appeared.
    Start-Sleep -Milliseconds 1500
    if ($Background) {
        $results += Show-Check 'main window stayed hidden' `
            (-not [MinProg]::IsWindowVisible($mainWindow)) ''
    } else {
        $results += Show-Check 'main window still minimized' ([MinProg]::IsIconic($mainWindow)) ''
    }

    $coverage = -1.0
    if ($progressWindow -ne [IntPtr]::Zero) {
        $bmp = [MinProg]::Capture($progressWindow)
        try {
            $coverage = [MinProg]::BarCoverage($bmp)
        } finally {
            $bmp.Dispose()
        }
    }
    $results += Show-Check 'progress bar reached 100% while minimized' ($coverage -ge 0.80) ("filled width={0:P0}" -f $coverage)

    if ($coverage -ge 0 -and $coverage -lt 0.80) {
        Write-Host 'Screenshot the progress window shows a stale first frame (the 0% regression).'
    }

    # The user-visible symptom of the self-clocking regression: the status
    # window is painted but its title-bar X / Close button / drag are dead
    # because the busy-waker flooded the Win32 queue with synthetic resizes.
    # A posted WM_CLOSE must therefore reach the app (and hide the window)
    # promptly. Before the fix this took minutes.
    if ([MinProg]::IsWindow($progressWindow)) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        [MinProg]::PostMessageW($progressWindow, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
        while ($sw.ElapsedMilliseconds -lt 5000 -and [MinProg]::IsWindowVisible($progressWindow)) {
            Start-Sleep -Milliseconds 25
        }
        $sw.Stop()
        $results += Show-Check 'status window responds to close (input not starved)' `
            ($sw.ElapsedMilliseconds -lt 2000) ("close latency={0} ms" -f $sw.ElapsedMilliseconds)
    }
}
finally {
    if ($progressWindow -ne [IntPtr]::Zero) {
        [MinProg]::PostMessageW($progressWindow, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    }
    if ($mainWindow -ne [IntPtr]::Zero) {
        [MinProg]::ShowWindow($mainWindow, 9) | Out-Null   # SW_RESTORE
    }
    Start-Sleep -Milliseconds 500
    if ($savePath -and (Test-Path -LiteralPath $savePath)) {
        Remove-Item -LiteralPath $savePath -Force -ErrorAction SilentlyContinue
    }
    Get-ChildItem $pendingDir -Filter '*.json' -ErrorAction SilentlyContinue | Remove-Item -Force
    Get-Process dmultra -ErrorAction SilentlyContinue | Stop-Process -Force
    if ($server) { $server.Dispose() }
}

Write-Host ''
if ($results -contains $false) { exit 1 } else { exit 0 }
