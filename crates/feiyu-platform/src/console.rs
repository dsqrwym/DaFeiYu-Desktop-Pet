//! 控制台窗口处理：给 GUI 子系统的 `feiyu.exe` 准备 stdout/stderr。
//!
//! GUI 子系统的进程默认没有控制台。从终端启动时（`feiyu.exe --list`），
//! 我们想把输出接回父控制台；双击启动时没有控制台，就把输出指到 `NUL`，
//! 否则 Rust 的 `println!` 会因为写入失败而 panic。
//!
//! 诊断命令请优先用 `feiyu-cli.exe`（console 子系统）：GUI 子系统从 PowerShell
//! 启动时不会被等待，输出会跑到提示符后面，退出码也拿不到。

use windows::core::w;
use windows::Win32::Foundation::{GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
};

/// 把 stdout/stderr 接到父控制台；没有父控制台就接到 `NUL`。
pub fn prepare_console() {
    // SAFETY: 只改本进程的标准句柄；句柄在进程生命周期内保持有效。
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            if let Some(out) = open(w!("CONOUT$")) {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, out);
                let _ = SetStdHandle(STD_ERROR_HANDLE, out);
                return;
            }
        }
        // 双击启动（没有控制台），或者 CONOUT$ 打不开：输出丢到 NUL。
        if let Some(nul) = open(w!("NUL")) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, nul);
            let _ = SetStdHandle(STD_ERROR_HANDLE, nul);
        }
    }
}

/// 以「写」方式打开一个设备名（`CONOUT$` / `NUL`）。
///
/// # Safety
///
/// 必须在调用 `SetStdHandle` 之后让句柄活到进程结束（`HANDLE` 是 Copy，
/// 这里不关闭它；关闭了标准输出就断了）。
unsafe fn open(name: windows::core::PCWSTR) -> Option<HANDLE> {
    // SAFETY: name 是本模块里的宽字符串字面量；其余参数按 CreateFileW 的要求填。
    unsafe {
        CreateFileW(
            name,
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()
    }
}
