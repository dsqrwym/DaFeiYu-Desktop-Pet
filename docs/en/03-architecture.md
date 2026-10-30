# 03 Architecture and the journey of one click

English | [简体中文](../03-架构与一次点击的旅程.md)

## 1. Four layers, dependencies only point downward

```
feiyu-app ──► feiyu-core
    │
    └──────► feiyu-platform
tools/pack-anim ──► feiyu-core
```

- **`feiyu-core`** — no Windows APIs. Animation containers, the `.fishmask` format, the state machine, physics, sleep choreography, and user configuration. It can be unit-tested anywhere.
- **`feiyu-platform`** — the only layer allowed to use `unsafe` and `windows::*`. It wraps the window, menu, tray, input, and H.264 decoding.
- **`feiyu-app`** — assembles the layers. `pet/frame_source.rs` abstracts “where does frame N come from”; the rest of `pet/` contains the brain and render loop. `cli.rs` handles command-line modes; `selftest.rs` is the real-Windows acceptance suite.
- **`pack-anim`** — converts webm into `.fishanim` or `.fishvid + .fishmask`.

The app never calls Win32 directly. That keeps `pet/` testable and prevents `HWND`, `UpdateLayeredWindow`, and DIB details from leaking into the state machine.

## 2. One UI thread

Windows window messages are handled on the thread that created the window. The program therefore runs:

```
main thread: CreateWindowEx → pump(GetMessageW → DispatchMessageW → wndproc)
                                      ↓ events
                           pump drains queue → on_event → Pet::tick/draw
```

The timer (`SetTimer`, 8ms) only pushes an `Event::Tick`. It never calls business logic directly. All business logic runs sequentially on the UI thread and needs no locks, except the pixel-alpha buffer shared with `WM_NCHITTEST`.

`wndproc` must not draw or perform expensive work. It only pushes events into a `VecDeque` and returns. Blocking the message loop makes menus and dragging feel stuck.

## 3. Three coordinate systems

| Coordinate system | Origin | Used for |
|---|---|---|
| Screen pixels | top-left of the screen | window position, mouse position |
| Original canvas | top-left of the 544×306 frame | `origin` / `baseline` in `manifest.json` |
| Frame pixels | top-left of the cropped frame | the bitmap submitted to `UpdateLayeredWindow` |

The packer crops each clip to the union of its opaque pixels, so every clip can have a different frame size. The manifest records:

- `origin`: where the cropped frame sits in the 544×306 canvas;
- `baseline`: the foot line **inside the cropped frame**.

The window position is computed from the anchor:

```rust
let x = anchor_x - canvas_width / 2 + origin_x;
let y = feet_y - baseline;
```

This is `window_pos()` in `pet/mod.rs`. Every code path that places the window must use it; duplicating the formula causes “off by 46 pixels” bugs.

## 4. One click, end to end

### 4.1 Message arrives

1. `WM_LBUTTONDOWN` reaches `wndproc`.
2. `wndproc` records the mouse grab offset and pushes `Event::MouseDown`.
3. `pump` dispatches the message, drains the queue, and calls `on_event(MouseDown)`.

### 4.2 Business layer

`Pet::on_event` records the grab origin, clears `dragging`, and preloads the `drag` animation with `ensure_loaded("drag")`.

### 4.3 Release

1. `WM_LBUTTONUP` becomes `Event::MouseUp`.
2. If `dragging == false`, this was a click and `clicked()` runs.
3. `clicked()` wakes a sleeping pet or calls `stimulate(Stimulus::Click)`.
4. `Brain` changes mood to `Happy` (or `Pout` after three rapid clicks).
5. `play(id)` switches the current animation and calls `after_switch`.
6. `action_playing = true` means “return to idle after this clip finishes”.

### 4.4 Next tick

1. Timer → `Event::Tick` → `Pet::tick()`.
2. `tick` computes `dt = now - last` and advances the brain, typing, and physics.
3. `acc += dt * fps`; when it exceeds one frame, `step_frames(1)` runs.
4. `advance_frame` advances the frame index and handles loop/return.
5. `draw_frame` gets the frame from its `FrameSource`, mirrors if needed, blends a seam if needed, and calls `win.draw()`.
6. `win.draw()` submits premultiplied BGRA through `UpdateLayeredWindow`; DWM composites it.

### 4.5 Finishing a clip

When the frame wraps and `action_playing` is true, `step_frames` clears it, calls `resume_idle_at()`, and schedules the next idle action and sleep check.

> Win32 only puts events into the queue. `pet/` decides which frame should be visible. `core` owns the pure decisions behind those choices.

## 5. One drag

1. Press: record the grab offset and preload `drag`.
2. Move more than 2 pixels: play `drag`, restrict playback to frames 35–110 (skip the standing intro).
3. While dragging: `WM_MOUSEMOVE` calls `SetWindowPos` for direct tracking and records recent samples.
4. Release: estimate velocity from the first and last sample and hand it to `Motion` for gravity and bounce.
5. Land with low speed: `rest`; hold the drag pose for 0.1 seconds, then return to idle.

The pet does not switch to idle immediately on release because it may still be in the air. `landed_at` is set only when `Motion::step` returns `rest`.

## 6. One sleep cycle

`SleepCycle` divides the 10-second sleep clip into three stages:

| Stage | Frames | Behavior |
|---|---|---|
| `Falling` | 0–156 | stand → close eyes → curl up → Z bubbles 1→2→3 |
| `Asleep` | 104–156 | loop only while 2–3 bubbles are visible |
| `Waking` | current–240 | play forward from the current frame; Z bubbles dissipate |

At the end of a segment, `advance_frame` calls `cycle.on_segment_end()`. Falling→asleep returns true; the asleep segment returns false and keeps looping; waking runs to the end and `step_frames` returns to idle.

This complexity exists because the source is one 10-second closed loop. Playing it as a normal loop makes the pet “sleep for ten seconds and wake up”; not looping only gives a two-second sleep.

## 7. Where state lives

| State | Stored in | Why |
|---|---|---|
| Mood (idle/sleep/happy/pout/drag) | `Brain.mood` | pure logic, unit-testable |
| Current animation id, frame, time remainder | `Pet` | tied to the window |
| Frame source (fishanim / h264 + mask) | `pet::FrameSource` | player only needs frame N |
| Product parameters | `core::Config` | loaded once at startup, read-only |
| Walking distance/direction/speed | `Pet.walk` | advanced every frame |
| Physics velocity | `Pet.motion` | exists after release |
| Sleep stage | `SleepCycle` | pure logic, unit-testable |
| Typing heat/cooldown | `Pet` | uses `Instant` |
| Window pixel alpha | `platform::WndState` | read by `WM_NCHITTEST` |

The rule: **put pure logic in `core`**. `Brain`, `Motion`, `Pool`, and `SleepCycle` are unit-tested; `Pet` translates their decisions into window operations.

## 8. Acceptance

- Draw the “wndproc → queue → pump → Pet::on_event → tick → draw” flow.
- Explain `origin` versus `baseline`.
- Explain why `wndproc` must not block.
- Name the functions a click passes through before returning to idle.
