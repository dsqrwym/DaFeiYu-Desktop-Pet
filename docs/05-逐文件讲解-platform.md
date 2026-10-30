# 05 逐文件讲解：feiyu-platform（Win32 层）

> [English](en/05-platform-files.md) | 简体中文

这一层是整个项目**唯一**允许 `unsafe` 与 `windows::*` 的地方。它的目标不是“封装所有
Win32”，而是只封装项目用得到的那几件事，并且把每个 `unsafe` 的成立条件写清楚。

```
crates/feiyu-platform/src/
├── window.rs    分层窗口 + DIB + wndproc + 消息泵
├── menu.rs      原生右键菜单（含子菜单）
├── tray.rs      托盘图标（Shell_NotifyIconW）
├── icon.rs      帧像素 → HICON（预乘→直通 alpha）
├── input.rs     最后一次键鼠输入、光标位置、左键状态
├── typing.rs    键盘原始输入计数（RIDEV_INPUTSINK）
├── video.rs     H.264 解码（Media Foundation）
├── shot.rs      屏幕抓图 → BMP
├── screen.rs    主屏工作区
├── dpi.rs       DPI 感知
└── instance.rs  单实例互斥
```

---

## 1. `window.rs`：分层窗口

### 1.1 为什么用 `UpdateLayeredWindow`

普通窗口只能整块矩形或色键抠像；桌宠要**发丝级半透明**（头发、裙边）。
`WS_EX_LAYERED` + `UpdateLayeredWindow` 把一张**预乘 BGRA** 位图交给 DWM，
DWM 负责逐像素合成。代价是每帧提交整张位图（没有脏矩形），本项目 ~200×240
一帧约 0.2ms，可以接受。

### 1.2 结构

```rust
pub struct LayeredWindow {
    hwnd: HWND,
    dib: Dib,
    mem_dc: HDC,
    old_bitmap: HGDIOBJ,
}
```

- `dib`：一张 32bpp 的 DIB，`bits` 指向像素区；
- `mem_dc`：兼容 DC，DIB 选进去；
- `old_bitmap`：`SelectObject` 返回的旧对象，`Drop` 时要还回去。

字段顺序有意义：`Drop` 按声明顺序执行，所以 `dib` 先于 `mem_dc` —— 不行，
代码里是手动按“恢复 DC → 删位图 → 删 DC → 销毁窗口”的顺序释放的，比依赖字段顺序更稳。

### 1.3 `create`

```rust
let hwnd = unsafe {
    CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        class, PCWSTR(title_w.as_ptr()), WS_POPUP,
        x, y, width as i32, height as i32,
        None, None, HINSTANCE::from(hinstance).into(), None,
    )
};
```

四个扩展样式各管一件事：

| 样式 | 作用 | 少了会怎样 |
|---|---|---|
| `WS_EX_LAYERED` | 逐像素透明 | 窗口是一块黑矩形 |
| `WS_EX_TOOLWINDOW` | 不出现在 Alt+Tab | 任务栏/切换器里多一个条目 |
| `WS_EX_TOPMOST` | 置顶 | 被别的窗口盖住 |
| `WS_EX_NOACTIVATE` | 点它不抢焦点 | 用户打字时点一下鱼，输入框失焦 |

`WS_EX_NOACTIVATE` 是后面**很多坑的来源**（菜单弹不出来、收不到键盘消息），
但它是正确的选择：桌宠不该抢焦点。

### 1.4 `draw`

```rust
pub fn draw(&mut self, width: u32, height: u32, bgra: &[u8]) -> Result<()> {
    if (w, h) != (self.dib.width, self.dib.height) {
        // 尺寸变了：换 DIB（不同动画裁切尺寸不同）
    }
    unsafe { copy_nonoverlapping(bgra.as_ptr(), self.dib.bits, n) };
    // 同步 alpha 给命中测试
    with_state(|s| {
        s.alpha.clear();
        s.alpha.extend(bgra.chunks_exact(4).map(|p| p[3]));
    });
    self.present()
}
```

两个细节：

1. **尺寸变化是正常路径**，不是异常：不同动画裁切尺寸不同，换段就要换 DIB；
2. **alpha 单独存一份**给 `WM_NCHITTEST` 用。`STATE` 是全局 `Mutex<Option<WndState>>`，
   因为 `WM_NCHITTEST` 可能在别的线程被系统调用，而 `draw` 在主线程。

### 1.5 `present`

```rust
let blend = BLENDFUNCTION {
    BlendOp: AC_SRC_OVER as u8,
    BlendFlags: 0,
    SourceConstantAlpha: 255,
    AlphaFormat: AC_SRC_ALPHA as u8,
};
UpdateLayeredWindow(hwnd, Some(screen_dc), None, Some(&size),
                    Some(self.mem_dc), Some(&src), COLORREF(0),
                    Some(&blend), ULW_ALPHA)
```

`AC_SRC_ALPHA` 要求像素是**预乘**的：颜色已经乘过 alpha。打包器在最后一步把 RGBA
转成预乘 BGRA，运行时就不用再算。

### 1.6 `wndproc`：只入队，不干活

```rust
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let (sx, sy) = client_point(lparam);
            let mut pt = POINT { x: sx, y: sy };
            let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
            let hit = hit_test(pt.x, pt.y);
            return LRESULT(if hit { HTCLIENT as isize } else { HTTRANSPARENT as isize });
        }
        WM_LBUTTONDOWN => {
            // 记录抓取偏移，push(MouseDown)
        }
        WM_MOUSEMOVE => { /* 拖拽用 SetWindowPos，否则 push(MouseMove) */ }
        WM_LBUTTONUP => push(Event::MouseUp { .. }),
        WM_RBUTTONUP => push(Event::RightClick { .. }),
        WM_TIMER => if wparam.0 == ANIM_TIMER { push(Event::Tick) },
        TRAY_MESSAGE => { /* 按 lparam 低字分派托盘左/右键 */ }
        WM_INPUT => crate::typing::handle_raw_input(lparam),
        WM_DESTROY => { push(Event::Quit); PostQuitMessage(0); }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
```

- `WM_NCHITTEST` 返回 `HTTRANSPARENT`，鼠标就能穿过透明像素点到桌面；
  只有 alpha ≥ 1 的像素才返回 `HTCLIENT`。
- `WM_INPUT` 必须调用 `DefWindowProc` 做清理，所以我们统一落到最后一行。
- 所有事件都进 `VecDeque`，业务逻辑在 `pump` 里跑，`wndproc` 里不画图。

### 1.7 `pump`：消息泵

```rust
pub fn pump(mut on_event: impl FnMut(Event)) -> Result<()> {
    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 == 0 { return Ok(()); }
        if ret.0 == -1 { return Err(...); }
        unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
        loop {
            let ev = with_state(|s| s.queue.pop_front()).flatten();
            match ev { Some(ev) => on_event(ev), None => break }
        }
    }
}
```

**为什么每取一条消息就把队列排空**：托盘回调是 shell 用**同步投递**（`SendMessage`）
发给窗口的。`wndproc` 压事件进队列时，`GetMessageW` 还在里面阻塞——同步投递会被它
内部处理掉，但**不会让等待返回**。队列要等下一条被取出的消息才会被处理。
平时动画定时器 8ms 一拍，所以看不出来；一旦隐藏（定时器停掉），托盘点击就永远躺在
队列里，用户看到的是“点了没反应”。这就是 `WAKE_TIMER` 存在的原因。

### 1.8 `Drop` 的释放顺序

```rust
SelectObject(self.mem_dc, self.old_bitmap);
DeleteObject(HGDIOBJ(self.dib.bitmap.0));
DeleteDC(self.mem_dc);
DestroyWindow(self.hwnd);
```

先恢复 DC 的选择状态，再删位图，最后删 DC/窗口。反过来会有“位图还被 DC 选中”
的未定义行为。

### 1.9 `find_window`：为什么不用 `FindWindowW`

实测对本进程这类 `WS_EX_NOACTIVATE` 的分层窗口，`FindWindowW` 返回 0，
而 `EnumWindows` 能按类名找到。它坏了的话，第二次启动 exe 会以为“没人在跑”
然后静默退出，用户看到的是“再点一次没反应”。

---

## 2. `menu.rs`：右键菜单

### 2.1 为什么用系统菜单

`TrackPopupMenu` 自带键盘导航、Esc 关闭、点别处自动消失、高 DPI 定位。
自绘这些要写几百行，而且永远不如系统那个顺手。

### 2.2 `WS_EX_NOACTIVATE` 的坑（本项目修过两次）

`TrackPopupMenu` 在窗口**不是前台**时会立即取消、返回 0；而 `WS_EX_NOACTIVATE` 的
窗口**永远成不了前台**。光调 `SetForegroundWindow` 会被系统“前台锁”默默拒掉。

正确做法（三件套）：

```rust
// 1. 临时摘掉 NOACTIVATE
let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE.0 as isize));
// 2. 把本线程输入队列挂到前台线程，绕过前台锁
let fg = GetForegroundWindow();
let fg_thread = GetWindowThreadProcessId(fg, None);
let me = GetCurrentThreadId();
let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
let _ = SetForegroundWindow(hwnd);
// 3. 弹菜单 + TPM_RETURNCMD 直接拿 id
let picked = TrackPopupMenuEx(self.handle, flags.0, x, y, hwnd, None);
// 4. 补一条 WM_NULL、拆 Attach、还原样式
```

判据：日志里 `菜单返回：Ok(...)（耗时 Nms）` 的 N 就是菜单活了多久。
≈0ms 就是没显示；几百毫秒才是真的停在屏幕上。

### 2.3 子菜单的所有权

```rust
pub fn submenu(&mut self, label: &str, build: impl FnOnce(&mut Menu)) -> Result<&mut Self> {
    let mut sub = Menu::new()?;
    build(&mut sub);
    AppendMenuW(self.handle, MF_POPUP, sub.handle.0 as usize, PCWSTR(wide.as_ptr()));
    std::mem::forget(sub);   // 句柄交给父菜单
    self.items += 1;
    Ok(self)
}
```

`AppendMenuW(MF_POPUP, 子菜单句柄, 标题)` 之后，子菜单归父菜单所有，父菜单
`DestroyMenu` 时会一并销毁。所以必须 `mem::forget` 掉 `sub` 的守卫，否则它析构时
先销毁一次，父菜单再销毁一次，同一句柄销毁两次是未定义行为。

### 2.4 自检钩子

`api_items` / `count_of` / `api_submenu` / `submenu_of` / `label_of` 直接问 Win32
“实际有几个项、第 N 项叫什么、它的子菜单有几项”。为什么不只数自己的 `items` 字段？
因为 `AppendMenuW` 的返回值被忽略，如果句柄用错，失败是静默的——只有问系统才能发现。

---

## 3. `tray.rs`：托盘图标

### 3.1 四个操作

`new`（`NIM_ADD`）、`set_tip`（`NIM_MODIFY` + `NIF_TIP`）、`set_icon`（`NIM_MODIFY`
+ `NIF_ICON`，**同时带 `NIF_MESSAGE`**）、`Drop`（`NIM_DELETE`）。

### 3.2 为什么 `Drop` 必须删

不删的话，进程退出后托盘上会留一个“幽灵图标”：点它没反应，直到鼠标划过通知区
才消失。用户会以为“程序还在跑”。

### 3.3 为什么换图标要带 `NIF_MESSAGE`

只传 `NIF_ICON` 时，某些 shell 版本会把它当成“这条记录只更新图标”，回调字段
按未提供处理 → 之后点击不再发消息。项目早期就踩过：`hide()` 里换完图标后，
点托盘图标再也收不到 `TrayLeftClick`。

### 3.4 为什么不用 v4

`NOTIFYICON_VERSION_4` 会把鼠标坐标搬进 `wparam`、改变 `lparam` 语义。本项目不需要
气球通知和自定义 id，保持 legacy 编码（`lparam` 低字就是鼠标消息）在 Win7~Win11
都支持，`wndproc` 的分派少一半分支。

---

## 4. `icon.rs`：从一帧像素画 HICON

两件事最容易错：

1. **预乘 vs 直通 alpha**：帧是预乘 BGRA，32bpp 图标要直通。降采样后必须
   `直通 = 预乘均值 × 255 ÷ alpha总和` 除回去，否则半透明边缘整圈发黑。
2. **先量 alpha 包围盒再缩放**：整帧直接缩会把大片透明留白算进去，角色只剩指甲盖大。

`Icon` 同时持有 `HICON`、彩色位图、AND 掩码，`Drop` 时三个都释放。
为什么 `CreateIconIndirect` 之后还留着两个位图？文档没说清它是否复制位图；
提前释放的代价是图标变黑块，保到 `Drop` 的代价只是几十字节。

托盘换图标的顺序也很关键：**先 `tray.set_icon(new.handle())`，再让旧 `Icon` 析构**。
反过来系统会短暂指着一个已经销毁的句柄，通知区闪空白。

---

## 5. `input.rs`：最后一次输入、光标、左键

- `last_input_ms()`：`GetLastInputInfo` 给“最后一次键鼠操作”的 tick，用
  `wrapping_sub` 算间隔（系统连开 49 天 tick 会回绕，普通减法会算出负数）。
- `cursor_pos()`：`GetCursorPos`，给“转向跟随”用。
- `left_button_down()`：`GetAsyncKeyState(VK_LBUTTON)`，给“点哪走哪”用。

为什么不直接收键盘/鼠标消息？因为窗口 `WS_EX_NOACTIVATE`，用户在别的窗口操作时
我们收不到。`GetLastInputInfo` 是会话级的，不需要焦点、不需要钩子。

---

## 6. `typing.rs`：键盘原始输入

需求：用户打字时，桌宠偶尔陪一段工作动画。

`WM_KEYDOWN` 只发给前台窗口，我们在后台收不到；`GetLastInputInfo` 又分不出键盘
还是鼠标。这里用 **Raw Input**：

```rust
let device = RAWINPUTDEVICE {
    usUsagePage: 0x01,   // Generic Desktop
    usUsage: 0x06,       // Keyboard
    dwFlags: RIDEV_INPUTSINK,   // 后台也收
    hwndTarget: hwnd,
};
RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32)?;
```

`wndproc` 收到 `WM_INPUT`：

```rust
GetRawInputData(HRAWINPUT(lparam.0 as *mut c_void), RID_INPUT, ...);
if raw.header.dwType != RIM_TYPEKEYBOARD.0 { return; }
let flags = raw.data.keyboard.Flags;
if u32::from(flags) & RI_KEY_BREAK == 0 { KEYS.fetch_add(1, Relaxed); }
```

- 一次按键会来两条（按下、抬起），只数按下，否则热度翻倍；
- **不记录键值**，只数次数；
- 窗口析构时 `RIDEV_REMOVE` 注销。

---

## 7. `shot.rs`：抓屏存 BMP

分层窗口的内容由 DWM 合成，`PrintWindow` 抓不到，必须走屏幕 `BitBlt`。
流程：`GetDC` → `CreateCompatibleDC` → `CreateCompatibleBitmap` → `BitBlt` →
`GdiFlush`（否则 `GetDIBits` 可能读到半成品）→ `GetDIBits` → 写 BMP。

用 BMP 而不是 PNG：零依赖，开发期再用 ffmpeg 转 PNG 看，不把图像库带进发行版。

---

## 8. `screen.rs` / `dpi.rs` / `instance.rs`

- `screen.rs`：`SystemParametersInfoW(SPI_GETWORKAREA)` 取主屏工作区（已排除任务栏）。
- `dpi.rs`：`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，
  否则 150% 缩放下窗口会被系统拉伸成糊的。
- `instance.rs`：命名互斥体。`Ok(None)` 表示已有实例在跑，调用方应安静退出
  （用户双击两次是常态，不是故障）。

---

## 9. `video.rs`：H.264 解码

`VideoReader::open(path, width, height, fps, frame_count)` 打开 `.fishvid`（内容 MP4）：

- 进程级 `MFStartup`，不反复初始化 COM；
- `IMFSourceReader` 请求 `MFVideoFormat_RGB32`；
- 从 native media type 读实际宽高和 `MF_MT_FRAME_RATE`；
- 视频可补偶数宽高，解码后裁到清单 `size`；
- 用 `MF_MT_DEFAULT_STRIDE` 判断 top-down/bottom-up；
- 顺序播放只解一帧；跳帧从目标前两秒的 keyframe 起解；
- 输出直通 BGRA、alpha=255；透明掩码由 app 层 `VideoSource` 套。

**时间基的坑**：ffmpeg 对 rawvideo 默认输出 25fps，而清单写 24fps。
`VideoReader` 不信清单，改用视频容器的 `MF_MT_FRAME_RATE` 换算 seek 时间；
打包器也显式 `-framerate 24 -r 24`，两边都钉住。

当前只实现 RGB32 输出；如果某个系统没有 RGB32 转换，会报清晰的解码错误，
NV12 手写转换留作后续兼容项。

---

## 10. 本章验收

- [ ] 能解释 `WS_EX_NOACTIVATE` 带来的三个坑（菜单、键盘、焦点）；
- [ ] 能说出 `TrackPopupMenu` 三件套各自解决什么；
- [ ] 能解释 `NIF_MESSAGE` 为什么必须和 `NIF_ICON` 一起传；
- [ ] 能说出预乘 alpha 与直通 alpha 的换算公式；
- [ ] 能解释 `WM_INPUT` 为什么要判断 `RI_KEY_BREAK`。
