# 07 Toolchain: `pack-anim`

English | [简体中文](../07-工具链-pack-anim.md)

`tools/pack-anim` is a standalone Rust tool. It converts the upstream webm clips into the runtime formats. It depends only on `feiyu-core` for format definitions; video decoding and encoding are delegated to ffmpeg.

## 1. Why a packer is needed

The runtime must not ship libvpx or a general-purpose media stack just to show a 10-second animation. The packer does the expensive work once:

- decode the source webm;
- key out the black background;
- crop to the opaque bounds;
- compute baseline and origin;
- write `.fishanim` (premultiplied BGRA + zstd + delta) or `.fishvid + .fishmask`.

The runtime then either decodes a self-contained zstd frame chain or asks Media Foundation for H.264 color and applies the mask.

## 2. Pipeline

```
webm (ffmpeg rawvideo) → RGBA/BGRA frames
    → build alpha from border flood fill
    → quantize color low bits
    → compute bbox and baseline
    → crop
    → BGRA + zstd (fishanim) OR yuv420p H.264 + 1-bit mask
    → manifest.json
```

### Canvas and size

The source is 640×360. The packer renders at height 306 and computes width proportionally, preserving the original 16:9 canvas. The manifest records the original canvas size and each clip's cropped size, origin, and baseline.

### Decoding

`decode::FrameSource` spawns ffmpeg and reads raw frames from a pipe. No video library is linked into the packer.

## 3. Keying: `mask::extract`

### Why not a global threshold

A global luminance threshold removes dark parts of the character (hair, eyes, clothing) as well as the background. The background is black, but so are some intended details.

### Border flood fill

The correct approach starts from the image border and floods through dark pixels. Only background connected to the border is made transparent. Interior dark pixels remain opaque.

The source is a black-composited video with no alpha, so `C = F × a`: colors are already premultiplied. A binary 0/255 alpha is not enough. Anti-aliased edges and translucent effects (bubbles, smoke, sparks) all carry `a < 1`, and promoting them to 255 gives a dark rim around the sprite and solid black blobs where the effect should be soft. Both show up in either codec, so it is not a decoder problem.

`mask::extract` now works in three steps:

1. background 0, everything else 255; small connected components (bubbles, sparks) between `effect_min` and `main × effect_rel` are normalized by their own 90th-percentile luminance, so they read as translucent instead of black discs;
2. a `--rim`-wide band (default 2) around the silhouette takes the **brightest inward neighbour** as `F` and solves `a = lum(C) / lum(F)`. Using the nearest inward colour instead of a component-wide core keeps one- or two-pixel features (braids, grass blades, dark outlines) from fading out;
3. the background colour is dilated by one pixel from the sprite, because H.264 subsamples chroma 2×2 and a black background would bleed a dark halo into the edge.

`extract` also returns straight color (`F = C × 255 / a`). `.fishanim` keeps the premultiplied pixels; the H.264 color plane must be straight, because a 1-bit mask drawn over premultiplied color is exactly the black rim.

Known limitation: a large translucent effect that is attached to the sprite (fireworks smoke) cannot be separated from art that is simply painted dark. A single frame of a black matte cannot tell them apart — **step 3.5 solves that with a second matte.**

### 3.5 Two-source unmixing: straight colour from the preview GIFs

The upstream repo also ships `assets/preview/*.gif` (106 files, 220×124, 12fps). They show the same character over the same timeline, **with 1-bit transparency and straight colour**. Two mattes close the equation:

```text
a = |C| / |F|     C from the webm (premultiplied on black), F from the GIF (straight)
```

With `--preview <dir>` (`preview.rs`):

1. **Pairing**: the GIF names are pinyin (`fang-yanhua.gif`) and do not match our slugs, so the packer matches first frames: shrink the webm frame to 220×124 and compare it against every GIF first frame (composited on black) by MSE. Correct pairs score 2–3 thousand, wrong ones usually exceed ten thousand; anything above 9000 counts as “no preview” and falls back to 3.4.
2. **Sampling**: 12fps GIF versus 24fps webm, so frame index is halved; past the end the last frame is reused. The GIF is upscaled with `premultiply` first, so transparent white never bleeds into the edge.
3. **Solving**: `a = max(C) / max(F)`; only pixels below 0.88 become candidates, and candidates are then filtered by connected area (≥ 40 px) to drop palette noise. The silhouette band (`edge`) skips that filter — that band is anti-aliasing, and thin arcs or smoke wisps would otherwise be erased down to black dots.
4. **Pixels the GIF dropped**: the preview only has 1-bit alpha, so translucent smoke is simply transparent there. Those pixels borrow the nearest opaque preview colour as `F` (BFS fill); otherwise they could only ever be black.

Output: `.fishanim` only changes alpha (colours stay premultiplied); the H.264 colour plane is recomputed with straight colour only where the mask keeps the pixel (`a ≥ 128`) — brightening masked-out pixels just burns bitrate.

Alpha is quantised to 5 bits (32 levels): the solved alpha is a smooth ramp, per-pixel differences are invisible, and they shatter zstd matches (worth 4.5% on `.fishanim`).

Each clip also prints a metric line:

```text
preview fang-yanhua.gif   solved  8.1%  dark  4.0% →  1.1%
```

`solved` is the share of pixels the second matte changed; `dark` is the share of “opaque and very dark” pixels (before → after), i.e. the direct measure of rims and blobs.

## 4. Quantization: `mask::quantize`

Dithering noise in the source creates high-frequency detail that is expensive to compress. The packer clears the low `n` bits of the color channels (default 3). The default is a deliberate quality/size trade-off; `--quant 0` keeps full color.

## 5. Crop and baseline

### Crop

The union of all opaque pixels across the clip becomes the crop rectangle. This removes large empty margins, especially around small characters in a 16:9 frame.

### Baseline: mode of the bottom-most row

For each frame, find the lowest opaque row. The baseline is the **mode** of those rows, not the maximum:

- fireworks, water splashes, and balloons occasionally reach the bottom for only a few frames;
- taking the maximum would lift the entire character by 20+ pixels.

Ties are broken toward the lower row. `baseline` is stored in cropped-frame coordinates; `origin` converts back to the original canvas.

## 6. Encoding and manifest

`format::encode_with(header, frames, level, delta)` builds the legacy `.fishanim` container. Defaults: `level=3`, `delta=16`, `quant=3`, `crop=true`.

The manifest records:

```json
{
  "format": 1,
  "canvas": { "width": 544, "height": 306 },
  "native_height": 306,
  "fps": 24,
  "animations": [
    { "id": "idle", "name": "Idle", "file": "idle.fishanim",
      "size": [211, 241], "origin": [163, 46], "baseline": 237,
      "frames": 241, "bytes": 5531091 }
  ]
}
```

`id` is the stable contract used by the state machine, random pool, and menu. The file name is an implementation detail.

### H.264 mode: `.fishvid + .fishmask`

```powershell
cargo run -p pack-anim --release -- `
  --src _upstream/dsh-pet/dsh-pet/assets/webm `
  --out assets/h264 --codec h264 --crf 20 --preview _upstream/dsh-pet/dsh-pet/assets/preview
```

For each clip:

- `<slug>.fishvid` — H.264 color in an MP4 container, using the `.fishvid` extension;
- `<slug>.fishmask` — 1-bit alpha + zstd (see `core/anim/mask.rs`);
- manifest entry gets `"codec": "h264"` and `"mask"`; `bytes` is video plus mask.

The encoder writes `-framerate <fps> -r <fps>` explicitly. Without it, ffmpeg defaults rawvideo to 25fps while the manifest says 24fps, and seeking to the last frame hits EOS too early. This bug was found during the prototype.

Color dimensions are padded to even width/height; the decoder crops back to the manifest size. CRF20 over 100 clips measures:

- old keying 69.5MB;
- edge band + straight colour 79.2MB;
- **two-source unmixing (`--preview`) 97.7MB** — translucent areas get brighter, which costs bitrate; CRF22 or a 4-bit mask could win it back later.

`.fishanim` goes 581.6 / 624.7 / **654.3MB** for the same three steps (after alpha quantisation).

## 7. Chinese name → file name

`NAME_MAP` maps upstream Chinese titles to stable ASCII slugs:

```rust
("待机呼吸休闲", "idle"),
("点击回应-挠痒咯咯笑", "click_tickle"),
("工作状态-垂头叹气冒汗", "work_sigh"),
```

- Chinese remains only in the manifest's display name;
- ASCII file names avoid encoding problems in git, zip, and ffmpeg;
- the six balance-event clips are intentionally not registered, so directory mode skips them.

## 8. Packing and verification

```powershell
# Legacy format
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim --preview _upstream/dsh-pet/dsh-pet/assets/preview

# Production H.264 format
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/h264 --codec h264 --crf 20 --preview _upstream/dsh-pet/dsh-pet/assets/preview

# Legacy verification: decode forward and reverse, check the delta chain
cargo run -p pack-anim --release -- --verify assets/anim
```

`--dump <n>` exports a PNG every n frames on a checkerboard background for visual inspection.

To judge transparency, `feiyu-cli --dump <id> <dir> <step> --dump-bg RRGGBB` writes the same frame
on white, mid-grey and black; the black one should match the original webm almost exactly.

## 9. Adding an animation

1. Add `("中文名", "slug")` to `NAME_MAP`.
2. Add the clip to the appropriate `CATEGORIES` entry in `pet/clips.rs`.
3. If it needs an intro/outro span, walking, or a special entry action, add a `ClipSpec` in `CLIPS`.
4. Re-pack.
5. Run `feiyu-cli.exe --list` and check its category.
6. Run `feiyu-cli.exe --showcase <slug>` and watch it.
7. Run `--selftest`; it checks the 100 → 97 menu and 89 random-pool counts.

## 10. Acceptance

- Draw the webm → runtime-format pipeline.
- Explain why keying must flood from the border.
- Explain why baseline uses the mode, not the maximum.
- Explain the H.264 mask and the 24fps time-base trap.
- Add a clip and make it appear in the menu.
