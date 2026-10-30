# 01 Getting started

English | [简体中文](../01-先跑起来.md)

This chapter only gets the project running and explains what each command does. Do not read the source before it runs; otherwise every error feels like something you broke.

## 1. Choose one way to run

### Option A: download a Release (recommended, no Rust)

Download one of the v0.1.0 packages:

- H.264, recommended: <https://github.com/dsqrwym/DaFeiYu-Desktop-Pet/releases/download/v0.1.0/feiyu-h264-v0.1.0.zip>
- Legacy `.fishanim`: <https://github.com/dsqrwym/DaFeiYu-Desktop-Pet/releases/download/v0.1.0/feiyu-fishanim-v0.1.0.zip>

Extract the zip, enter `feiyu/`, and double-click `feiyu.exe`. `feiyu-cli.exe` is the console diagnostics build. See the [main README](../../README.md) for the SHA256 values and the package comparison.

### Option B: build from source

Check the toolchain:

```powershell
rustc -V          # stable, 1.80 or newer
cargo -V
rustup show       # active toolchain should be stable-x86_64-pc-windows-msvc
ffmpeg -version   # needed only when packing assets
```

Without Rust, install [rustup](https://rustup.rs/). It will prompt for the **Visual Studio C++ build tools**, which provide `link.exe`. This is required: Rust on Windows uses the MSVC toolchain by default.

```powershell
git clone https://github.com/dsqrwym/DaFeiYu-Desktop-Pet.git
cd DaFeiYu-Desktop-Pet
powershell -ExecutionPolicy Bypass -File tools/fetch-assets.ps1

# Production format: H.264 + 1-bit mask (about 2–3 minutes)
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/h264 --codec h264 --crf 20

# Optional legacy format
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim

cargo run -p feiyu-app --release
```

`fetch-assets.ps1` shallow-clones the upstream into `_upstream/`. Neither `_upstream/` nor `assets/` is committed. The first build compiles all dependencies and can take a few minutes; later builds are much faster.

`-p` means “package” in Cargo. The binary itself is named `feiyu` (see `crates/feiyu-app/Cargo.toml`), not `feiyu-app`.

## 2. Repository layout

```
FeiYu/
├── Cargo.toml            workspace: five crates (runtime, packer, probe)
├── rust-toolchain.toml   stable + MSVC target
├── crates/
│   ├── feiyu-core/       pure logic, no Windows APIs, unit-testable
│   ├── feiyu-platform/   the only layer allowed to use unsafe / windows::*
│   └── feiyu-app/        Pet, CLI, two entry points, self-test
├── tools/
│   ├── pack-anim/        webm → .fishanim or .fishvid + .fishmask
│   ├── mf-probe/         H.264 decode probe (not part of the runtime)
│   └── fetch-assets.ps1  shallow-clone the upstream
├── assets/               generated; not committed
├── docs/                 this documentation
└── _upstream/            local upstream clone; not committed
```

The crates are split so the compiler can prevent the pure-logic layer from silently using Windows APIs. `feiyu-core` has no `windows` dependency and must compile anywhere.

## 3. Tests and the real-Windows self-test

```powershell
cargo test --workspace --release          # 115 unit tests
.\target\release\feiyu-cli.exe --selftest # 94 real-window checks
```

| | Unit tests | Self-test |
|---|---|---|
| Environment | `cargo test`, no window | Real layered window, real bitmap submission, real screen capture |
| What it covers | Codec, state machine, physics, sleep seams | Transparency, hit testing, menu, tray, baseline alignment |
| CI-friendly | Yes | No; needs a desktop session and assets |

`--selftest` runs `target\release\feiyu-cli.exe`; `cargo test` does not rebuild it. Run `cargo build --release` after changing code, or you will test an old binary.

Windows PE files have only one subsystem. `feiyu.exe` is GUI-subsystem (no terminal on double-click); `feiyu-cli.exe` is console-subsystem (synchronous output and exit codes). Both call the same `cli_main`.

## 4. First configuration change

Start with a parameter that does not require recompilation:

1. Copy `config.example.json` to `config.json` next to the executable or in the repository root.
2. Change `action_gap` from `[8, 15]` to `[2, 4]`.
3. Run `cargo run -p feiyu-app --release` and watch the pet change idle actions every few seconds.
4. Change it back.

`action_gap` is a product decision, not a technical limit: a pet that constantly fidgets feels anxious. Parameters live in `feiyu-core/src/config.rs`; missing fields and invalid values fall back to defaults instead of preventing startup.

## 5. Common failures

| Symptom | Cause | Fix |
|---|---|---|
| `link.exe not found` / `LINK : fatal error` | MSVC C++ tools missing | Re-run rustup, modify the toolchain, install MSVC and the Windows SDK |
| `读不了素材清单 ...manifest.json` | Assets were not packed | Run the `pack-anim` step |
| `打不开动画 ...` | Incomplete/corrupt asset directory | Re-run pack-anim |
| `无法启动 ffmpeg` | ffmpeg is not on `PATH` | Add its `bin` directory to `PATH` or pass `--ffmpeg` |
| The pet is not visible | It is at the bottom center, perhaps behind the taskbar, or not topmost | Find it from the tray/menu; check logs |
| Code changes have no effect | Testing an old executable | `cargo build --release` again |
| Self-test fails while unit tests pass | The self-test checks real Win32 behavior | Inspect the failed item; often DPI, tray, or assets |

## 6. Trace switches

```powershell
$env:FEIYU_EVENT_TRACE=1     # window events, menu results, timing
$env:FEIYU_SEAM_TRACE=1      # animation seam readings
$env:FEIYU_DRAG_TRACE=1      # press → drag pose latency
$env:FEIYU_WAKE_TRACE=1      # natural-wake rolls
$env:FEIYU_TYPING_TRACE=1    # typing-link attempts
$env:FEIYU_TYPING_EAGER=1    # force typing triggers (verification only)
```

These are investigation tools, not features. Remove them with `Remove-Item Env:FEIYU_EVENT_TRACE`.

## 7. Acceptance

- `cargo test --workspace --release` is green.
- `feiyu-cli.exe --selftest` is 94/94.
- The pet is visible and the right-click menu opens.
- You can explain what `-p` means in `cargo run -p feiyu-app`.
- You know why `--selftest` may run a stale binary.

Then continue with [chapter 02](02-rust-syntax-map.md).
