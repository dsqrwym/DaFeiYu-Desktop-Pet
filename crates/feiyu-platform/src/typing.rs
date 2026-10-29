//! 键盘输入计数：只回答一个问题——「用户刚才敲了几下键盘」。
//!
//! 桌宠窗口是 `WS_EX_NOACTIVATE`（点它不抢焦点），所以**收不到** `WM_KEYDOWN`：
//! 用户在别的窗口打字，消息根本轮不到我们。`GetLastInputInfo` 又只给「最后一次
//! 输入是多久以前」，分不出键盘还是鼠标。
//!
//! 这里用 **Raw Input + `RIDEV_INPUTSINK`**：不需要全局钩子、不注入别的进程，
//! 窗口不在前台也能收到 `WM_INPUT`。我们只数**按下次数**，不记录按了哪个键——
//! 「用户在打字」这个信号够了，记键值既没用又越界。
//!
//! 计数是进程级的原子量，由 `wndproc` 写、`Pet::tick` 取走。

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::RI_KEY_BREAK;

use crate::{Error, Result};

/// 自上次取走以来按下过多少个键。
static KEYS: AtomicU32 = AtomicU32::new(0);
/// 当前是否登记了键盘原始输入。
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// 取走并清零按键计数。`tick` 每拍调一次；不取走会一直累积。
pub fn typing_keys() -> u32 {
    KEYS.swap(0, Ordering::Relaxed)
}

/// 是否已经登记成功（自检用）。
pub fn typing_installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// 记一次键盘按下。
///
/// 用 `fetch_add` 而不是 `fetch_update`：后者在本工具链已改名（`try_update`），
/// 而新名字老编译器没有，会破 `rust-version = "1.80"` 的约定。
/// 回绕在实际中不可能发生：`tick` 每 8ms 取走一次，要溢出得两帧内敲 40 亿个键。
pub(crate) fn note_key_down() {
    KEYS.fetch_add(1, Ordering::Relaxed);
}

/// 给窗口登记键盘原始输入（后台也收）。
///
/// 失败不致命：拿不到键盘计数只是「打字联动」不生效，桌宠其余功能照常。
pub fn enable_typing(hwnd: HWND) -> Result<()> {
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x01, // Generic Desktop Controls
        usUsage: 0x06,     // Keyboard
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    // SAFETY: device 是栈上有效结构；cbsize 按 API 要求填 RAWINPUTDEVICE 的大小。
    unsafe {
        RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
            .map_err(|e| Error::win32("RegisterRawInputDevices", e))?;
    }
    INSTALLED.store(true, Ordering::Relaxed);
    Ok(())
}

/// 注销键盘原始输入。窗口析构时调一次；没登记过也不会报错。
pub fn disable_typing() {
    if !INSTALLED.swap(false, Ordering::Relaxed) {
        return;
    }
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: HWND(std::ptr::null_mut()),
    };
    // SAFETY: 同上；RIDEV_REMOVE 时 hwndTarget 必须为 NULL。
    unsafe {
        let _ = RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// `wndproc` 收到 `WM_INPUT` 时调用：只数键盘的「按下」。
///
/// 抬起（`RI_KEY_BREAK`）不算：一次按键会来两条消息，都数会把热度翻倍。
/// 按住不放产生的自动重复会被算作按键——那也确实是「人坐在键盘前」。
pub(crate) fn handle_raw_input(lparam: LPARAM) {
    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    // SAFETY: raw 是栈上有效缓冲；size 初值是缓冲大小，API 会回写实际大小。
    let got = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut c_void),
            RID_INPUT,
            Some(&mut raw as *mut RAWINPUT as *mut c_void),
            &mut size,
            header_size,
        )
    };
    if got == u32::MAX || got == 0 {
        return;
    }
    if raw.header.dwType != RIM_TYPEKEYBOARD.0 {
        return;
    }
    // SAFETY: dwType 已确认是键盘，`data` 联合体的 keyboard 分支有效。
    let flags = unsafe { raw.data.keyboard.Flags };
    if u32::from(flags) & RI_KEY_BREAK == 0 {
        note_key_down();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数要能取走并清零；这里只测纯逻辑，不碰 Win32 登记。
    #[test]
    fn keys_are_counted_and_drained() {
        let _ = typing_keys(); // 清掉别的测试可能留下的数
        note_key_down();
        note_key_down();
        assert_eq!(typing_keys(), 2);
        assert_eq!(typing_keys(), 0, "取走后必须清零");
    }
}
