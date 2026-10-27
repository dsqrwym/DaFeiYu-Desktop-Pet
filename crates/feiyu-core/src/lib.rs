//! FeiYu 桌宠的核心库：与平台无关，全部可在无窗口环境下单测。
//!
//! 约束：本 crate **不得** 出现 `#[cfg(windows)]`、`windows::*` 或任何 Win32 类型；
//! 平台能力一律通过 `feiyu-platform` 的 trait 注入。

#![forbid(unsafe_code)]

pub mod anim;
pub mod brain;

pub use anim::{AnimEntry, AnimError, AnimHeader, AnimReader, Codec, FrameEntry, Manifest};
pub use brain::{Brain, Mood, Stimulus};
