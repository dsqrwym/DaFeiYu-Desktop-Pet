//! 平台层：把 Win32 的「分层窗口 + 逐像素 alpha」封装成最小接口。
//!
//! 这一层是整个项目里**唯一**允许出现 `unsafe` 与 `windows::*` 的地方；
//! `feiyu-core` 保持零平台依赖，将来换 macOS/Linux 只替换本 crate。
//!
//! 已实现的职责：
//! * 每显示器 DPI v2 感知（否则 150% 缩放下窗口会被系统拉伸成糊的）；
//! * 单实例互斥（第二个进程安静退出）；
//! * 具有逐像素 alpha 的分层窗口：`UpdateLayeredWindow` + 预乘 BGRA DIB；
//! * 逐像素命中测试（`WM_NCHITTEST` 返回 `HTTRANSPARENT`，透明区鼠标穿透）；
//! * 拖动、定时器、右键事件；
//! * 原生上下文菜单；
//! * 托盘图标（帧像素现场画成 `HICON`，不依赖 `.ico` 资源）；
//! * 窗口截图（自检取证用，产物是 BMP，零第三方依赖）；
//! * 系统输入活动（最后一次键鼠操作是多久以前）。

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(windows))]
compile_error!("feiyu-platform 目前只实现了 Windows 后端");

pub mod dpi;
pub mod icon;
pub mod input;
pub mod instance;
pub mod menu;
pub mod screen;
pub mod shot;
pub mod tray;
pub mod window;

pub use dpi::enable_per_monitor_v2;
pub use icon::Icon;
pub use input::{cursor_pos, last_input_ms, left_button_down};
pub use instance::SingleInstance;
pub use menu::{CommandId, Menu};
pub use screen::primary_work_area;
pub use tray::TrayIcon;
pub use window::{find_window, pump, quit, wake, Event, LayeredWindow, TRAY_MESSAGE};

/// 平台层错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Win32 调用失败：{context}（{source}）")]
    Win32 {
        context: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    pub(crate) fn win32(context: &'static str, source: windows::core::Error) -> Self {
        Self::Win32 { context, source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
