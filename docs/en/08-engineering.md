# 08 Engineering method

English | [简体中文](../08-工程方法.md)

This chapter is about how a requirement becomes a deliverable, not about a particular Rust feature.

## 1. The loop

A healthy change loop:

1. state the user-visible behavior;
2. find the smallest layer that owns it;
3. write a regression test for a bug, or a property test for new logic;
4. implement;
5. run unit tests, clippy, fmt, and the real-Windows self-test when the UI is affected;
6. record the evidence and the trade-off;
7. commit one logical change.

Do not start by editing the largest file. Start by deciding which layer should own the change; architecture mistakes are more expensive than syntax mistakes.

## 2. Three levels of evidence

| Level | Tool | Proves |
|---|---|---|
| 1 | `cargo test` | pure logic: codec, state machine, physics, configuration |
| 2 | `--selftest` | real Win32: window, transparency, hit testing, menu, tray |
| 3 | probe script / recording | timing, memory, CPU, visual output, edge behavior |

A unit test cannot prove that a window is visible. A self-test cannot prove that a color is aesthetically acceptable. Use the right evidence for the claim.

### Test value

- Assert properties that must always hold, not today's incidental output.
- Every fixed bug gets a regression test.
- Bad input and boundary values are part of the contract.
- A test that cannot fail is not a test.

## 3. Git habits

- one commit = one coherent change;
- commit messages explain why, not just what;
- do not move a published tag; make a new patch release;
- never commit `target/`, `dist/`, `assets/`, `_upstream/`, or local config;
- check `git status` and `git diff --check` before committing;
- do not push secrets or credentials.

## 4. Performance measurement

### Diagnostic

State what you measured and how:

```text
RSS: WorkingSet64 after 5s warm-up + 30s sample
CPU: TotalProcessorTime delta / wall time
Threads: Process.Threads.Count
```

### Change

Make one change at a time. Record the before/after numbers and the environment.

### Evidence

Keep the script and raw output. “It feels faster” is not a measurement. Separate H.264 and `.fishanim` numbers; do not average them.

## 5. Debugging: message vs behavior

When the pet “does nothing”, first split the problem:

1. **Did the message arrive?** Use `FEIYU_EVENT_TRACE=1`.
2. **Did the state machine change?** Use `--verbose` or a trace switch.
3. **Did the window receive new pixels?** Use `--shot` / `--scrub`.
4. **Did DWM composite them?** Capture the screen, not just the internal buffer.

This prevents hours of debugging the wrong layer. A black screenshot can mean the desktop is not composited, not that the renderer is broken.

## 6. Review checklist

- Is the change in the correct crate?
- Does it add or update a test?
- Are errors contextual and actionable?
- Are unsafe blocks justified with `SAFETY`?
- Are paths and manifest fields validated?
- Does it change a public contract or a generated asset?
- Are both codecs still supported?
- Did the docs and measurements get updated?
- Is there a rollback path?

## 7. Ten rules

1. Make the smallest change that proves the point.
2. Put pure logic in `core`.
3. Measure before optimizing.
4. Treat tests as executable requirements.
5. Never trust an old binary after a code change.
6. Keep evidence for every performance claim.
7. Separate resource numbers by codec/package.
8. Do not move published tags.
9. Do not commit generated assets.
10. Write down why, not just how.

## 8. Acceptance

- Explain the three evidence levels.
- Turn a bug report into a regression test.
- Measure RSS/CPU without mixing package formats.
- Review a change for architecture, tests, errors, and documentation.
