//! 素材与动画子模块。
//!
//! 当前只有 `.fishanim` 容器（[`format`] 读写格式、[`reader`] 运行时取帧）。
//! 后续阶段在此追加：`manifest`（素材索引）、`state`（状态机）、`physics`（拖拽与抛掷）。

pub mod format;
pub mod reader;

pub use format::{AnimError, AnimFile, AnimHeader, Codec, FrameEntry};
pub use reader::AnimReader;
