//! FeiYu 桌宠的核心库：与平台无关，全部可在无窗口环境下单测。
//!
//! 约束：本 crate **不得** 出现 `#[cfg(windows)]`、`windows::*` 或任何 Win32 类型；
//! 平台能力一律通过 `feiyu-platform` 的 trait 注入。

#![forbid(unsafe_code)]
// MSRV 1.80：clippy 建议的 `as_chunks`（1.88 稳定）和 `is_multiple_of`（1.87 稳定）
// 都还不在最低支持版本里。这里保留手写的 `chunks_exact` / `%`，等 MSRV 提上去再改。
#![allow(clippy::chunks_exact_to_as_chunks, clippy::manual_is_multiple_of)]

pub mod anim;
pub mod brain;
pub mod config;
pub mod motion;
pub mod pool;
pub mod rng;
pub mod sleep;

pub use anim::{
    AnimEntry, AnimError, AnimHeader, AnimReader, ClipCodec, Codec, FrameEntry, Manifest,
    MaskError, MaskSet,
};
pub use brain::{Brain, Mood, Stimulus};
pub use config::{Config, Typing as TypingConfig, Walk as WalkConfig};
pub use motion::{fling_velocity, Limits, Motion};
pub use pool::Pool;
pub use rng::Rng;
pub use sleep::{span_to_frames, SleepCycle, SleepStage};
