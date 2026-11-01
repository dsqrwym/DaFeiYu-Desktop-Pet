# Regenerate docs/images/demo.gif from a real on-screen run of the pet.
#
# The gif answers exactly one question: what does the pet look like on a desktop.
# Two details carry all the weight, and both were learned the hard way:
#
#   1. Capture with ddagrab (Desktop Duplication), never gdigrab. gdigrab BitBlts
#      the screen DC, and a layered window's fully transparent pixels come back as
#      solid black there. The H.264 pack cuts soft smoke away with a 1-bit mask, so
#      every puff turns into a black blob that nobody can see on the real screen --
#      the recording lies, the desktop does not.
#   2. Build the palette with max_colors=256:stats_mode=full and dither=sierra2_4a.
#      The previous recipe (160 colours, stats_mode=diff, dither=bayer:bayer_scale=4)
#      quantised dark smoke into solid black blocks: bayer_scale spreads the error in
#      coarse tiles, and a small "diff" palette has no entry for dark navy.
#
# Step 5 counts near-black pixels per frame, so a regression cannot slip back in
# quietly: a clean gif stays in the dozens (the character's own outlines), the two
# bugs above pushed it into the thousands.
#
# The pet holds a single-instance lock, so close the running one first or pass
# -StopRunningPet. Recording covers the screen with a solid-colour stage window.
#
# ASCII only: Windows PowerShell 5.1 reads BOM-less files as ANSI.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tools/make-demo-gif.ps1
#   powershell -ExecutionPolicy Bypass -File tools/make-demo-gif.ps1 -Only fireworks
#   powershell -ExecutionPolicy Bypass -File tools/make-demo-gif.ps1 -AnimDir assets/h264
#   powershell -ExecutionPolicy Bypass -File tools/make-demo-gif.ps1 -StopRunningPet
param(
    [string]$AnimDir = 'assets/anim',
    [string]$Out = 'docs\images\demo.gif',
    [string]$Only = '',
    [int]$Seconds = 5,
    [int]$GifFps = 15,
    [int]$GifWidth = 360,
    [switch]$StopRunningPet
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root

# id + the window cut out of each 5s take; ss/dur were picked by looking at frame
# sheets, so that every take shows the part of the clip worth showing.
$takes = @(
    @{ id = 'idle';           ss = 0.50; dur = 1.6 },
    @{ id = 'click_happy';    ss = 0.30; dur = 1.7 },
    @{ id = 'eat_watermelon'; ss = 0.80; dur = 1.6 },
    @{ id = 'animals';        ss = 1.00; dur = 1.7 },
    @{ id = 'fireworks';      ss = 1.00; dur = 1.6 },
    @{ id = 'bubbles';        ss = 2.20; dur = 1.6 },
    @{ id = 'work_busy';      ss = 0.80; dur = 1.6 },
    @{ id = 'sleep';          ss = 0.80; dur = 1.7 }
)

$work = Join-Path $root 'target\demo-gif'
$clips = Join-Path $work 'clips'
$audit = Join-Path $work 'audit'

Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;
public class DemoStage {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr CreateWindowExW(uint ex, string cls, string title, uint style, int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern ushort RegisterClassW(ref WNDCLASS wc);
  [DllImport("user32.dll")] public static extern IntPtr DefWindowProcW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetMessageW(out MSG m, IntPtr h, uint a, uint b);
  [DllImport("user32.dll")] public static extern bool TranslateMessage(ref MSG m);
  [DllImport("user32.dll")] public static extern IntPtr DispatchMessageW(ref MSG m);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool DestroyWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern void PostQuitMessage(int code);
  [DllImport("gdi32.dll")] public static extern IntPtr CreateSolidBrush(uint color);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam, lParam; public uint time; public int x, y; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct WNDCLASS {
    public uint style; public IntPtr lpfnWndProc; public int cbClsExtra; public int cbWndExtra;
    public IntPtr hInstance; public IntPtr hIcon; public IntPtr hCursor; public IntPtr hbrBackground;
    public string lpszMenuName; public string lpszClassName;
  }
  public delegate IntPtr WndProcD(IntPtr h, uint m, IntPtr w, IntPtr l);
  static IntPtr stage = IntPtr.Zero;
  static WndProcD proc;
  static IntPtr OnMsg(IntPtr h, uint m, IntPtr w, IntPtr l) {
    if (m == 0x0010) { DestroyWindow(h); return IntPtr.Zero; }
    if (m == 0x0002) { PostQuitMessage(0); return IntPtr.Zero; }
    return DefWindowProcW(h, m, w, l);
  }
  // A borderless fullscreen window: the plain backdrop that keeps desktop clutter
  // out of the frame. Runs on its own message loop so the caller can keep working.
  public static void Start(uint bgr) {
    var th = new Thread(delegate() {
      proc = new WndProcD(OnMsg);
      WNDCLASS wc = new WNDCLASS();
      wc.lpfnWndProc = Marshal.GetFunctionPointerForDelegate(proc);
      wc.hbrBackground = CreateSolidBrush(bgr);
      wc.lpszClassName = "FeiYuDemoStage";
      RegisterClassW(ref wc);
      int sw = GetSystemMetrics(0), sh = GetSystemMetrics(1);
      stage = CreateWindowExW(0, "FeiYuDemoStage", "stage", 0x80000000u, 0, 0, sw, sh, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
      ShowWindow(stage, 8);
      SetWindowPos(stage, (IntPtr)(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);
      MSG msg;
      while (GetMessageW(out msg, IntPtr.Zero, 0, 0) > 0) { TranslateMessage(ref msg); DispatchMessageW(ref msg); }
      stage = IntPtr.Zero;
    });
    th.IsBackground = true;
    th.Start();
  }
  public static void Stop() { if (stage != IntPtr.Zero) PostMessageW(stage, 0x0010, IntPtr.Zero, IntPtr.Zero); }
  public static int[] Screen() { return new int[]{ GetSystemMetrics(0), GetSystemMetrics(1) }; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  // By process id: the user may have their own pet running, so the class name alone
  // is ambiguous, and the recording must only ever contain the instance we started.
  public static IntPtr FindPid(int pid, string cls) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      var sb = new StringBuilder(64);
      int n = GetClassName(h, sb, 64);
      uint owner; GetWindowThreadProcessId(h, out owner);
      if (n > 0 && sb.ToString() == cls && owner == (uint)pid) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static IntPtr[] HideOthers(int pid, string cls) {
    var list = new System.Collections.Generic.List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      var sb = new StringBuilder(64);
      int n = GetClassName(h, sb, 64);
      uint owner; GetWindowThreadProcessId(h, out owner);
      if (n > 0 && sb.ToString() == cls && owner != (uint)pid && IsWindowVisible(h)) {
        ShowWindow(h, 0);
        list.Add(h);
      }
      return true;
    }, IntPtr.Zero);
    return list.ToArray();
  }
  public static void Restore(IntPtr[] hs) { foreach (var h in hs) { if (h != IntPtr.Zero) ShowWindow(h, 8); } }
  public static int[] Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return new int[]{ r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top }; }
}
'@
[void][DemoStage]::SetProcessDPIAware()

function Measure-DarkestBlob([string]$dir, [int]$ceiling) {
    # Largest run of near-black pixels in any frame. The stage is a solid colour, so
    # anything this dark is either the character's own outline or the bug we are
    # looking for.
    Add-Type -AssemblyName System.Drawing
    $worst = 0
    foreach ($f in (Get-ChildItem $dir -Filter '*.png' | Sort-Object Name)) {
        $bmp = New-Object System.Drawing.Bitmap $f.FullName
        $rect = New-Object System.Drawing.Rectangle 0, 0, $bmp.Width, $bmp.Height
        $fmt = [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
        $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, $fmt)
        $bytes = New-Object byte[] ($data.Stride * $data.Height)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
        $bmp.UnlockBits($data)
        $bmp.Dispose()
        $n = 0
        for ($i = 0; $i -lt $bytes.Length; $i += 4) {
            if ($bytes[$i] -le $ceiling -and $bytes[$i + 1] -le $ceiling -and $bytes[$i + 2] -le $ceiling) { $n++ }
        }
        if ($n -gt $worst) { $worst = $n }
    }
    return $worst
}

$relaunched = $false
try {
    $ffmpeg = (where.exe ffmpeg 2>$null | Select-Object -First 1)
    $ffprobe = (where.exe ffprobe 2>$null | Select-Object -First 1)
    if (-not $ffmpeg) { throw 'ffmpeg not on PATH' }
    if (-not $ffprobe) { throw 'ffprobe not on PATH' }

    $wanted = if ($Only) { @($Only -split ',') } else { @($takes | ForEach-Object { $_.id }) }
    $selected = @($takes | Where-Object { $wanted -contains $_.id })
    if ($selected.Count -eq 0) {
        throw "no such clip in the take list: $Only (known: $(@($takes | ForEach-Object { $_.id }) -join ', '))"
    }

    Write-Host '[1/5] cargo build --release -p feiyu-app'
    cargo build --release -p feiyu-app
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    # -Encoding UTF8 is not decoration: the manifest carries Chinese clip names and
    # Windows PowerShell 5.1 would otherwise read them as ANSI and break the JSON.
    $manifest = Get-Content (Join-Path $root (Join-Path $AnimDir 'manifest.json')) -Encoding UTF8 -Raw | ConvertFrom-Json
    $canvasW = [int]$manifest.canvas.width
    if (-not $canvasW) { throw "$AnimDir/manifest.json has no canvas width" }
    $exe = Join-Path $root 'target\release\feiyu-cli.exe'

    $running = @(Get-Process feiyu, feiyu-cli -ErrorAction SilentlyContinue)
    if ($running.Count -gt 0) {
        if (-not $StopRunningPet) {
            throw "pet already running (pid $(($running | ForEach-Object { $_.Id }) -join ', ')); the single-instance lock would make the recording instance exit. Close it or pass -StopRunningPet."
        }
        Write-Host '[1/5] stopping the running pet (restarted at the end)'
        $petPath = ($running | Where-Object { $_.Path } | Select-Object -First 1).Path
        $running | Stop-Process -Force
        Start-Sleep -Seconds 1
    }

    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $clips | Out-Null
    New-Item -ItemType Directory -Force $audit | Out-Null

    Write-Host "[2/5] record $($selected.Count) takes from $AnimDir (screen gets covered)"
    [DemoStage]::Start(0x00362A24)   # navy, BGR order for CreateSolidBrush
    Start-Sleep -Milliseconds 1600
    try {
        $scr = [DemoStage]::Screen()
        foreach ($take in $selected) {
            $id = $take.id
            $entry = $manifest.animations | Where-Object { $_.id -eq $id }
            if ($null -eq $entry) { Write-Host "  skip $id (not in $AnimDir)"; continue }
            $p = Start-Process -FilePath $exe -ArgumentList '--anim', $id, '--dir', $AnimDir, '--run', '30' `
                -WorkingDirectory $root -PassThru -WindowStyle Hidden
            $saved = @()
            try {
                $h = [IntPtr]::Zero
                for ($i = 0; $i -lt 100; $i++) {
                    Start-Sleep -Milliseconds 80
                    $h = [DemoStage]::FindPid($p.Id, 'FeiYuPetWindow')
                    if ($h -ne [IntPtr]::Zero) { break }
                }
                if ($h -eq [IntPtr]::Zero) { Write-Host "  no window for $id"; continue }
                $saved = [DemoStage]::HideOthers($p.Id, 'FeiYuPetWindow')
                Start-Sleep -Milliseconds 400

                # Anchor the crop on the feet, not on the window rect: clips are cut
                # tight, so the same box keeps the character in the same place across
                # takes. anchor_x = left + canvasW/2 - origin.x, feet_y = top + baseline.
                $rc = [DemoStage]::Rect($h)
                $anchorX = $rc[0] + [int]($canvasW / 2) - [int]$entry.origin[0]
                $feetY = $rc[1] + [int]$entry.baseline
                $cw = 460
                $ch = 360
                $x = [Math]::Max(0, $anchorX - 230)
                $y = [Math]::Max(0, $feetY - 310)
                if ($x + $cw -gt $scr[0]) { $x = $scr[0] - $cw }
                if ($y + $ch -gt $scr[1]) { $y = $scr[1] - $ch }
                $x = $x -band -bnot 1   # even offset and size: x264 4:2:0 wants it
                $y = $y -band -bnot 1

                $mp4 = Join-Path $clips "$id.mp4"
                # Creating or destroying the fullscreen stage invalidates any live
                # duplication (887a0026 = DXGI_ERROR_ACCESS_LOST), so settle first and
                # retry rather than trusting the first attempt.
                $ok = $false
                for ($try = 1; $try -le 8 -and -not $ok; $try++) {
                    Remove-Item $mp4 -Force -ErrorAction SilentlyContinue
                    # Growing backoff: right after the stage or the previous pet process
                    # goes away, duplication can stay unavailable for several seconds.
                    Start-Sleep -Milliseconds (1500 + 1200 * $try)
                    $ErrorActionPreference = 'Continue'
                    & $ffmpeg -y -v warning -f lavfi -i 'ddagrab=0:framerate=24:draw_mouse=0' `
                        -vf "hwdownload,format=bgra,crop=${cw}:${ch}:${x}:${y}" -t $Seconds `
                        -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv420p $mp4 2>&1 |
                        ForEach-Object { Write-Host "  ffmpeg: $_" }
                    $ErrorActionPreference = 'Stop'
                    $ok = (Test-Path $mp4) -and ((Get-Item $mp4).Length -gt 40KB)
                    if (-not $ok) { Write-Host "  retry $try for $id" }
                }
                if (-not $ok) { throw "recording $id failed" }
                Write-Host ("  {0,-16} win={1}x{2} crop={3},{4} -> {5}KB" -f $id, $rc[2], $rc[3], $x, $y, [int]((Get-Item $mp4).Length / 1KB))
            } finally {
                if ($saved.Count -gt 0) { [DemoStage]::Restore($saved) }
                $p | Stop-Process -Force -ErrorAction SilentlyContinue
                for ($i = 0; $i -lt 30; $i++) {
                    if ([DemoStage]::FindPid($p.Id, 'FeiYuPetWindow') -eq [IntPtr]::Zero) { break }
                    Start-Sleep -Milliseconds 100
                }
            }
        }
    } finally {
        [DemoStage]::Stop()
    }

    Write-Host '[3/5] cut the takes together -> gif'
    $inputs = @()
    $chain = ''
    for ($i = 0; $i -lt $selected.Count; $i++) {
        $t = $selected[$i]
        $inputs += @('-ss', "$($t.ss)", '-t', "$($t.dur)", '-i', (Join-Path $clips "$($t.id).mp4"))
        $chain += "[$i`:v]"
    }
    $n = $selected.Count
    $filter = "${chain}concat=n=${n}:v=1:a=0,fps=${GifFps},scale=${GifWidth}:-2:flags=lanczos,split[gifin][pal]"
    $filter += ";[pal]palettegen=max_colors=256:stats_mode=full[palette]"
    $filter += ';[gifin][palette]paletteuse=dither=sierra2_4a:diff_mode=rectangle[out]'
    $gifPath = Join-Path $root $Out
    & $ffmpeg -y -loglevel error @inputs -filter_complex $filter -map '[out]' -loop 0 $gifPath
    if ($LASTEXITCODE -ne 0) { throw 'ffmpeg gif encode failed' }

    Write-Host '[4/5] decode the gif back'
    Remove-Item "$audit\*" -Force -ErrorAction SilentlyContinue
    & $ffmpeg -y -loglevel error -i $gifPath -fps_mode passthrough (Join-Path $audit 'g%03d.png')
    if ($LASTEXITCODE -ne 0) { throw 'decoding the gif for the audit failed' }

    $worst = Measure-DarkestBlob $audit 18
    $frames = (Get-ChildItem $audit -Filter '*.png').Count
    $size = [int]((Get-Item $gifPath).Length / 1KB)
    Write-Host ('[5/5] {0}: {1} frames, {2}KB, worst frame has {3} near-black px' -f $Out, $frames, $size, $worst)
    # A clean gif sits in the dozens: that is the character's own dark outline. The
    # gdigrab / palette bugs both landed in the thousands.
    if ($worst -gt 400) {
        throw "black shadow back? $worst near-black pixels in the worst frame (expect < 400)"
    }
} finally {
    if ($petPath) {
        Write-Host "restarting the pet: $petPath"
        Start-Process $petPath -WorkingDirectory (Split-Path $petPath) | Out-Null
        $relaunched = $true
    }
    Pop-Location
}
