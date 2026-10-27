//! 系统级输入活动：只回答一个问题——「用户最后一次碰键盘/鼠标是多久以前」。
//!
//! 桌宠睡着时，如果电脑前的人一直在动，它就该有机会被吵醒；
//! 这件事**只能问系统**：我们的窗口没焦点（`WS_EX_NOACTIVATE`），
//! 收不到键盘消息，用户在别的窗口打字也完全轮不到我们。
//!
//! `GetLastInputInfo` 读的是整个会话级共享的输入时间戳，
//! 不需要钩子、不需要权限，也不看谁有焦点。

use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

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
