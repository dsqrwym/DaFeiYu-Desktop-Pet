//! 桌宠的应用层：命令行、组装、自检。
//!
//! 两个可执行入口共用这里的代码：
//!
//! * `feiyu.exe` —— GUI 子系统，双击运行，不弹终端窗口；
//! * `feiyu-cli.exe` —— console 子系统，诊断命令用（`--selftest` / `--list` …），
//!   输出与退出码和终端同步。
//!
//! 拆成 lib + 两个 bin 是被 Windows 的「子系统」逼的：同一个 PE 只有一个子系统，
//! console 子系统双击会弹终端，GUI 子系统又拿不到同步的命令行输出。两份入口、
//! 一份实现，是唯一能同时满足「双击无窗口」和「诊断可脚本化」的做法。

pub mod cli;
pub mod pet;
pub mod selftest;

pub use cli::cli_main;
