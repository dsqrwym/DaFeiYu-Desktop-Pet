# 04 Core, file by file

English | [简体中文](../04-逐文件讲解-core.md)

`feiyu-core` is deliberately free of Windows APIs. It owns animation containers, the manifest, the state machine, physics, sleep choreography, configuration, and the 1-bit mask format.

## 1. Module map

```
crates/feiyu-core/src/
├── lib.rs
├── anim/
│   ├── mod.rs
│   ├── format.rs      .fishanim container
│   ├── manifest.rs    manifest.json + path validation
│   └── mask.rs        .fishmask 1-bit alpha
├── brain.rs           mood state machine
├── config.rs          user parameters
├── motion.rs          fling/fall/bounce
├── pool.rs            weighted random pool
├── rng.rs             xorshift64
└── sleep.rs           sleep-cycle segmentation
```

## 2. `anim/format.rs` — `.fishanim`

The legacy container is a zstd-compressed table of frames with delta compression. The header records width, height, fps, frame count, baseline, origin, and the absolute offset of each frame.

Key properties:

- random access through an offset table;
- deltas only reference a limited keyframe chain, so any frame can be reconstructed;
- corruption and truncation are checked before decoding;
- the returned pixels are premultiplied BGRA.

Tests cover round trips, bad magic, truncated data, truncation at every boundary, and delta-chain consistency.

## 3. `anim/manifest.rs` — the contract between packer and player

`Manifest` contains the canvas size, fps, native height, and a list of `AnimEntry` values:

```rust
struct AnimEntry {
    id: String,
    name: String,
    file: String,
    codec: ClipCodec, // FishAnim (default) or H264
    mask: Option<String>,
    size: [u16; 2],
    origin: [u16; 2],
    baseline: u16,
    frames: u32,
    bytes: u64,
}
```

`origin` is the cropped frame's position in the original canvas. `baseline` is the foot line inside the cropped frame; `feet_row()` converts it back to canvas coordinates.

Validation checks:

- format version, non-empty animation list, unique ids;
- non-zero sizes and frame counts;
- geometry inside the canvas;
- H.264 entries require a mask; fishanim entries must not have one;
- `file` and `mask` must be plain filenames in the same directory (no `..`, no absolute paths).

The path rule prevents a malicious manifest from reading arbitrary files.

## 4. `anim/mask.rs` — `.fishmask`

H.264 color does not carry alpha. The mask is:

```text
0   8  "FISHMSK1"
8   2  width   u16
10  2  height  u16
12  4  frames  u32
16  ... zstd(frame-major bit-packed masks, LSB-first)
```

Each frame uses `ceil(width × height / 8)` bytes. Alpha ≥ 128 is 1. The whole file is decompressed once; `frame_bits(idx)` returns a slice for any frame. `matches(w, h, frames)` checks the mask against the manifest before playback.

A 241-frame mask is about 85–110KB compressed and about 1.5MB in memory. Keeping alpha out of H.264 avoids the edge halos produced by lossy alpha; the cost is one extra bitmap per loaded clip.

## 5. `brain.rs` — the mood state machine

Five moods: `Idle`, `Happy`, `Pout`, `Sleep`, `Drag`.

The brain tracks three time quantities:

- `elapsed`: time since the last meaningful interaction;
- `sleep_after`: the randomized delay before sleep;
- `action_gap`: the randomized delay before an idle action.

Stimuli (`Click`, `DragStart`, `DragEnd`, `Typing`, ...) trigger transitions. The brain is pure logic and is unit-tested; it does not know about windows or files.

## 6. `motion.rs` — after release

Velocity is estimated from the first and last mouse samples over roughly 120ms. `Motion::step` integrates gravity, handles bounce and friction, and returns `Rest` when the pet has landed and slowed down.

Why first/last samples: averaging every sample would underestimate a quick flick and make the pet feel sticky. Using the endpoints captures the release velocity.

## 7. `pool.rs` — weighted random pool

`Pool` stores `(id, weight)` pairs. `pick` draws a weighted random id and avoids immediately repeating the same item when possible. Category weights are split evenly across the clips in that category.

## 8. `rng.rs` — no external `rand`

A small xorshift64 generator keeps the dependency tree small and makes tests reproducible by seeding. It is not cryptographic and is only used for animation selection.

## 9. `sleep.rs` — one clip, three stages

The 10-second sleep animation is a closed loop, but the desired behavior has distinct phases. `SleepCycle`:

- `Falling` — frames 0–156; lie down and produce Z bubbles;
- `Asleep` — frames 104–156; loop where 2–3 bubbles are visible;
- `Waking` — current frame → 240; play forward and let the bubbles dissipate.

Why start waking at the current frame: waking from the middle of the loop should not jump back to the beginning. Why loop at 104–156: at frame 174 the Z bubbles have already dissipated; looping there would look like the pet is awake while the animation says it is asleep.

## 10. `config.rs` — user parameters

Configuration lives in core because it is product logic, not window logic. Parsing uses serde defaults: missing fields use defaults; invalid values are clamped or replaced instead of rejecting the whole file.

Load priority:

1. explicit `--config` path (must load, or error);
2. `config.json` next to the executable;
3. `config.json` in the current directory;
4. built-in defaults.

A malformed automatically discovered file produces a warning and falls back to defaults; a malformed explicit file is an error.

## 11. `mask.rs` in the pipeline

The packer computes alpha from the black-background webm, writes the 1-bit mask, and encodes H.264 color separately. At runtime `VideoSource` decodes color, reads the mask slice, copies visible BGRA pixels, and zeros transparent pixels.

The mask is not an alpha video: it is a lossless binary decision. That is why hard edges stay hard and do not shimmer.

## 12. Acceptance

- Explain the `.fishanim` offset table and delta chain.
- Explain why H.264 needs a separate mask.
- Explain `origin`, `baseline`, and `feet_row()`.
- Explain why sleep uses three stages.
- Explain why configuration loading distinguishes explicit and implicit paths.
