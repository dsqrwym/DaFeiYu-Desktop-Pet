//! 托盘图标（`Shell_NotifyIconW`）的最小封装。
//!
//! 只做四件事：添加、改提示文字、换图标、删除。**不做** `NIM_SETVERSION(NOTIFYICON_VERSION_4)`：
//! v4 会把鼠标坐标搬进 `wparam` 并改变 `lparam` 的编码语义，而我们不需要气球通知
//! 和自定义图标 id。保持 legacy 编码（`lparam` 低字就是鼠标消息）在 Win7~Win11
//! 全都支持，窗口那边的分派能少一半分支。
//!
//! 另外，`Drop` 里必须 `NIM_DELETE`：进程退出时如果不管，托盘上会留一个
//! 「幽灵图标」——点它没反应，直到鼠标划过通知区才消失。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::HICON;

use crate::window::TRAY_MESSAGE;
use crate::{Error, Result};

/// 图标 id。同一窗口只挂一个图标时随便给，但必须在 ADD/MODIFY/DELETE 三处一致。
const UID: u32 = 1;

/// 挂在某个窗口上的托盘图标。
pub struct TrayIcon {
    hwnd: HWND,
    uid: u32,
}

impl TrayIcon {
    /// 添加图标。鼠标事件会以 [`TRAY_MESSAGE`] 投给 `hwnd` 的回调。
    pub fn new(hwnd: HWND, icon: HICON, tip: &str) -> Result<Self> {
        let mut data = data_for(hwnd, tip);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.hIcon = icon;
        // SAFETY: data 是完整的 NOTIFYICONDATAW，指针在调用期间有效。
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            return Err(Error::win32(
                "Shell_NotifyIconW(NIM_ADD)",
                windows::core::Error::from_win32(),
            ));
        }
        Ok(Self { hwnd, uid: UID })
    }

    /// 改鼠标悬停时的提示文字。
    pub fn set_tip(&self, tip: &str) -> Result<()> {
        let mut data = data_for(self.hwnd, tip);
        data.uFlags = NIF_TIP;
        // SAFETY: 同上，data 在调用期间有效。
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            return Err(Error::win32(
                "Shell_NotifyIconW(NIM_MODIFY)",
                windows::core::Error::from_win32(),
            ));
        }
        Ok(())
    }

    /// 换掉通知区里显示的图标。
    ///
    /// **调用方必须「先换这里，再销毁旧图标」**。顺序反过来的话，旧 `HICON` 已经被
    /// `DestroyIcon`，而系统还指着它，通知区会闪一下空白（或直接变成一个空格子，
    /// 直到鼠标划过通知区才恢复）。
    ///
    /// 本函数**不持有**这个 `HICON`；传进来的句柄必须比托盘图标的寿命更长。
    pub fn set_icon(&self, icon: HICON) -> Result<()> {
        let mut data = data_for(self.hwnd, "");
        // 把 `NIF_MESSAGE` 一并带上：换图标同时重新声明回调配对，
        // 免得某些 shell 版本把这条记录的回调字段当成「未提供」而改掉。
        data.uFlags = NIF_ICON | NIF_MESSAGE;
        data.hIcon = icon;
        // SAFETY: data 是完整的 NOTIFYICONDATAW，hIcon 在调用期间有效。
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            return Err(Error::win32(
                "Shell_NotifyIconW(NIM_MODIFY 换图标)",
                windows::core::Error::from_win32(),
            ));
        }
        Ok(())
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let mut data = NOTIFYICONDATAW::default();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = self.uid;
        // SAFETY: 只按句柄撤图标，不读其他字段。
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }
}

/// 填好「句柄 + id + 提示文字」，其余字段留零。
fn data_for(hwnd: HWND, tip: &str) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = UID;
    data.uCallbackMessage = TRAY_MESSAGE;
    // szTip 是定长 128 个 u16，超长部分丢掉（宁可截断，也不越界）。
    let wide: Vec<u16> = tip.encode_utf16().take(127).collect();
    data.szTip[..wide.len()].copy_from_slice(&wide);
    data
}
