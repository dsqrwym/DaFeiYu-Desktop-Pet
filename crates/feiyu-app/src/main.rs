//! 双击入口：GUI 子系统，不创建控制台窗口。
//!
//! 要看 `--selftest` / `--list` 的输出，请用同目录的 `feiyu-cli.exe`：
//! PowerShell / cmd 对 GUI 子系统的程序不会等待，输出会跑到提示符后面，
//! 退出码也拿不到。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    // 从终端启动时把 stdout/stderr 接回父控制台；双击时没有控制台，
    // 就把它们指到 NUL——否则 `println!` 会因写入失败而 panic。
    feiyu_platform::prepare_console();
    feiyu_app::cli_main()
}
