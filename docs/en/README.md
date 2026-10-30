# Teaching documentation (English edition)

English | [简体中文](../README.md)

This documentation is about one real project: **FeiYu / DaFeiYu Desktop Pet**. It is not a generic Rust tutorial with a toy example. It walks through requirements, architecture, source files, tests, and the mistakes that were actually made.

The Chinese chapters are the full line-by-line reference. The English chapters are **condensed translations** written for readers who do not know Rust; they keep the important commands, code paths, and design decisions, while the Chinese edition keeps every file and function detail.

## Reading order

Do not skip chapters:

| # | Chapter | After reading you should be able to |
|---|---|---|
| 01 | [Getting started](01-getting-started.md) | Download a release or build from source; run tests and the self-test |
| 02 | [Rust syntax map](02-rust-syntax-map.md) | Read most Rust in this project; understand ownership, borrowing, and `Result` |
| 03 | [Architecture and one click](03-architecture.md) | Trace a click from a window message to a screen pixel |
| 04 | [Core, file by file](04-core-files.md) | Understand the animation container, manifest, state machine, physics, and sleep choreography |
| 05 | [Platform, file by file](05-platform-files.md) | Understand the layered window, menu, tray, icons, input, and Media Foundation |
| 06 | [App, file by file](06-app-files.md) | Understand how `pet/` assembles everything |
| 07 | [pack-anim toolchain](07-pack-anim.md) | Follow webm → `.fishanim` or `.fishvid + .fishmask` |
| 08 | [Engineering method](08-engineering.md) | Turn a requirement into tested, reviewable work |
| 09 | [Exercises and next steps](09-exercises-and-next-steps.md) | Add an animation, change behavior, or extend the self-test independently |

## How the teaching works

An 8,000-line project does not benefit from comments such as “this line is a left brace”. The method is:

1. **Teach syntax once.** Ownership, `?`, `match`, and borrowing are explained in chapter 02 using real project code.
2. **Cover every file.** The Chinese 04–07 cover every `.rs` file under `crates/` and `tools/`.
3. **Cover every function.** Inputs, outputs, important branches, and why the function exists.
4. **Explain the critical lines.** The diff chain, hit testing, menu setup, and sleep seam are broken down line by line in the Chinese edition.
5. **Treat tests as code.** Tests document what the author believes must always be true.

## Companion material

- [README](../../README.md) — downloads, build commands, package comparison, and measured resource usage.
- [NOTICE](../../NOTICE.md) — code license, asset license, credits, and the unofficial-project disclaimer.
- Every module starts with a `//!` comment explaining why the file exists; those comments are often more useful than the code itself.
