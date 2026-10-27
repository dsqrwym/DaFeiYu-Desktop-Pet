//! 分层窗口（`WS_EX_LAYERED` + `UpdateLayeredWindow`）与消息泵。
//!
//! 为什么不用 GDI 的 `WM_PAINT` 绘制：桌面宠物需要**逐像素透明**，
//! 普通窗口只能整块矩形或色键抠像。`UpdateLayeredWindow` 直接把一张预乘
//! BGRA 位图交给 DWM，能做到发丝级半透明，且不进入 `WM_PAINT` 流程。
//!
//! 已知代价：`UpdateLayeredWindow` 每次提交**整帧**，没有脏矩形优化。
//! 本项目帧尺寸只有窗口大小（≈200×240），一次提交 ≈0.2ms，可以接受。
//!
//! 命中测试走 `WM_NCHITTEST` → `HTTRANSPARENT`：透明区鼠标事件直接穿透到桌面，
//! 只有不透明像素才接收点击。这是「点不到空气」的唯一正确做法。

use std::collections::VecDeque;
use std::sync::Mutex;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, SIZE,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, ScreenToClient,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, RGBQUAD,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, IsWindowVisible, KillTimer, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, GWL_EXSTYLE,
    HTCLIENT, HTTRANSPARENT, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IDC_ARROW, MSG,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA,
    WM_APP, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_RBUTTONUP,
    WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

use crate::{Error, Result};

/// 托盘图标的回调消息（`NOTIFYICONDATAW.uCallbackMessage`）。
///
/// 用 `WM_APP` 之后的区间：这些消息系统不会自作主张发过来。
pub const TRAY_MESSAGE: u32 = WM_APP + 1;

/// 「已有实例被第二次启动」的唤醒消息，见 [`wake`]。
pub const WAKE_MESSAGE: u32 = WM_APP + 2;

/// 窗口类名的宽字符形式。做成函数而不是 `const`：`w!` 宏展开后要知道字符串长度，
/// 写在函数里最不需要琢磨它能不能放进常量求值。
fn window_class() -> PCWSTR {
    windows::core::w!("FeiYuPetWindow")
}

/// 窗口事件。全部在消息泵线程上按顺序交付。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// 定时器到点。
    Tick,
    /// 左键按下（客户区坐标）。
    MouseDown { x: i32, y: i32 },
    /// 鼠标移动（未拖拽时）。
    MouseMove { x: i32, y: i32 },
    /// 左键抬起。
    MouseUp { x: i32, y: i32 },
    /// 右键抬起，用于弹菜单。坐标是**屏幕**坐标（菜单需要屏幕坐标，
    /// 而不是客户区坐标——客户区坐标还得再由调用方换算一次）。
    RightClick { x: i32, y: i32 },
    /// 窗口被拖到了新位置（屏幕坐标，左上角）。
    Moved { x: i32, y: i32 },
    /// 托盘图标上左键单击（约定含义：把桌宠唤出来）。
    TrayLeftClick,
    /// 托盘图标上右键（屏幕坐标，直接可以拿去弹菜单）。
    TrayRightClick { x: i32, y: i32 },
    /// 别的进程又启动了一份，请求把这一只唤出来。
    Woken,
    /// 窗口正在销毁。
    Quit,
}

/// 消息泵线程共享的窗口状态。
///
/// 用 `Mutex` 而不是裸指针：`WM_NCHITTEST` 与主线程的 `draw()` 会并发触碰
/// 像素缓冲，用锁换取「没有数据竞争」这个保证，代价可忽略（每消息一次加锁）。
struct WndState {
    queue: VecDeque<Event>,
    width: i32,
    height: i32,
    /// 每像素 alpha，长度 = `width × height`。
    alpha: Vec<u8>,
    /// alpha ≥ 该值才算命中（0 会让完全透明的像素也吃掉点击）。
    hit_alpha: u8,
    /// 拖拽中：鼠标相对窗口左上角的偏移。
    grab: Option<POINT>,
}

static STATE: Mutex<Option<WndState>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut WndState) -> T) -> Option<T> {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_mut().map(f)
}

fn push(ev: Event) {
    let _ = with_state(|s| s.queue.push_back(ev));
}

/// 命中测试：`(x, y)` 为客户区坐标。
///
/// 状态未就绪时返回 `true`（宁可让窗口可点，也不要让它变成一块死玻璃）。
fn hit_test(x: i32, y: i32) -> bool {
    with_state(|s| {
        if x < 0 || y < 0 || x >= s.width || y >= s.height {
            return false;
        }
        let idx = (y * s.width + x) as usize;
        s.alpha.get(idx).is_some_and(|a| *a >= s.hit_alpha)
    })
    .unwrap_or(true)
}

fn client_point(lparam: LPARAM) -> (i32, i32) {
    // 客户区坐标是打包在 lparam 里的两个有符号 16 位。
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            // NCHITTEST 给的是屏幕坐标，先换算到客户区。
            let (sx, sy) = client_point(lparam);
            let mut pt = POINT { x: sx, y: sy };
            // SAFETY: pt 是栈上有效结构。
            let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
            let hit = hit_test(pt.x, pt.y);
            return LRESULT(if hit {
                HTCLIENT as isize
            } else {
                HTTRANSPARENT as isize
            });
        }
        WM_LBUTTONDOWN => {
            let (x, y) = client_point(lparam);
            // SAFETY: 句柄来自本窗口；POINT/RECT 均为栈上有效内存。
            unsafe {
                let _ = SetCapture(hwnd);
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                let grab = POINT {
                    x: cur.x - rect.left,
                    y: cur.y - rect.top,
                };
                let _ = with_state(|s| s.grab = Some(grab));
            }
            push(Event::MouseDown { x, y });
        }
        WM_MOUSEMOVE => {
            let (x, y) = client_point(lparam);
            let dragging = with_state(|s| s.grab).flatten();
            match dragging {
                Some(grab) => {
                    // SAFETY: GetCursorPos/SetWindowPos 只碰本窗口句柄与栈上 POINT。
                    unsafe {
                        let mut cur = POINT::default();
                        let _ = GetCursorPos(&mut cur);
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            cur.x - grab.x,
                            cur.y - grab.y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                        let mut rect = RECT::default();
                        let _ = GetWindowRect(hwnd, &mut rect);
                        push(Event::Moved {
                            x: rect.left,
                            y: rect.top,
                        });
                    }
                }
                None => push(Event::MouseMove { x, y }),
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = client_point(lparam);
            // SAFETY: 释放本窗口的鼠标捕获。
            unsafe {
                let _ = ReleaseCapture();
            }
            let _ = with_state(|s| s.grab = None);
            push(Event::MouseUp { x, y });
        }
        WM_RBUTTONUP => {
            // 菜单直接吃屏幕坐标，这里不再传客户区坐标。
            // SAFETY: 栈上 POINT 接收光标位置。
            let mut pt = POINT::default();
            let _ = unsafe { GetCursorPos(&mut pt) };
            push(Event::RightClick { x: pt.x, y: pt.y });
        }
        WM_TIMER => push(Event::Tick),
        TRAY_MESSAGE => {
            // 没开 NOTIFYICON_VERSION_4：lparam 低字就是鼠标消息，
            // 坐标得自己取（v4 才会把坐标塞进 wparam）。
            match (lparam.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP => push(Event::TrayLeftClick),
                WM_RBUTTONUP => {
                    // SAFETY: 栈上 POINT 接收光标位置。
                    let mut pt = POINT::default();
                    let _ = unsafe { GetCursorPos(&mut pt) };
                    push(Event::TrayRightClick { x: pt.x, y: pt.y });
                }
                // 双击、移动这些暂不需要：鼠标划过也会来消息，
                // 全转成事件只会让泵空转。
                _ => {}
            }
        }
        WAKE_MESSAGE => push(Event::Woken),
        WM_DESTROY => {
            push(Event::Quit);
            // SAFETY: 结束本线程消息循环。
            unsafe { PostQuitMessage(0) };
        }
        _ => {}
    }
    // SAFETY: 未处理的消息交回默认处理，这是 Win32 的规定动作。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 预乘 BGRA 的 DIB 位图，直接作为 `UpdateLayeredWindow` 的源。
struct Dib {
    bitmap: HBITMAP,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Dib {
    fn new(width: i32, height: i32) -> Result<Self> {
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // 负高度 = 自上而下存储，行序与我们的像素数组一致。
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default()],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: 传入合法的 BITMAPINFO 与接收指针；不共享文件映射。
        let bitmap =
            unsafe { CreateDIBSection(None, &mut info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| Error::win32("CreateDIBSection", e))?;
        Ok(Self {
            bitmap,
            bits: bits as *mut u8,
            width,
            height,
        })
    }

    fn len(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }
}

/// 一个带逐像素 alpha 的顶层窗口。
pub struct LayeredWindow {
    hwnd: HWND,
    dib: Dib,
    mem_dc: HDC,
    old_bitmap: HGDIOBJ,
}

impl LayeredWindow {
    /// 创建窗口（不显示）。`width/height` 是**位图**尺寸，也是窗口客户区尺寸。
    pub fn create(width: u32, height: u32, x: i32, y: i32, title: &str) -> Result<Self> {
        let class = window_class();
        // SAFETY: 取本模块句柄，无需释放。
        let hinstance =
            unsafe { GetModuleHandleW(None) }.map_err(|e| Error::win32("GetModuleHandleW", e))?;

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            // SAFETY: 系统自带光标资源，无需释放。
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            lpszClassName: class,
            ..Default::default()
        };
        // SAFETY: 只读传入已填好的窗口类结构。
        if unsafe { RegisterClassW(&wc) } == 0 {
            // SAFETY: 无参数。
            let err = unsafe { GetLastError() };
            if err != ERROR_CLASS_ALREADY_EXISTS {
                return Err(Error::win32(
                    "RegisterClassW",
                    windows::core::Error::from_win32(),
                ));
            }
        }

        let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: 所有指针参数指向调用期间有效的内存；父窗口/菜单为空。
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class,
                PCWSTR(title_w.as_ptr()),
                WS_POPUP,
                x,
                y,
                width as i32,
                height as i32,
                None,
                None,
                windows::Win32::Foundation::HINSTANCE::from(hinstance).into(),
                None,
            )
        }
        .map_err(|e| Error::win32("CreateWindowExW", e))?;

        let dib = Dib::new(width as i32, height as i32)?;
        // SAFETY: 屏幕 DC 用完即还。
        let screen_dc = unsafe { GetDC(None) };
        // SAFETY: 由屏幕 DC 派生兼容 DC（无失败路径）。
        let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
        // SAFETY: 把 DIB 选进 DC，返回旧对象以便恢复。
        let old_bitmap = unsafe { SelectObject(mem_dc, HGDIOBJ(dib.bitmap.0)) };
        // SAFETY: 与 GetDC 配对。
        unsafe { ReleaseDC(None, screen_dc) };

        let zeroed = vec![0u8; dib.len()];
        let mut window = Self {
            hwnd,
            dib,
            mem_dc,
            old_bitmap,
        };
        // SAFETY: bits 是 DIB 的有效像素区，长度由 len() 保证。
        unsafe { std::ptr::copy_nonoverlapping(zeroed.as_ptr(), window.dib.bits, zeroed.len()) };

        STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(WndState {
                queue: VecDeque::new(),
                width: width as i32,
                height: height as i32,
                alpha: Vec::new(),
                // 0 会让全透明像素也命中；1 起步即可（预乘后 < 1/255 不可见）。
                hit_alpha: 1,
                grab: None,
            });

        // 先把全透明位图提交一次，避免窗口刚显示时是块黑方块。
        window.present()?;
        Ok(window)
    }

    /// 原生句柄（自检/截图用）。
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 显示窗口但不抢焦点。
    pub fn show(&self) {
        // SAFETY: 句柄有效。
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// 隐藏窗口。不动定时器——那是调用方的事（藏起来的桌宠没必要继续烧 CPU）。
    pub fn hide(&self) {
        // SAFETY: 句柄有效。
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// 当前是否可见（自检用）。
    pub fn is_visible(&self) -> bool {
        // SAFETY: 只查本窗口的可见性标志。
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// 从隐藏状态唤出，并保持原来的 z 序归属。
    ///
    /// **不能直接用 [`Self::show`]**：那个会把窗口强行设成 topmost，
    /// 等于把用户关掉的「总在最前」悄悄打开。
    pub fn reveal(&self) {
        // SAFETY: 句柄有效；下面只改可见性与 z 序。
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        if self.is_topmost() {
            self.set_topmost(true);
        } else {
            self.raise();
        }
    }

    /// 抬到**同一层** z 序的顶端：置顶的仍在置顶层，不置顶的仍在普通层。
    ///
    /// 刚关掉别的窗口时想把它叫出来看看，又不希望它从此一路压在其他窗口上面，
    /// 用的就是这一步而不是 `set_topmost(true)`。
    pub fn raise(&self) {
        // SAFETY: 句柄有效；只改 z 序，不动位置与尺寸。
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// 位图尺寸。
    pub fn size(&self) -> (u32, u32) {
        (self.dib.width as u32, self.dib.height as u32)
    }

    /// 屏幕坐标下的窗口矩形 `(left, top, right, bottom)`。
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        let mut rect = RECT::default();
        // SAFETY: 句柄有效，rect 为栈上接收结构。
        unsafe { GetWindowRect(self.hwnd, &mut rect) }.ok();
        (rect.left, rect.top, rect.right, rect.bottom)
    }

    /// 切换「总在最前」。
    ///
    /// 关掉之后窗口会落到普通窗口的 z 序顶端：仍然看得见，但被别的窗口盖住
    /// 就点不到了——这正是这个开关的意义（工作时不想被桌宠压住）。
    pub fn set_topmost(&self, on: bool) {
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        // SAFETY: 句柄有效；只改 z 序，不动位置与尺寸。
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// 当前是否「总在最前」（自检用）。
    pub fn is_topmost(&self) -> bool {
        // SAFETY: 读本窗口扩展样式。
        let style = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32;
        style & WS_EX_TOPMOST.0 != 0
    }

    /// 移动窗口左上角（屏幕坐标，物理像素）。
    pub fn move_to(&self, x: i32, y: i32) {
        // SAFETY: 句柄有效。
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// 启动/重启周期定时器（毫秒）。`ms == 0` 表示停掉。
    pub fn set_tick(&self, ms: u32) {
        // SAFETY: 定时器 id 固定为 1，回调走 WM_TIMER。
        unsafe {
            if ms == 0 {
                let _ = KillTimer(Some(self.hwnd), 1);
            } else {
                let _ = SetTimer(Some(self.hwnd), 1, ms, None);
            }
        }
    }

    /// 提交一整帧预乘 BGRA 像素（尺寸变化时自动重建 DIB）。
    ///
    /// `bgra.len()` 必须等于 `width × height × 4`。
    pub fn draw(&mut self, width: u32, height: u32, bgra: &[u8]) -> Result<()> {
        let (w, h) = (width as i32, height as i32);
        if (w, h) != (self.dib.width, self.dib.height) {
            // 尺寸变了：换 DIB。不同动画的裁切尺寸本来就不同，属正常路径。
            let dib = Dib::new(w, h)?;
            // SAFETY: 换掉旧位图对象并恢复 DC 的选择状态。
            unsafe {
                let old = SelectObject(self.mem_dc, HGDIOBJ(dib.bitmap.0));
                if !old.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(old.0));
                }
                let _ = DeleteObject(HGDIOBJ(self.dib.bitmap.0));
            }
            self.dib = dib;
            let _ = with_state(|s| {
                s.width = w;
                s.height = h;
            });
        }
        debug_assert_eq!(bgra.len(), self.dib.len(), "帧字节数与位图尺寸不符");
        let n = bgra.len().min(self.dib.len());
        // SAFETY: 目标为 DIB 像素区，长度 >= n。
        unsafe { std::ptr::copy_nonoverlapping(bgra.as_ptr(), self.dib.bits, n) };

        // 同步 alpha 给命中测试；单独存一份，避免与绘制路径共享裸指针。
        let _ = with_state(|s| {
            s.alpha.clear();
            s.alpha.extend(bgra.chunks_exact(4).map(|p| p[3]));
        });
        self.present()
    }

    /// 把当前 DIB 提交给 DWM。
    fn present(&mut self) -> Result<()> {
        let size = SIZE {
            cx: self.dib.width,
            cy: self.dib.height,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: 屏幕 DC 只作目标表面；DIB 仍是当前选中的位图。
        unsafe {
            let screen_dc = GetDC(None);
            let r = UpdateLayeredWindow(
                self.hwnd,
                Some(screen_dc),
                None,
                Some(&size),
                Some(self.mem_dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            ReleaseDC(None, screen_dc);
            r.map_err(|e| Error::win32("UpdateLayeredWindow", e))
        }
    }

    /// 当前窗口是否带分层样式（自检用，等价于「真的能逐像素透明」）。
    pub fn is_layered(&self) -> bool {
        // SAFETY: 读本窗口扩展样式。
        let style = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32;
        style & WS_EX_LAYERED.0 != 0
    }

    /// 把用户数据指针挂到窗口上（当前未使用，留给后续窗口关联状态）。
    pub fn set_user_data(&self, ptr: isize) {
        // SAFETY: 写入本窗口的 USERDATA 槽位。
        unsafe { SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, ptr) };
    }

    /// 命中测试（自检用）：`(x, y)` 为客户区坐标，`true` 表示该点会抢鼠标。
    pub fn hit_test(&self, x: i32, y: i32) -> bool {
        hit_test(x, y)
    }
}

impl Drop for LayeredWindow {
    fn drop(&mut self) {
        // SAFETY: 逐个释放本结构独占的 GDI 对象；DC 先恢复再删位图。
        unsafe {
            let _ = SelectObject(self.mem_dc, self.old_bitmap);
            let _ = DeleteObject(HGDIOBJ(self.dib.bitmap.0));
            let _ = DeleteDC(self.mem_dc);
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
        let _ = STATE.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// 找到已经在跑的那一只桌宠的窗口。
///
/// 只按类名找、不限定窗口名：本项目只有一个窗口类，且它带 `WS_EX_TOOLWINDOW`，
/// 不会在 Alt+Tab 里露面，不会和别的程序撞名。
pub fn find_window() -> Option<HWND> {
    // SAFETY: 只读查找，不传窗口名（PCWSTR::null）。
    unsafe { FindWindowW(window_class(), PCWSTR::null()) }.ok()
}

/// 请已经在跑的那个实例把自己唤出来。
///
/// 跨进程投递 `WM_APP` 区间的自定义消息是允许的：接收方是自己的同类窗口，
/// 消息号一致，不需要注册全局消息。
pub fn wake(hwnd: HWND) -> Result<()> {
    // SAFETY: 句柄由 `find_window` 找到，是只读使用；消息只是一个空载荷信号。
    unsafe { PostMessageW(Some(hwnd), WAKE_MESSAGE, WPARAM(0), LPARAM(0)) }
        .map_err(|e| Error::win32("PostMessageW", e))
}

/// 结束消息循环（从事件回调里调用它可安全退出）。
pub fn quit() {
    // SAFETY: 向本线程消息队列投递 WM_QUIT。
    unsafe { PostQuitMessage(0) };
}

/// 阻塞跑消息循环，事件按顺序回调。
///
/// 回调里可以安全调用 [`LayeredWindow`] 的方法——事件出队后才回调，
/// 不会持锁进入用户代码。
pub fn pump(mut on_event: impl FnMut(Event)) -> Result<()> {
    let mut msg = MSG::default();
    loop {
        // SAFETY: msg 为栈上接收结构；取本线程全部消息。
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 {
            return Ok(());
        }
        if ret.0 == -1 {
            return Err(Error::win32(
                "GetMessageW",
                windows::core::Error::from_win32(),
            ));
        }
        // SAFETY: 标准翻译+分发。
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        loop {
            let ev = with_state(|s| s.queue.pop_front()).flatten();
            match ev {
                Some(ev) => on_event(ev),
                None => break,
            }
        }
    }
}
