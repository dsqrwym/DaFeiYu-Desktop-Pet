//! 从屏幕抓窗口矩形并存成 BMP。
//!
//! 用途只有一个：**让「真的画出来了」可验证**。分层窗口的内容由 DWM 合成，
//! `PrintWindow` 抓不到，必须走屏幕 `BitBlt`。用 BMP 是为了零依赖
//! （开发期再用 ffmpeg 转 PNG 看，不把图像库带进发行版）。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GdiFlush, GetDC,
    GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::{Error, Result};

/// 抓取窗口在屏幕上的实际像素，写入 32 位 BMP（自上而下）。返回 `(宽, 高)`。
pub fn capture_to_bmp(hwnd: HWND, path: impl AsRef<Path>) -> Result<(u32, u32)> {
    let mut rect = RECT::default();
    // SAFETY: 句柄有效，rect 为栈上接收结构。
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|e| Error::win32("GetWindowRect", e))?;
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;

    // SAFETY: 屏幕 DC 与内存 DC 成对创建/释放；位图最后统一删除。
    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let bmp = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
        let old = SelectObject(mem_dc, HGDIOBJ(bmp.0));

        let blit = BitBlt(
            mem_dc,
            0,
            0,
            width as i32,
            height as i32,
            Some(screen_dc),
            rect.left,
            rect.top,
            SRCCOPY,
        );
        // 先冲掉 GDI 批处理，否则 GetDIBits 可能读到半成品。
        let _ = GdiFlush();

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: Default::default(),
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            mem_dc,
            bmp,
            0,
            height,
            Some(pixels.as_mut_ptr() as *mut std::ffi::c_void),
            &mut info,
            DIB_RGB_COLORS,
        );

        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);

        blit.map_err(|e| Error::win32("BitBlt", e))?;
        if lines == 0 {
            return Err(Error::win32(
                "GetDIBits",
                windows::core::Error::from_win32(),
            ));
        }
        write_bmp(path.as_ref(), width, height, &pixels)?;
    }
    Ok((width, height))
}

/// BMP 头 + 像素。`pixels` 为自上而下的 BGRA。
///
/// 公开是为了让 `--dump` 能把**解出来的动画帧**也写成同一格式：
/// 判读素材内容（哪一帧出现了 Z、哪一段是静止）只能靠眼睛，
/// 而给眼睛看的东西必须能脱离程序独立打开。
pub fn write_bmp(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    let header_size = 54u32;
    let image_size = width * height * 4;
    let mut out = BufWriter::new(File::create(path)?);
    let mut head = [0u8; 54];
    head[0] = b'B';
    head[1] = b'M';
    head[2..6].copy_from_slice(&(header_size + image_size).to_le_bytes());
    head[10..14].copy_from_slice(&header_size.to_le_bytes());
    head[14..18].copy_from_slice(&40u32.to_le_bytes());
    head[18..22].copy_from_slice(&(width as i32).to_le_bytes());
    head[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes());
    head[26..28].copy_from_slice(&1u16.to_le_bytes());
    head[28..30].copy_from_slice(&32u16.to_le_bytes());
    head[34..38].copy_from_slice(&image_size.to_le_bytes());
    out.write_all(&head)?;
    out.write_all(pixels)?;
    out.flush()?;
    Ok(())
}
