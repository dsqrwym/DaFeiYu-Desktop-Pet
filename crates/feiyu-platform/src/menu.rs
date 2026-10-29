//! 原生右键菜单。
//!
//! 为什么用系统菜单而不是自绘：`TrackPopupMenu` 自带键盘导航、Esc 关闭、
//! 多显示器与高 DPI 下的定位、以及「点别处自动消失」的全部细节。
//! 自绘这些要写几百行，而且永远不如系统那个用着顺手。
//!
//! 一个必须处理的坑：本项目的窗口是 `WS_EX_NOACTIVATE`（点它不抢焦点，
//! 否则桌宠会把用户正在打字的窗口踢到后台）。而系统的「点别处关菜单」逻辑
//! 依赖前台窗口，所以这里是三件套：弹出前 `SetForegroundWindow`、用
//! `TPM_RETURNCMD` 直接拿 id、关闭后补一条 `WM_NULL`。
//! 缺了最后那条的典型症状是菜单虽然消失了，但下一次点击会先被系统吃掉。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetForegroundWindow, GetMenuItemCount,
    GetMenuStringW, GetSubMenu, GetWindowLongPtrW, GetWindowThreadProcessId, PostMessageW,
    SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenuEx, GWL_EXSTYLE, HMENU, MENU_ITEM_FLAGS,
    MF_BYPOSITION, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_NULL, WS_EX_NOACTIVATE,
};

use crate::{Error, Result};

/// 菜单项 id。含义由调用方定义（本项目见 `feiyu-app::pet`）。
pub type CommandId = u16;

/// 一个弹出的上下文菜单。析构时自动 `DestroyMenu`。
pub struct Menu {
    handle: HMENU,
    items: usize,
}

impl Menu {
    /// 建一个空菜单。
    pub fn new() -> Result<Self> {
        // SAFETY: 无输入参数；失败由封装返回 Err。
        let handle =
            unsafe { CreatePopupMenu() }.map_err(|e| Error::win32("CreatePopupMenu", e))?;
        Ok(Self { handle, items: 0 })
    }

    /// 追加一条普通命令。
    pub fn command(&mut self, id: CommandId, label: &str) -> &mut Self {
        self.push(MF_STRING, id, label);
        self
    }

    /// 追加一条带勾选的命令（勾选状态由调用方每次弹出时按当前状态填入）。
    pub fn check(&mut self, id: CommandId, label: &str, checked: bool) -> &mut Self {
        let flags = if checked {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        self.push(flags, id, label);
        self
    }

    /// 追加一条灰掉（不可点）的命令。
    pub fn disabled(&mut self, id: CommandId, label: &str) -> &mut Self {
        self.push(MF_STRING | MF_GRAYED, id, label);
        self
    }

    /// 追加一个子菜单，`build` 里填它的项。
    ///
    /// 层级不做限制，但实际只用到「动作 → 分类 → 具体动画」三层。
    ///
    /// 所有权要点：子菜单句柄交给父菜单后，父 `DestroyMenu` 会连同子菜单一起销毁。
    /// 所以这里必须 `mem::forget` 掉子菜单的守卫——否则它析构时先销毁一次，
    /// 父菜单析构时再销毁同一个句柄，是未定义行为（实测会弹菜单崩）。
    pub fn submenu(&mut self, label: &str, build: impl FnOnce(&mut Menu)) -> Result<&mut Self> {
        let mut sub = Menu::new()?;
        build(&mut sub);
        let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: wide 在调用期间有效；子菜单句柄由 sub 独占且随后转交父菜单。
        unsafe {
            let _ = AppendMenuW(
                self.handle,
                MF_POPUP,
                sub.handle.0 as usize,
                PCWSTR(wide.as_ptr()),
            );
        }
        std::mem::forget(sub);
        self.items += 1;
        Ok(self)
    }

    /// 追加分隔线。
    pub fn separator(&mut self) -> &mut Self {
        // SAFETY: 分隔线不需要文字与 id，传空指针是文档允许的写法。
        unsafe {
            let _ = AppendMenuW(self.handle, MF_SEPARATOR, 0, PCWSTR::null());
        }
        self.items += 1;
        self
    }

    /// 已追加的项数（分隔线也计入）。
    pub fn items(&self) -> usize {
        self.items
    }

    /// **Win32 实际**顶层项数（自检用）。
    ///
    /// 与 [`Self::items`] 分开：后者是我们自己数的一本账，前者问系统。
    /// 两者不一致就说明 AppendMenuW 失败过（比如句柄用错），而那类失败
    /// 在代码里是「忽略返回值」的，不主动查就永远发现不了。
    pub fn api_items(&self) -> usize {
        Self::count_of(self.handle)
    }

    /// 任意菜单句柄的项数（含子菜单——子菜单不归调用方所有，不能包成 `Menu`，
    /// 否则 Drop 会把它销毁两次）。
    pub fn count_of(hmenu: HMENU) -> usize {
        // SAFETY: 句柄由调用方保证有效（直接来自本模块的 GetAll）。
        let n = unsafe { GetMenuItemCount(Some(hmenu)) };
        n.max(0) as usize
    }

    /// 本菜单第 `pos` 项的文字（自检用）。
    pub fn label(&self, pos: i32) -> String {
        Self::label_of(self.handle, pos)
    }

    /// 任意菜单句柄第 `pos` 项的文字（自检用：确认分类标题没写空）。
    pub fn label_of(hmenu: HMENU, pos: i32) -> String {
        let mut buf = [0u16; 128];
        // SAFETY: buf 足够大；GetMenuStringW 返回实际拷入的字符数。
        // 必须带 `MF_BYPOSITION`：不带时 `pos` 会被当成命令 id，
        // 于是传 6 拿到的是「点哪走哪」（CMD_CLICK_WALK），而不是第 7 项。
        let n = unsafe { GetMenuStringW(hmenu, pos as u32, Some(&mut buf[..]), MF_BYPOSITION) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// 第 `pos` 项的子菜单（不是子菜单就返回 `None`）。自检用。
    pub fn api_submenu(&self, pos: i32) -> Option<HMENU> {
        Self::submenu_of(self.handle, pos)
    }

    /// 任意菜单句柄第 `pos` 项的子菜单。
    pub fn submenu_of(hmenu: HMENU, pos: i32) -> Option<HMENU> {
        // SAFETY: 同上；GetSubMenu 对越界/非子菜单返回 NULL。
        let h = unsafe { GetSubMenu(hmenu, pos) };
        (!h.0.is_null()).then_some(h)
    }

    /// 在屏幕坐标 `(x, y)` 弹出并等待选择。
    ///
    /// 返回被点中的 id；用户点了别处、按 Esc、或窗口先没了，都返回 `None`。
    /// 本函数内部跑模态消息循环，期间窗口定时器照常触发（动画不会卡住）。
    pub fn show_at(&self, hwnd: HWND, x: i32, y: i32) -> Result<Option<CommandId>> {
        // SAFETY: 句柄来自 CreatePopupMenu；hwnd 由调用方保证有效；
        // TPMPARAMS 传 None 表示不做范围限制。
        unsafe {
            // 见模块注释：NOACTIVATE 窗口得自己顶到前台。
            //
            // 但光调 `SetForegroundWindow` 不够：带 `WS_EX_NOACTIVATE` 的窗口
            // **永远成不了前台窗口**（这不是 bug，是那个扩展样式的定义）。
            // 而 `TrackPopupMenu` 在窗口不是前台时会**立即取消、返回 0**、
            // 菜单一帧都不显示——实测日志就是 `菜单返回：Ok(None)`。
            // 所以弹之前临时把这个位摘掉，弹完立刻还原：平时点鱼仍然不抢焦点。
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
            // 只调 `SetForegroundWindow` 还不够：系统有「前台锁」，别的进程正
            // 在前台时这个请求会被默默丢弃（返回 false，菜单依旧立刻取消）。
            // 标准做法是把本线程的输入队列暂时挂到当前前台线程上，改完再拆了。
            let fg = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != 0
                && fg_thread != me
                && AttachThreadInput(me, fg_thread, true).as_bool();
            let _ = SetForegroundWindow(hwnd);
            let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON;
            let picked = TrackPopupMenuEx(self.handle, flags.0, x, y, hwnd, None);
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
            // 无论菜单怎么结束，都要把样式还原——漏了这一步，桌宠从此会抢焦点。
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
            // 没选就是 0；TPM_RETURNCMD 下返回值直接是 id。
            Ok((picked.0 > 0).then_some(picked.0 as CommandId))
        }
    }

    fn push(&mut self, flags: MENU_ITEM_FLAGS, id: CommandId, label: &str) {
        // 转成 UTF-16 并补结尾 0：AppendMenuW 要求以 NUL 结尾的宽字符串。
        let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: wide 在本次调用期间有效；菜单句柄来自本结构。
        // 失败只可能是句柄/参数这类编程错误，这里没有可恢复的路径，忽略即可
        // （菜单少一项比整个程序报错退出要好）。
        unsafe {
            let _ = AppendMenuW(self.handle, flags, id as usize, PCWSTR(wide.as_ptr()));
        }
        self.items += 1;
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        // SAFETY: 句柄由本结构独占，只销毁一次。
        unsafe {
            let _ = DestroyMenu(self.handle);
        }
    }
}
