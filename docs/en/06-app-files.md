# 06 App, file by file

English | [简体中文](../06-逐文件讲解-app.md)

`feiyu-app` assembles core logic and platform primitives into a real desktop pet.

## 1. Module map

```
crates/feiyu-app/src/
├── lib.rs              exports + cli_main
├── cli.rs              command-line modes, demo mode, pump callback
├── main.rs             GUI entry point
├── bin/feiyu-cli.rs    console entry point
├── pet/
│   ├── mod.rs          Pet fields, open, tick, events, render, tray, geometry
│   ├── frame_source.rs FrameSource, FishAnimSource, VideoSource
│   ├── clips.rs        clip metadata, categories, random pool
│   ├── menu.rs         context menu and playback commands
│   ├── walk.rs         walking
│   ├── follow.rs       mouse-follow turning
│   ├── typing.rs       typing integration
│   ├── sleep.rs        natural wake and sleep spans
│   └── pixels.rs       mirroring, seam blending
└── selftest.rs         94 real-window checks
```

## 2. `cli.rs`

`Args` is parsed by hand. There are about twenty flags, so a CLI dependency would add more surface than it removes.

`run()` decides which mode to enter:

```rust
let dir = args.dir.clone().unwrap_or_else(default_anim_dir);
let cfg = load_config(args.config.as_deref())?;
if args.selftest { ... }
if args.list { return list_anims(&dir, &cfg); }
run_pet(args, dir, cfg)
```

`default_anim_dir()` checks the current directory first, then the executable's directory. In each location it prefers `assets/h264` and falls back to `assets/anim`. That makes a source checkout use H.264 by default while packaged builds continue to work when launched from a shortcut.

`load_config` priority: explicit `--config` > `config.json` next to the executable > `config.json` in the current directory > defaults. An explicit file that cannot be loaded is an error; a broken automatically discovered file warns and falls back.

`showcase` plays every clip for a full loop. It sleeps 10ms and calls `tick` instead of busy-waiting, because `tick` uses real `dt`; busy-waiting would prevent the frame accumulator from reaching a frame.

`list_anims` reads the manifest, groups clips by category, applies configured weights, and prints sizes and frame counts without opening a window.

## 3. `Pet` fields

```rust
pub struct Pet {
    win: LayeredWindow,
    manifest: Manifest,
    cfg: Config,
    anims: HashMap<String, Box<dyn FrameSource>>,
    current: String,
    frame: u32,
    acc: f32,
    anchor_x: i32,
    feet_y: i32,
    brain: Brain,
    loop_span: Option<(u32, u32)>,
    dissolve: Option<Dissolve>,
    sleep_cycle: Option<SleepCycle>,
    walk: Option<Walk>,
    motion: Option<Motion>,
    ...
}
```

The `anims` map is bounded. At most six sources are kept: the current clip, the pending clip, and warm `idle`/`drag` entries. Reopening a clip is cheaper than keeping one hundred decoded buffers alive.

## 4. `open`: startup order

`Pet::open`:

1. loads the manifest and validates it;
2. finds the work area;
3. computes the initial anchor and window position;
4. creates the layered window;
5. builds the random pool before moving the manifest into `Pet` (field initialization order matters);
6. loads the configured clip and draws the first frame.

`ensure_loaded(id)` delegates to `frame_source::open(dir, entry, fps)`:

```rust
match entry.codec {
    ClipCodec::FishAnim => Box::new(FishAnimSource::open(...)),
    ClipCodec::H264 => Box::new(VideoSource::open(video, mask, entry, fps)),
}
```

`VideoSource` decodes color and applies the 1-bit mask; the result is still premultiplied BGRA, so `draw_frame`, seam blending, mirroring, and hit testing do not know which codec is active.

## 5. `tick`: decisions every 8ms

`tick` computes real `dt` and advances the brain when the pet is not forced into a demo clip. A mood change calls `play(id)` and `after_switch(id)`.

Next, idle actions are scheduled. A walk clip can be rejected when there is no room, in which case the pet returns to idle and reschedules.

Mouse-follow turning, click-to-walk, typing integration, sleep, and physics are all advanced here. The order matters: the brain decides the pose before the frame accumulator decides whether a new frame should be drawn.

## 6. Events: click and drag

`on_event` handles mouse down/move/up, right-click, tray events, typing, and timer ticks. A press records the grab offset and preloads `drag`; movement beyond a small threshold starts the drag pose; release either clicks or flings the pet.

`clicked()` wakes a sleeping pet or stimulates `Brain`. `stimulate` picks a click-response clip for the current mood. `play` resets the frame, loop span, dissolve, walk, and sleep cycle, then hands the next window position to `draw_frame` through `pending_dst`, so position, size, and pixels are committed by one `UpdateLayeredWindow` instead of moving first and painting afterwards.

## 7. `advance_frame`: the easiest place to introduce bugs

This function advances the frame index, handles the loop span, starts a seam dissolve, and decides when an action clip has finished. The critical state is:

- `frame` — the requested frame;
- `acc` — the fractional frame accumulator;
- `loop_span` — the current sub-range, if any;
- `dissolve` — the seam blend in progress;
- `action_playing` — whether to return to idle after the clip.

Seam dissolve does not freeze the frame index. It blends the previous frame with the advancing current frame over six steps, so the pet keeps moving instead of visibly pausing.

## 8. Walking

`walk.rs` stores the remaining distance, direction, speed, and destination. `begin_walk` decides whether the pet can move (screen bounds) and chooses the clip. Each tick advances the anchor and places the window. A new click in the opposite direction interrupts and reverses the walk.

## 9. Menu

The menu is data-driven from `CATEGORIES` and `CLIPS`. Each menu command maps to a stable clip id; command ids are offset by a fixed base, so adding a menu item must not renumber existing entries.

`after_switch` applies per-clip entry rules: sleep starts a `SleepCycle`, idle reschedules sleep, drag skips the standing intro. These rules live in `clips.rs`, not in hard-coded id checks.

## 10. Typing integration

Typing produces heat that decays over time. When heat crosses a threshold, the pet randomly plays a work-state clip. The menu can disable the feature. `FEIYU_TYPING_TRACE` prints every attempt; `FEIYU_TYPING_EAGER` forces triggers for verification.

## 11. `selftest.rs`

The self-test is not a unit test. It opens a layered window, submits frames, captures the screen, and checks:

- transparency and hit testing;
- menu structure and command mapping;
- tray add/update/delete;
- baseline alignment;
- sleep span transitions;
- typing integration;
- window movement.

The screen-pixel check can fail on a non-composited desktop (for example, a remote session); that is an environment failure, not a codec regression.

## 12. Acceptance

- Trace a click from `on_event` to `draw_frame`.
- Explain why `anims` is bounded.
- Explain how `ensure_loaded` selects a codec.
- Explain why `play` resets loop span, dissolve, walk, and sleep state.
- Explain why a clip switch stages the window position instead of calling `SetWindowPos` first.
- Explain why the self-test needs a real desktop.
