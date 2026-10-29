//! 素材与动画子模块。
//!
//! * [`format`] —— `.fishanim` 二进制容器读写；
//! * [`reader`] —— 运行时随机取帧；
//! * [`manifest`] —— 素材清单，把动画 id 映射到文件与画布几何。
//! * [`mask`] —— `.fishmask` 1-bit alpha 掩码。
//! * [`crate::brain`] —— 纯逻辑状态机，决定该播哪段动画。

pub mod format;
pub mod manifest;
pub mod mask;
pub mod reader;

pub use format::{AnimError, AnimFile, AnimHeader, Codec, FrameEntry};
pub use manifest::{AnimEntry, Canvas, ClipCodec, Manifest};
pub use mask::{MaskError, MaskSet};
pub use reader::AnimReader;
