//! 单实例互斥：防止用户双击两次出现两只桌宠各跑一套物理。

use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;

use crate::{Error, Result};

/// 持有期间进程独占一个命名互斥体；`Drop` 时自动释放。
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// 尝试成为唯一实例。
    ///
    /// `Ok(None)` 表示已有实例在跑——调用方应当安静退出，
    /// 而不是弹错误框（用户双击两次是常态，不是故障）。
    pub fn acquire(name: &str) -> Result<Option<Self>> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` 是以 NUL 结尾的宽字符串，在调用期间保持存活。
        let handle = unsafe { CreateMutexW(None, false, windows::core::PCWSTR(wide.as_ptr())) }
            .map_err(|e| Error::win32("CreateMutexW", e))?;
        // SAFETY: 无参数读取当前线程的 last error。
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        Ok(if already { None } else { Some(Self { handle }) })
    }

    /// 默认的互斥体名（带 `Local\` 前缀：同一登录会话内唯一，不跨快速用户切换）。
    pub const DEFAULT_NAME: &'static str = "Local\\FeiYuPet.SingleInstance";
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构独占，只关闭一次。
        let _ = unsafe { CloseHandle(self.handle) };
    }
}
