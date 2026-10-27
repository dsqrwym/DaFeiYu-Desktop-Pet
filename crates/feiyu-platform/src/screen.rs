//! 屏幕几何：桌宠需要知道「站在哪」，而不是写死坐标。

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};

use crate::{Error, Result};

/// 主显示器工作区 `(left, top, right, bottom)`——已排除任务栏，桌宠贴底应贴这里。
///
/// 为什么不用 `GetSystemMetrics(SM_CYSCREEN)`：那会把任务栏算进去，
/// 结果是桌宠有半个身子藏在任务栏后面。
pub fn primary_work_area() -> Result<(i32, i32, i32, i32)> {
    let mut rect = RECT::default();
    // SAFETY: 只写本进程栈上的 RECT；无输入缓冲。
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .map_err(|e| Error::win32("SystemParametersInfoW(SPI_GETWORKAREA)", e))?;
    Ok((rect.left, rect.top, rect.right, rect.bottom))
}
