//! DPI 感知：必须在创建任何窗口**之前**设置。

use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// 开启每显示器 DPI v2 感知，返回是否成功。
///
/// 为什么必须：默认进程是 DPI-unaware，系统会把窗口位图整块拉伸到缩放比，
/// 结果是 125%/150% 缩放下桌宠边缘发糊、鼠标坐标也对不上。
///
/// 返回 `false` 表示设置被拒（系统过旧或宿主已预设），此时进程按原有感知模式
/// 运行——能用但不推荐，值得在启动日志里记一笔。
pub fn enable_per_monitor_v2() -> bool {
    // SAFETY: 只传一个常量句柄，无指针参数。
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.is_ok()
}
