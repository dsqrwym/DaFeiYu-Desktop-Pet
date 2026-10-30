# 05 Platform, file by file

English | [简体中文](../05-逐文件讲解-platform.md)

`feiyu-platform` is the only crate allowed to use `unsafe` and `windows::*`. It wraps only what the pet needs and documents the preconditions of every `unsafe` block.

## 1. Module map

```
crates/feiyu-platform/src/
├── window.rs     layered window + DIB + wndproc + message pump
├── menu.rs       native context menu
├── tray.rs       Shell_NotifyIconW
├── icon.rs       frame pixels → HICON
├── input.rs      last input time, cursor, left button
├── typing.rs     raw keyboard input counter
├── video.rs      Media Foundation H.264 decoder
├── shot.rs       screen capture → BMP
├── screen.rs     primary work area
├── dpi.rs        DPI awareness
└── instance.rs   single-instance mutex
```

## 2. `window.rs` — layered window

`UpdateLayeredWindow` gives per-pixel transparency. The window stores a 32bpp DIB, a compatible memory DC, and the old bitmap selected into the DC.

`draw(width, height, bgra)`:

- if the size changed, replace the DIB (different clips have different cropped sizes);
- copy premultiplied BGRA into the DIB;
- copy alpha into a global hit-test buffer;
- call `present()`.

`present()` uses `AC_SRC_ALPHA` and `ULW_ALPHA`. Pixels must be premultiplied; the packer does this conversion once, not every frame.

`WM_NCHITTEST` returns `HTTRANSPARENT` for alpha 0 and `HTCLIENT` for visible pixels. The alpha buffer is shared with the UI thread through a `Mutex<Option<WndState>>` because Windows may call hit testing from another thread.

`wndproc` only enqueues events:

- mouse down/move/up;
- right-click;
- timer ticks;
- tray callback;
- raw input;
- destroy.

It never draws or does work that can block. `pump` dispatches messages and drains the queue after each `GetMessageW`.

Why each message drains the queue: shell tray callbacks are delivered synchronously. A hidden window stops the animation timer, so without a wake timer the callback can sit in the queue forever and the tray click appears dead.

`Drop` restores the old bitmap, deletes the DIB, deletes the DC, and destroys the window, in that order.

`find_window` uses `EnumWindows` rather than `FindWindowW`: the latter returned 0 for this `WS_EX_NOACTIVATE` layered window in practice.

## 3. `menu.rs` — native context menu

`TrackPopupMenu` provides keyboard navigation, Esc handling, click-outside dismissal, and high-DPI placement. Reimplementing those behaviors would be large and worse.

Two important fixes:

- with `WS_EX_NOACTIVATE`, the owner is not foreground, so the menu can immediately cancel. The code temporarily attaches to the foreground thread / uses the standard foreground workaround, then restores state;
- submenu handles must be destroyed in the right order; destroying the root does not necessarily destroy every submenu.

A self-test hook builds the menu and returns its command mapping without requiring a human click.

## 4. `tray.rs` — tray icon

Four operations: add, update, delete, and set the tooltip. `Drop` must call `NIF_DELETE`; otherwise a ghost icon remains until Explorer restarts.

`NIF_MESSAGE` and `NIF_ICON` are sent together when changing the icon. Sending only the icon can leave the callback id/tooltip out of sync and break clicks.

This project does not implement the v4 notification protocol; the known tray-click limitation is documented and intentionally out of scope.

## 5. `icon.rs` — frame → HICON

Windows expects straight alpha in `HICON`, while the renderer uses premultiplied BGRA. `Icon::from_bgra` converts each channel back:

```text
straight = premultiplied * 255 / alpha
```

It also handles the all-transparent case and owns the resulting `HICON` with a `Drop` implementation.

## 6. `input.rs` — idle and cursor state

Uses `GetLastInputInfo` for the last system input time, `GetCursorPos` for the cursor, and `GetAsyncKeyState` for the left button. These are used by idle/sleep logic and click handling.

## 7. `typing.rs` — raw keyboard counter

Registers raw keyboard input with `RIDEV_INPUTSINK`. It counts key-down transitions and ignores key-up events. It does not read text or key identities; only the count is exposed. `disable_typing()` unregisters the device.

## 8. `shot.rs` — capture to BMP

`GdiFlush` → `BitBlt` from the screen to a DIB → `GetDIBits` → write BMP. BMP avoids an image dependency in the release build; development screenshots can be converted to PNG with ffmpeg.

## 9. `screen.rs`, `dpi.rs`, `instance.rs`

- `screen.rs`: `SystemParametersInfoW(SPI_GETWORKAREA)` returns the work area excluding the taskbar.
- `dpi.rs`: `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` prevents blurry scaling at 150%.
- `instance.rs`: a named mutex; `Ok(None)` means another instance is already running, so the new process exits quietly.

## 10. `video.rs` — Media Foundation H.264

`VideoReader::open(path, width, height, fps, frame_count)`:

- starts Media Foundation once per process (`OnceLock`);
- creates an `IMFSourceReader` with advanced video processing;
- reads native frame size and `MF_MT_FRAME_RATE` from the native media type;
- requests `MFVideoFormat_RGB32`;
- pads odd dimensions to even sizes for H.264, then crops to the manifest size;
- reads `MF_MT_DEFAULT_STRIDE` to detect bottom-up frames;
- decodes only one frame during sequential playback;
- seeks from a keyframe before the target for random access.

The time-base trap: ffmpeg's rawvideo input defaults to 25fps while the manifest says 24fps. `VideoReader` uses the container's native frame rate for seek timestamps, and `pack-anim` explicitly writes `-framerate <fps> -r <fps>`.

Only RGB32 output is implemented. If a system cannot provide RGB32, the decoder returns a clear error; NV12 conversion is a future compatibility task.

## 11. Acceptance

- Explain `WS_EX_NOACTIVATE` and its menu/keyboard/focus consequences.
- Explain the three parts of the popup-menu fix.
- Explain why tray icon updates need `NIF_MESSAGE`.
- Explain premultiplied versus straight alpha.
- Explain why raw input checks key-down.
- Explain why H.264 seek uses the container time base rather than the manifest fps.
