//! 系统级输入活动：只回答一个问题——「用户最后一次碰键盘/鼠标是多久以前」。
//!
//! 桌宠睡着时，如果电脑前的人一直在动，它就该有机会被吵醒；
//! 这件事**只能问系统**：我们的窗口没焦点（`WS_EX_NOACTIVATE`），
//! 收不到键盘消息，用户在别的窗口打字也完全轮不到我们。
//!
//! `GetLastInputInfo` 读的是整个会话级共享的输入时间戳，
//! 不需要钩子、不需要权限，也不看谁有焦点。

use windows::Win32::Foundation::POINT;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, LASTINPUTINFO, VK_LBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// 鼠标左键现在是按着的吗。
///
/// 用于「点桌面某个位置让它走过去」：点桌面**不会**产生我们窗口的消息，
/// 只能靠轮询看按键状态。按 8ms 一拍算，人手一次点击 ≥ 30ms，不会漏。
pub fn left_button_down() -> bool {
    // SAFETY: 只读全局按键状态，无参数。
    (unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000) != 0
}

/// 鼠标当前在哪（屏幕像素）。取不到就返回 `None`。
///
/// 「跟随鼠标」要的是**位置**，跟 `last_input_ms` 的「多久没动」不一样。
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut pt = POINT::default();
    // SAFETY: pt 是栈上有效结构。
    if unsafe { GetCursorPos(&mut pt) }.is_ok() {
        Some((pt.x, pt.y))
    } else {
        None
    }
}

/// 距用户最后一次键盘/鼠标操作过去了多少毫秒。
///
/// 失败（极少见）返回 `None`，调用方据此照常让鱼睡——叫不醒不该变成报错。
pub fn last_input_ms() -> Option<u32> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: info 是栈上有效结构，cbSize 已按 API 要求填好。
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return None;
    }
    // SAFETY: 无参数，只读系统 tick 计数。
    let now = unsafe { GetTickCount() };
    // 两个都是 32 位 tick，系统连开 49 天就会回绕：必须用 wrapping_sub，
    // 否则回绕那一刻会算出一个几十亿毫秒的负数（转 u32 后变成巨大值），
    // 于是「人一直在动」被误判成「一直没人动」，鱼再也叫不醒。
    Some(now.wrapping_sub(info.dwTime))
}
