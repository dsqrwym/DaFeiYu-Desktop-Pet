# 02 Rust syntax map (using this project as the example)

English | [简体中文](../02-Rust-语法地图.md)

This is not a complete Rust book. It maps the syntax you will meet in this repository and explains the ideas that usually trip up readers coming from Python, JavaScript, or Java.

## 1. Workspace, crates, modules

A Cargo workspace is a collection of crates:

```toml
[workspace]
members = ["crates/feiyu-core", "crates/feiyu-platform", "crates/feiyu-app", "tools/pack-anim"]
```

- **crate** — one compilation unit (one library or binary);
- **module** — a namespace inside a crate (`mod pet;`, `mod clips;`);
- **package** — a `Cargo.toml` plus its crates.

This project splits code by dependency direction. `feiyu-core` cannot import `windows`; only `feiyu-platform` can. The compiler enforces the architecture.

## 2. Variables, mutability, constants

```rust
let width = 544;       // immutable by default
let mut frame = 0u32;  // explicitly mutable
const TICK_MS: u32 = 8;
```

Rust does not let you mutate a binding unless it is declared `mut`. This makes accidental state changes visible in the signature and in review.

Constants are compile-time values. Configuration values that can change at runtime live in `feiyu-core/src/config.rs`, not in `const`.

## 3. Functions and expressions

```rust
fn window_pos(canvas_width: u16, entry: &AnimEntry, anchor_x: i32, feet_y: i32) -> (i32, i32) {
    let x = anchor_x - canvas_width as i32 / 2 + entry.origin[0] as i32;
    let y = feet_y - entry.baseline as i32;
    (x, y)
}
```

The last expression is the return value; `return` is only needed for early exits. `if`, `match`, and blocks are expressions too:

```rust
let scale = if dpi > 120 { 1.5 } else { 1.0 };
```

## 4. Structs, enums, `match`

```rust
struct AnimEntry {
    id: String,
    frames: u32,
    size: [u16; 2],
}

enum Mood { Idle, Happy, Pout, Sleep, Drag }
```

`match` must be exhaustive:

```rust
match mood {
    Mood::Idle => ...,
    Mood::Happy => ...,
    Mood::Pout => ...,
    Mood::Sleep => ...,
    Mood::Drag => ...,
}
```

Adding a new enum variant makes every non-wildcard `match` fail to compile. That is intentional: the compiler shows you every place that needs a decision.

## 5. `Option` and `Result`

Rust has no `null` and no exceptions.

```rust
Option<T> = Some(T) | None
Result<T, E> = Ok(T) | Err(E)
```

- Use `Option` when absence is normal: `loop_span: Option<(u32, u32)>`.
- Use `Result` when an operation can fail: opening a file, decoding a frame, creating a window.

The `?` operator propagates the error:

```rust
let manifest = Manifest::load(path)?;
let entry = manifest.require(id)?;
```

`?` returns early on `Err` and converts the error through `From`. This project uses `anyhow::Result` at the application boundary and typed errors in core/platform code.

`.with_context(...)` adds human-readable context without losing the original error.

## 6. Ownership, borrowing, lifetimes

### Ownership

A value has one owner. Moving it transfers ownership:

```rust
let s = String::from("idle");
consume(s);        // s moved
// println!("{s}"); // compile error
```

### Borrowing

You may have many immutable references **or** one mutable reference, never both at the same time:

```rust
let mut frames = Vec::new();
let first = &frames[0];        // immutable borrow
// frames.push(frame);         // cannot mutate while first is alive
```

This is why `draw_frame` destructures `Self`:

```rust
let Self { win, anims, current, frame, .. } = self;
let anim = anims.get_mut(current)?;
```

The compiler can then see that `win` and `anims` are different fields, so borrowing both mutably is sound.

### Lifetimes

A lifetime is the period for which a reference is valid. Most lifetimes are inferred. `frame(&mut self, idx: u32) -> Result<&[u8]>` means “the returned slice lives as long as the mutable borrow of the player”. You cannot keep the slice after the next mutation.

A practical rule: if the compiler complains about a borrow, first ask **who owns the value** and **how long the reference is needed**, not which lifetime annotation to add.

## 7. Strings and collections

- `String` owns UTF-8 text; `&str` borrows it.
- `Vec<T>` is a growable array.
- `HashMap<K, V>` is used for loaded animation sources:
  `HashMap<String, Box<dyn FrameSource>>`.
- `VecDeque<T>` is the window-event queue; push at the back, pop from the front.

## 8. Iterators and closures

```rust
let visible: Vec<&AnimEntry> = entries
    .iter()
    .filter(|e| e.frames > 0)
    .collect();
```

Iterators are lazy. `map`, `filter`, and `collect` describe a pipeline; the work happens when consumed. Closures capture variables from the surrounding scope:

```rust
let mut count = 0;
let mut bump = || count += 1;
bump();
```

The compiler infers whether a closure needs `Fn`, `FnMut`, or `FnOnce`.

## 9. Traits, derive, generics

A trait describes shared behavior:

```rust
trait FrameSource {
    fn frame_count(&self) -> u32;
    fn frame(&mut self, idx: u32) -> Result<&[u8]>;
}
```

`FishAnimSource` and `VideoSource` implement it. The player stores `Box<dyn FrameSource>` so it does not care which codec produced the pixels.

`#[derive(Debug, Clone, PartialEq)]` asks the compiler to generate common implementations. This project derives `Deserialize` for manifest structs and `Error` for error enums via `thiserror`.

## 10. Tests

Tests live in the same file as the code:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_h264_over_legacy_fishanim() {
        // ...
    }
}
```

`#[cfg(test)]` removes the module from release builds. `use super::*` imports the parent module, including private helpers, so tests can check implementation details without making them public.

## 11. `unsafe` and SAFETY comments

Only `feiyu-platform` is allowed to use `unsafe`. Every block must explain why its preconditions hold:

```rust
// SAFETY: `bits` points to a DIB owned by this window and is valid for `len` bytes.
unsafe { copy_nonoverlapping(src.as_ptr(), self.bits, len) };
```

`unsafe` does not turn off the borrow checker; it only permits specific operations such as raw pointer access and FFI calls.

## 12. Atomics and time

`AtomicBool` / `AtomicU64` are used where a callback cannot hold a lock. `Instant` measures elapsed time; `Duration` is the difference. Animation timing uses `f32` seconds, but frame indices stay `u32` to avoid float drift.

## 13. Visibility

- private: default;
- `pub(crate)`: visible inside the crate;
- `pub`: public API.

This project exposes small interfaces (`FrameSource`, `VideoReader`) and keeps implementation helpers private.

## 14. Cheat sheet

| Rust | Meaning in this project |
|---|---|
| `let mut x` | state that changes every tick |
| `Option<T>` | optional span, optional motion, optional dissolve |
| `Result<T, E>` | file, decode, Win32, and manifest failures |
| `?` | propagate an error with context |
| `match` | exhaustive state/event handling |
| `&mut self` | mutate the player or window |
| `Box<dyn Trait>` | choose a codec at runtime |
| `#[derive(...)]` | serialization, equality, debug output |
| `#[cfg(test)]` | unit tests compiled only for tests |
| `unsafe` | Win32/FFI boundary with a SAFETY comment |

If you can read these constructs, you can read most of the Rust in this repository. The full Chinese chapter has more examples and line-by-line explanations.
