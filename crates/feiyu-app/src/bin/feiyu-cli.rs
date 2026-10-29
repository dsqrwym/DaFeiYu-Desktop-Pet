//! 诊断入口：console 子系统。
//!
//! 与 `feiyu.exe` 跑的是同一份 [`feiyu_app::cli_main`]，区别只是本程序带控制台：
//! `feiyu-cli.exe --selftest` 的输出和退出码都能直接被脚本/CI 使用。

fn main() -> std::process::ExitCode {
    feiyu_app::cli_main()
}
