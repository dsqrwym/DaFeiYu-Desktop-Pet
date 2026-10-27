//! 从桌宠自己的帧像素画一张托盘图标（`HICON`）。
//!
//! 为什么不塞 `.ico` 资源文件：那要 `.rc` + 资源编译器（或 `embed-resource`），
//! 给构建多加一条依赖链；而素材本来就在手里——取 idle 首帧降采样即可，
//! 托盘里的样子和桌面上的样子天然一致。
//!
//! 两个容易踩的点：
//!
//! * 帧是**预乘** BGRA（`UpdateLayeredWindow` 的要求），而 32bpp 图标位图要**直通** alpha。
//!   降采样之后必须把颜色除回去，否则半透明的边缘会整圈发黑。
//! * 整帧直接缩会把大片透明留白算进去，角色只剩指甲盖大。先量出 alpha 包围盒，
//!   按包围盒等比装箱，再居中摆——这也是所有「从大图生成小图标」的标准做法。

use std::ffi::c_void;

use windows::core::BOOL;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HGDIOBJ, RGBQUAD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
};

use crate::{Error, Result};

/// alpha 低于它算「空像素」。预乘后 8/255 已经几乎看不见。
const ALPHA_FLOOR: u8 = 8;

/// 系统度量读不到时的兜底图标尺寸。
const FALLBACK_SIZE: u32 = 16;

/// 一张可交给托盘（或窗口）使用的图标。
pub struct Icon {
    handle: HICON,
    /// 图标的彩色位图。`CreateIconIndirect` 之后**仍然保留**：
    /// 文档没说清它到底复不复制位图。提前释放的代价是图标变黑块，
    /// 保到 `Drop` 再删的代价只是两个几十字节的位图活得久一点——选后者。
    color: HBITMAP,
    /// 1bpp AND 掩码，同上。
    mask: HBITMAP,
}

impl Icon {
    /// 底层句柄，交给 `TrayIcon` 等需要 `HICON` 的地方。
    pub fn handle(&self) -> HICON {
        self.handle
    }

    /// 按系统小图标尺寸（`SM_CXSMICON`：100% 缩放是 16，150% 是 24）
    /// 从一帧预乘 BGRA 画图标。
    ///
    /// 尺寸跟着系统走而不是写死 32：写死的话系统会再缩一次，边缘糊成一团。
    pub fn from_bgra(bgra: &[u8], width: u32, height: u32) -> Result<Self> {
        // SAFETY: 只查一个系统度量，无指针参数。
        let metric = unsafe { GetSystemMetrics(SM_CXSMICON) };
        // 服务会话/无桌面时可能读到 0，退回 16 而不是崩。
        let size = if metric >= 8 {
            (metric as u32).min(64)
        } else {
            FALLBACK_SIZE
        };
        Self::from_bgra_sized(bgra, width, height, size)
    }

    /// 指定尺寸生成（自检与单测用）。
    pub fn from_bgra_sized(bgra: &[u8], width: u32, height: u32, size: u32) -> Result<Self> {
        Self::from_pixels(&icon_pixels(bgra, width, height, size), size)
    }

    fn from_pixels(px: &[u8], size: u32) -> Result<Self> {
        let size = size.max(1);
        debug_assert_eq!(px.len(), (size * size * 4) as usize, "图标字节数与尺寸不符");
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                // 负高度 = 自上而下，行序与我们的像素数组一致。
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default()],
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: info 已填满；bits 只是接收指针，不共享文件映射。
        let color =
            unsafe { CreateDIBSection(None, &mut info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| Error::win32("CreateDIBSection", e))?;
        // SAFETY: bits 指向 size×size×4 字节的 DIB 像素区。
        unsafe { std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len()) };

        // AND 掩码：1bpp，全 0 = 处处「不遮挡」。32bpp 图标的透明交给 alpha 通道，
        // 这张掩码只是给不认 alpha 的老 API 兜底。
        let stride = (size as usize).div_ceil(16) * 2;
        let zeros = vec![0u8; stride * size as usize];
        // SAFETY: zeros 在调用期间有效，长度覆盖 stride × height。
        let mask = unsafe {
            CreateBitmap(
                size as i32,
                size as i32,
                1,
                1,
                Some(zeros.as_ptr() as *const c_void),
            )
        };
        if mask.is_invalid() {
            // SAFETY: color 由本函数独占，失败路径上必须还回去。
            unsafe {
                let _ = DeleteObject(HGDIOBJ(color.0));
            }
            return Err(Error::win32(
                "CreateBitmap",
                windows::core::Error::from_win32(),
            ));
        }

        let params = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        // SAFETY: params 引用的两个位图都有效且尺寸匹配。
        match unsafe { CreateIconIndirect(&params) } {
            Ok(handle) => Ok(Self {
                handle,
                color,
                mask,
            }),
            Err(e) => {
                // SAFETY: 两个位图已不再被任何图标引用。
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(color.0));
                    let _ = DeleteObject(HGDIOBJ(mask.0));
                }
                Err(Error::win32("CreateIconIndirect", e))
            }
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: 三个句柄都由本结构独占，各释放一次。
        unsafe {
            let _ = DestroyIcon(self.handle);
            let _ = DeleteObject(HGDIOBJ(self.color.0));
            let _ = DeleteObject(HGDIOBJ(self.mask.0));
        }
    }
}

/// 把一帧预乘 BGRA 装进 `size × size` 的方框，输出**直通 alpha** 的 BGRA。
///
/// 这个函数完全不碰 Win32，单独抽出来是为了能直接单测：
/// 「图标像素对不对」不该靠盯着托盘区用肉眼看。
fn icon_pixels(bgra: &[u8], width: u32, height: u32, size: u32) -> Vec<u8> {
    let (w, h, s) = (width as usize, height as usize, size.max(1) as usize);
    let mut out = vec![0u8; s * s * 4];
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return out;
    }

    // 1. alpha 包围盒。整帧直接缩会把透明留白也算进去，角色会小得看不见。
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if bgra[(y * w + x) * 4 + 3] >= ALPHA_FLOOR {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return out; // 整帧全透明：空图标，但尺寸仍合法。
    }
    let (bw, bh) = (x1 - x0, y1 - y0);

    // 2. 等比装箱并居中。宽高比变了一点就整条鱼被压扁，所以只取一个缩放系数。
    let (dw, dh) = if bw * s / bh >= s {
        (s, (bh * s / bw).max(1).min(s))
    } else {
        ((bw * s / bh).max(1).min(s), s)
    };
    let ox = (s - dw) / 2;
    let oy = (s - dh) / 2;

    // 3. 盒式平均。在**预乘**空间里平均才是对的（这正是 alpha 合成的定义），
    //    算完再按 alpha 除回直通值。
    for dy in 0..dh {
        let sy0 = y0 + dy * bh / dh;
        let sy1 = (y0 + (dy + 1) * bh / dh).max(sy0 + 1).min(y1);
        for dx in 0..dw {
            let sx0 = x0 + dx * bw / dw;
            let sx1 = (x0 + (dx + 1) * bw / dw).max(sx0 + 1).min(x1);
            let n = (sx1 - sx0) * (sy1 - sy0);
            if n == 0 {
                continue;
            }
            let (mut b, mut g, mut r, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let o = (sy * w + sx) * 4;
                    b += bgra[o] as u32;
                    g += bgra[o + 1] as u32;
                    r += bgra[o + 2] as u32;
                    a += bgra[o + 3] as u32;
                }
            }
            let alpha = (a / n as u32) as u8;
            if alpha == 0 {
                continue;
            }
            // 直通颜色 = 预乘均值 ÷ (alpha/255) = 预乘总和 × 255 ÷ alpha 总和。
            let straight = |v: u32| ((v * 255 / a.max(1)).min(255)) as u8;
            let dst = ((oy + dy) * s + ox + dx) * 4;
            out[dst] = straight(b);
            out[dst + 1] = straight(g);
            out[dst + 2] = straight(r);
            out[dst + 3] = alpha;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张 `w×h` 的预乘 BGRA 帧。
    fn frame(w: u32, h: u32, px: impl Fn(usize, usize) -> (u8, u8, u8, u8)) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let (b, g, r, a) = px(x, y);
                let o = (y * w as usize + x) * 4;
                v[o] = b;
                v[o + 1] = g;
                v[o + 2] = r;
                v[o + 3] = a;
            }
        }
        v
    }

    #[test]
    fn transparent_frame_stays_empty() {
        let px = frame(8, 8, |_, _| (0, 0, 0, 0));
        let out = icon_pixels(&px, 8, 8, 16);
        assert_eq!(out.len(), 16 * 16 * 4);
        assert!(out.iter().all(|b| *b == 0), "全透明输入不该凭空长出像素");
    }

    #[test]
    fn opaque_square_keeps_its_colour() {
        // 中间 4×4 是不透明纯红，四周全透明。
        let px = frame(8, 8, |x, y| {
            if (2..6).contains(&x) && (2..6).contains(&y) {
                (0, 0, 255, 255)
            } else {
                (0, 0, 0, 0)
            }
        });
        let out = icon_pixels(&px, 8, 8, 16);
        let centre = (8 * 16 + 8) * 4;
        assert_eq!(out[centre + 2], 255, "红色通道应保持 255");
        assert_eq!(out[centre + 3], 255, "不透明区 alpha 应保持 255");
        // 正方形包围盒等比装箱后会铺满整张图标，四角也在包围盒内。
        assert_eq!(out[3], 255, "方框包围盒应铺满整张图标");
    }

    #[test]
    fn premultiplied_alpha_is_undone() {
        // 50% alpha 的红色，预乘后 RGB = 128。
        let px = frame(4, 4, |_, _| (0, 0, 128, 128));
        let out = icon_pixels(&px, 4, 4, 8);
        let centre = (4 * 8 + 4) * 4;
        assert!(
            (out[centre + 2] as i32 - 255).abs() <= 2,
            "除回直通 alpha 后红应≈255，实际 {}",
            out[centre + 2]
        );
        assert!((out[centre + 3] as i32 - 128).abs() <= 2);
    }

    #[test]
    fn wide_frame_keeps_aspect_ratio() {
        // 宽 16 高 4 的不透明条，装进 16×16 后高度≈4，上下留透明边。
        let px = frame(16, 8, |_, y| {
            if (2..6).contains(&y) {
                (255, 255, 255, 255)
            } else {
                (0, 0, 0, 0)
            }
        });
        let out = icon_pixels(&px, 16, 8, 16);
        let row_empty = |ry: usize| (0..16).all(|x| out[(ry * 16 + x) * 4 + 3] == 0);
        assert!(row_empty(0), "上边应留透明");
        assert!(row_empty(15), "下边应留透明");
        assert!(!row_empty(8), "中间应有内容");
    }

    #[test]
    fn tiny_or_mismatched_input_does_not_panic() {
        assert_eq!(icon_pixels(&[], 0, 0, 16).len(), 16 * 16 * 4);
        assert_eq!(icon_pixels(&[0u8; 16], 8, 8, 4).len(), 4 * 4 * 4);
    }
}
