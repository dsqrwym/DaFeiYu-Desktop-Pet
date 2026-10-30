# 09 Exercises and next steps

English | [简体中文](../09-练习与下一步.md)

These exercises are ordered by risk. Do them one at a time and run the relevant tests after each one.

## Exercise 1: change a configuration value

Copy `config.example.json` to `config.json`, change `action_gap` from `[8, 15]` to `[2, 4]`, restart, and observe. Then explain why the value is a product decision.

## Exercise 2: move an existing clip to another category

Edit `CATEGORIES` in `pet/clips.rs`, run `--list`, and verify the category and random weight. Do not change the clip id.

## Exercise 3: add an animation end to end

1. Add the Chinese name → ASCII slug mapping in `pack-anim`.
2. Add the clip to `CATEGORIES`.
3. Add a `ClipSpec` if it needs a span, walk, or entry action.
4. Pack H.264 assets.
5. Run `--list`, `--showcase <slug>`, and `--selftest`.

## Exercise 4: add a self-test check

Pick a behavior with a real Win32 boundary, such as menu placement or tray update. Add a check that fails when the behavior regresses. Do not test a pure function here; put that in `cargo test`.

## Exercise 5: add a unit test

Choose a pure function in `feiyu-core` and add a property test. Prefer boundaries and malformed input over a happy-path snapshot.

## Exercise 6: shorten the click-response duration

First measure the clip and decide which frames form the actual response. Then add a span in `clips.rs`. Do not cut frames blindly; verify with `--showcase` and the self-test.

## Exercise 7: add a menu toggle

Copy the typing-toggle pattern: state in `Pet` or `Config`, menu item, command id, action handling, and a test. Keep command ids stable.

## Exercise 8: add a configuration field

Add a serde field with a default, validate/clamp its range, document it, and add a parse test. Remember that an explicit bad config is an error, while a broken auto-discovered config falls back to defaults.

## Exercise 9: continue splitting `pet/mod.rs`

Propose the split first: renderer, event handling, animation cache, or geometry. Move one responsibility at a time and prove behavior unchanged with tests and the self-test. Do not split by line count alone.

## Known gaps

| Gap | Status | Next step |
|---|---|---|
| H.264 edge quality | CRF20 still shows blocks at 4× zoom; alpha is exact via 1-bit mask | Accept the 71MB tier or try VP9 alpha |
| H.264 compatibility | `VideoReader` currently outputs RGB32 only | Add NV12 fallback if a target system lacks RGB32 |
| H.264 runtime cost | 55MB RSS / 40 threads / 7.9% of one core visible | Reduce decoder/cache count or use legacy `.fishanim` |
| Full visual showcase | 100/100 automated run completed | Human review of each category |
| Click-response length | about 10 seconds | Exercise 6 |
| `pet/mod.rs` size | about 1,600 lines | Exercise 9 |

Both release packages contain 100 animations and pass self-test 94/94. The legacy package is the lower-runtime-resource choice; H.264 is the smaller-download choice.

## Final note

This documentation is not “read once and know Rust”. It shows what a real Rust project looks like and how to work on it. The real progress comes from: change one small thing → run tests → inspect logs/screenshots → record why. Every odd constant in this project came from a real bug or a measured trade-off.

Tray-click support requires the v4 notification protocol and is intentionally out of scope.
