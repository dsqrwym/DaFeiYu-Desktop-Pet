# Build and probe the H.264 + alpha prototype from the production .fishanim.
# ASCII only: Windows PowerShell 5.1 reads BOM-less files as ANSI.
#
# For each clip it writes:
#   <out>\<name>.raw                    premultiplied BGRA frames (production frames)
#   <out>\<name>_sbs_crf<crf>.mp4       left = color, right = alpha (one decoder)
#   <out>\<name>_color_crf<crf>.mp4     color only, width padded to even
#   <out>\<name>_alpha_crf<acrf>.mp4    alpha only, width padded to even
# and runs mf-probe on the side-by-side and the two-stream variants.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tools/h264-proto.ps1
#   powershell -ExecutionPolicy Bypass -File tools/h264-proto.ps1 -Crf 23 -AlphaCrf 18

param(
    [string]$Out = 'target\proto_h264',
    [int]$Crf = 20,
    [int]$AlphaCrf = 18,
    [int]$Dilate = 0,
    [switch]$SkipProbe
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
New-Item -ItemType Directory -Force $Out | Out-Null

$clips = @(
    @{ name = 'watermelon'; id = 'eat_watermelon' },
    @{ name = 'crab'; id = 'move_crab' }
)

Write-Host 'cargo build -p mf-probe --release'
cargo build -p mf-probe --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$probe = Join-Path $root 'target\release\mf-probe.exe'

foreach ($clip in $clips) {
    $fish = Join-Path $root ("assets\anim\{0}.fishanim" -f $clip.id)
    if (-not (Test-Path $fish)) { throw "missing $fish (run pack-anim first)" }
    $raw = Join-Path $root (Join-Path $Out ("{0}.raw" -f $clip.name))
    $enc = Join-Path $root (Join-Path $Out ("{0}_enc.raw" -f $clip.name))
    $mask = Join-Path $root (Join-Path $Out ("{0}.mask" -f $clip.name))

    Write-Host "[$($clip.name)] export (dilate=$Dilate)"
    $info = (& $probe --export $fish --raw-out $raw --enc-out $enc --dilate $Dilate --mask-out $mask) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "export failed: $info" }
    $m = [regex]::Match($info, 'width=(\d+) height=(\d+) frames=(\d+)')
    if (-not $m.Success) { throw "cannot parse export: $info" }
    $w = [int]$m.Groups[1].Value
    $h = [int]$m.Groups[2].Value
    $frames = [int]$m.Groups[3].Value
    $we = $w + ($w % 2)

    $sbs = Join-Path $root (Join-Path $Out ("{0}_sbs_crf{1}.mp4" -f $clip.name, $Crf))
    $sbsFilter = "[0:v]split=2[c][a];[c]format=bgra,format=yuv420p[cc];[a]alphaextract,format=yuv420p[aa];[cc][aa]hstack=inputs=2[v]"
    Write-Host "[$($clip.name)] side-by-side crf $Crf"
    & ffmpeg -hide_banner -loglevel error -y -f rawvideo -pix_fmt bgra -s "${w}x${h}" -i $raw `
        -filter_complex $sbsFilter -map '[v]' `
        -c:v libx264 -preset medium -crf $Crf -pix_fmt yuv420p `
        -g 24 -keyint_min 24 -sc_threshold 0 -an $sbs
    if ($LASTEXITCODE -ne 0) { throw 'ffmpeg side-by-side failed' }

    $color = Join-Path $root (Join-Path $Out ("{0}_color_crf{1}.mp4" -f $clip.name, $Crf))
    Write-Host "[$($clip.name)] color crf $Crf (pad ${we}x${h}, dilated input)"
    & ffmpeg -hide_banner -loglevel error -y -f rawvideo -pix_fmt bgra -s "${w}x${h}" -i $enc `
        -vf "format=bgra,pad=${we}:${h}:0:0:black,format=yuv420p" `
        -c:v libx264 -preset medium -crf $Crf -pix_fmt yuv420p `
        -g 24 -keyint_min 24 -sc_threshold 0 -an $color
    if ($LASTEXITCODE -ne 0) { throw 'ffmpeg color failed' }

    $alpha = Join-Path $root (Join-Path $Out ("{0}_alpha_crf{1}.mp4" -f $clip.name, $AlphaCrf))
    Write-Host "[$($clip.name)] alpha crf $AlphaCrf (pad ${we}x${h})"
    & ffmpeg -hide_banner -loglevel error -y -f rawvideo -pix_fmt bgra -s "${w}x${h}" -i $raw `
        -vf "alphaextract,pad=${we}:${h}:0:0:black,format=yuv420p" `
        -c:v libx264 -preset medium -crf $AlphaCrf -pix_fmt yuv420p `
        -g 24 -keyint_min 24 -sc_threshold 0 -an $alpha
    if ($LASTEXITCODE -ne 0) { throw 'ffmpeg alpha failed' }

    $sbsKB = (Get-Item $sbs).Length / 1KB
    $colorKB = (Get-Item $color).Length / 1KB
    $alphaKB = (Get-Item $alpha).Length / 1KB
    $maskKB = (Get-Item $mask).Length / 1KB
    Write-Host ('  sbs={0:N1}KB color={1:N1}KB alpha={2:N1}KB sep={3:N1}KB mask={4:N1}KB color+mask={5:N1}KB' -f $sbsKB, $colorKB, $alphaKB, ($colorKB + $alphaKB), $maskKB, ($colorKB + $maskKB))

    if (-not $SkipProbe) {
        & $probe --video $sbs --ref $raw --out (Join-Path $Out ("decoded_{0}_sbs" -f $clip.name)) `
            --frames $frames --fps 24 --width $w --height $h --label "$($clip.name)_sbs"
        if ($LASTEXITCODE -ne 0) { throw 'mf-probe side-by-side failed' }

        & $probe --video $color --alpha-video $alpha --ref $raw --out (Join-Path $Out ("decoded_{0}_sep" -f $clip.name)) `
            --frames $frames --fps 24 --width $w --height $h --video-width $we --label "$($clip.name)_sep"
        if ($LASTEXITCODE -ne 0) { throw 'mf-probe two-stream failed' }

        & $probe --video $color --mask $mask --ref $raw --out (Join-Path $Out ("decoded_{0}_mask" -f $clip.name)) `
            --frames $frames --fps 24 --width $w --height $h --video-width $we --label "$($clip.name)_mask"
        if ($LASTEXITCODE -ne 0) { throw 'mf-probe mask failed' }

        & $probe --video $color --mask $mask --feather --ref $raw --out (Join-Path $Out ("decoded_{0}_feather" -f $clip.name)) `
            --frames $frames --fps 24 --width $w --height $h --video-width $we --label "$($clip.name)_feather"
        if ($LASTEXITCODE -ne 0) { throw 'mf-probe feather failed' }
    }
}

Pop-Location
