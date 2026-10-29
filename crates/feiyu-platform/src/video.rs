//! H.264 颜色视频解码（Media Foundation）。
//!
//! 只负责“颜色帧”：透明掩码由 `feiyu_core::MaskSet` 在 app 层套用。
//! 输出统一为**直通 BGRA**（alpha=255），尺寸等于清单里的裁切尺寸；补边在
//! `pack-anim` 里做，解码端裁掉。
//!
//! Media Foundation 的 COM 生命周期按进程处理：第一次打开时 `MFStartup`，
//! 之后不再关闭。桌宠是单进程单 UI 线程，退出时由系统回收；这样避免多个
//! `VideoReader` 互相把对方的 MF 引用计数减掉。

use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFStartup, MFVideoFormat_RGB32, MFSTARTUP_FULL,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_VERSION,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Variant::VT_I8;

use crate::{Error, Result};

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
/// seek 时从目标前两秒的 keyframe 开始解，避免从文件头开始。
const SEEK_MARGIN: u32 = 48;

static MF_RUNTIME: OnceLock<std::result::Result<(), String>> = OnceLock::new();

fn ensure_mf() -> Result<()> {
    match MF_RUNTIME.get_or_init(|| {
        // MFStartup 在多数 Windows 上会自己处理 COM；显式 CoInitializeEx 是 MF 示例的常规做法。
        // 这里不配对 CoUninitialize/MFShutdown：运行时是进程级的，退出时由系统回收，
        // 避免多个 reader 的引用计数互相踩。
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| format!("MFStartup 失败：{e}"))
    }) {
        Ok(()) => Ok(()),
        Err(e) => Err(Error::Video(e.clone())),
    }
}

/// 一段 H.264 颜色视频的帧读取器。
pub struct VideoReader {
    reader: IMFSourceReader,
    /// 清单里的裁切尺寸（输出尺寸）。
    width: u16,
    height: u16,
    /// 视频实际尺寸（可能因补偶像素大一圈）。
    video_w: u32,
    video_h: u32,
    fps: f32,
    /// 视频容器自己的时间基；seek/时间戳换算必须用它。
    /// 它与 manifest 的播放 fps 可能不同（ffmpeg 默认 25fps 就是典型情况）。
    time_fps: f32,
    frame_count: u32,
    /// 下一帧顺序读取时应得到的帧号。
    next_idx: u32,
    /// 当前缓存的帧号。
    cached_idx: Option<u32>,
    /// 输出 BGRA（直通 alpha=255）。
    out: Vec<u8>,
    /// 解码缓冲的行方向。
    bottom_up: bool,
}

impl VideoReader {
    /// 打开一个 `.fishvid`（内容是 MP4/H.264）。
    ///
    /// `width × height` 是清单里的裁切尺寸；视频可以先补到偶数宽高，解码端裁掉。
    pub fn open(
        path: impl AsRef<Path>,
        width: u16,
        height: u16,
        fps: f32,
        frame_count: u32,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::Video("输出宽高不能为 0".into()));
        }
        if !(fps.is_finite() && fps > 0.0 && fps <= 240.0) {
            return Err(Error::Video(format!("fps 非法: {fps}")));
        }
        if frame_count == 0 {
            return Err(Error::Video("帧数不能为 0".into()));
        }
        ensure_mf()?;

        let wide: Vec<u16> = path
            .as_ref()
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut attrs: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attrs, 2) }
            .map_err(|e| Error::win32("MFCreateAttributes", e))?;
        let attrs = attrs.ok_or_else(|| Error::Video("MFCreateAttributes 返回空".into()))?;
        unsafe {
            attrs
                .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(|e| Error::win32("启用高级视频处理", e))?;
        }

        let reader = unsafe { MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs) }
            .map_err(|e| Error::win32("MFCreateSourceReaderFromURL", e))?;
        unsafe { reader.SetStreamSelection(STREAM, true) }
            .map_err(|e| Error::win32("SetStreamSelection", e))?;

        let native = unsafe { reader.GetNativeMediaType(STREAM, 0) }
            .map_err(|e| Error::win32("GetNativeMediaType", e))?;
        let frame_size = unsafe { native.GetUINT64(&MF_MT_FRAME_SIZE) }
            .map_err(|e| Error::win32("读视频尺寸", e))?;
        let video_w = (frame_size >> 32) as u32;
        let video_h = frame_size as u32;
        let rate = unsafe { native.GetUINT64(&MF_MT_FRAME_RATE) }.unwrap_or(0);
        let time_fps = if rate != 0 {
            let num = (rate >> 32) as f64;
            let den = (rate & 0xFFFF_FFFF) as f64;
            if num > 0.0 && den > 0.0 {
                (num / den) as f32
            } else {
                fps
            }
        } else {
            fps
        };
        if video_w < u32::from(width) || video_h < u32::from(height) {
            return Err(Error::Video(format!(
                "视频尺寸 {video_w}×{video_h} 小于清单 {}×{}",
                width, height
            )));
        }

        let media_type =
            unsafe { MFCreateMediaType() }.map_err(|e| Error::win32("MFCreateMediaType", e))?;
        unsafe {
            media_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| Error::win32("设置主类型", e))?;
            media_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| Error::win32("设置 RGB32", e))?;
            media_type
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(video_w) << 32) | u64::from(video_h),
                )
                .map_err(|e| Error::win32("设置帧尺寸", e))?;
            // 正 stride 请求 top-down；实际方向仍以 current media type 为准。
            media_type
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, video_w * 4)
                .map_err(|e| Error::win32("设置 stride", e))?;
        }
        unsafe { reader.SetCurrentMediaType(STREAM, None, &media_type) }
            .map_err(|e| Error::win32("设置 RGB32 输出", e))?;

        let current = unsafe { reader.GetCurrentMediaType(STREAM) }
            .map_err(|e| Error::win32("GetCurrentMediaType", e))?;
        let stride = unsafe { current.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|v| v as i32)
            .unwrap_or((video_w * 4) as i32);
        let bottom_up = stride < 0;

        Ok(Self {
            reader,
            width,
            height,
            video_w,
            video_h,
            fps,
            time_fps,
            frame_count,
            next_idx: 0,
            cached_idx: None,
            out: vec![0; width as usize * height as usize * 4],
            bottom_up,
        })
    }

    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    pub fn fps(&self) -> f32 {
        self.fps
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// 取第 `idx` 帧，返回直通 BGRA（alpha=255）。
    ///
    /// 顺序播放时只解一帧；跳帧时从目标前两秒的 keyframe 开始，解到目标时间。
    pub fn frame(&mut self, idx: u32) -> Result<&[u8]> {
        if idx >= self.frame_count {
            return Err(Error::Video(format!(
                "帧号 {idx} 越界（共 {} 帧）",
                self.frame_count
            )));
        }
        if self.cached_idx == Some(idx) {
            return Ok(&self.out);
        }
        if self.next_idx == idx {
            self.decode_next()?;
        } else {
            self.seek_to(idx)?;
        }
        Ok(&self.out)
    }

    fn decode_next(&mut self) -> Result<()> {
        self.decode_sample()?;
        self.cached_idx = Some(self.next_idx);
        self.next_idx = self.next_idx.saturating_add(1);
        Ok(())
    }

    fn seek_to(&mut self, idx: u32) -> Result<()> {
        let start = idx.saturating_sub(SEEK_MARGIN);
        let start_100ns = (f64::from(start) / f64::from(self.time_fps) * 10_000_000.0) as i64;
        self.set_position(start_100ns)?;
        self.next_idx = 0;
        self.cached_idx = None;

        for _ in 0..self.frame_count.saturating_add(64) {
            let ts = self.decode_sample()?;
            let Some(found) = self.idx_from_ts(ts) else {
                continue;
            };
            if found >= idx {
                // 时间戳有时会落在目标后一帧（编码器时基/取整）；seek 接受
                // “目标或之后最近的一帧”，顺序播放仍按帧号精确推进。
                self.cached_idx = Some(found);
                self.next_idx = found.saturating_add(1);
                return Ok(());
            }
        }
        Err(Error::Video(format!("seek 到第 {idx} 帧失败")))
    }

    fn set_position(&self, position_100ns: i64) -> Result<()> {
        let mut position = PROPVARIANT::default();
        unsafe {
            // PROPVARIANT 是 union + ManuallyDrop；逐字段赋值会触发析构警告，
            // 直接 write 一个初始化好的 PROPVARIANT_0_0。
            std::ptr::write(
                &mut position.Anonymous.Anonymous,
                ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_I8,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        hVal: position_100ns,
                    },
                }),
            );
        }
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &position) }
            .map_err(|e| Error::win32("SetCurrentPosition", e))
    }

    /// 解一帧填进 `self.out`，返回时间戳（100ns）。
    fn decode_sample(&mut self) -> Result<i64> {
        let mut actual = 0u32;
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample = None;
        unsafe {
            self.reader
                .ReadSample(
                    STREAM,
                    0,
                    Some(&mut actual),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| Error::win32("ReadSample", e))?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Err(Error::Video("视频提前结束".into()));
        }
        if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            // 输出类型由我们固定为 RGB32；真正变化时下一帧仍按 RGB32 解释。
            // 保持静默，避免日志被首帧刷屏。
        }
        let sample = sample.ok_or_else(|| Error::Video("ReadSample 返回空帧".into()))?;
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|e| Error::win32("ConvertToContiguousBuffer", e))?;
        let (ptr, len) = lock_buffer(&buffer)?;
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
        let copied = self.copy_frame(bytes);
        let unlocked =
            unsafe { buffer.Unlock() }.map_err(|e| Error::win32("IMFMediaBuffer::Unlock", e));
        copied?;
        unlocked?;
        Ok(ts)
    }

    fn copy_frame(&mut self, bytes: &[u8]) -> Result<()> {
        let stride = bytes.len() / self.video_h as usize;
        if stride < self.video_w as usize * 4 {
            return Err(Error::Video(format!(
                "解码缓冲 stride 太小：{stride} < {}",
                self.video_w as usize * 4
            )));
        }
        if bytes.len() < stride * self.video_h as usize {
            return Err(Error::Video("解码缓冲不足一整帧".into()));
        }
        let w = self.width as usize;
        let h = self.height as usize;
        for y in 0..h {
            let sy = if self.bottom_up {
                self.video_h as usize - 1 - y
            } else {
                y
            };
            let src = &bytes[sy * stride..];
            let dst = &mut self.out[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let s = x * 4;
                let d = x * 4;
                dst[d] = src[s];
                dst[d + 1] = src[s + 1];
                dst[d + 2] = src[s + 2];
                dst[d + 3] = 255;
            }
        }
        Ok(())
    }

    fn idx_from_ts(&self, ts: i64) -> Option<u32> {
        let f = ts as f64 / 10_000_000.0 * f64::from(self.time_fps);
        if f.is_finite() && f >= 0.0 && f <= f64::from(u32::MAX) {
            Some(f.round() as u32)
        } else {
            None
        }
    }
}

fn lock_buffer(buffer: &IMFMediaBuffer) -> Result<(*mut u8, u32)> {
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }
        .map_err(|e| Error::win32("IMFMediaBuffer::Lock", e))?;
    if ptr.is_null() {
        return Err(Error::Video("IMFMediaBuffer::Lock 返回空指针".into()));
    }
    Ok((ptr, len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny.fishvid")
    }

    #[test]
    fn decodes_frames_and_seeks_backwards() {
        let mut r = VideoReader::open(fixture(), 64, 64, 24.0, 24).expect("打开 tiny.fishvid");
        assert_eq!(r.frame_count(), 24);
        assert_eq!(r.size(), (64, 64));

        let f0 = r.frame(0).expect("第 0 帧").to_vec();
        assert_eq!(f0.len(), 64 * 64 * 4);
        assert!(f0.chunks_exact(4).all(|p| p[3] == 255), "alpha 必须为 255");

        // 上半红、下半蓝：验证 RGB32 的行方向没有倒。
        let top = &f0[(8 * 64 + 32) * 4..];
        let bottom = &f0[(56 * 64 + 32) * 4..];
        assert!(top[0] < 100 && top[2] > 150, "上半应为红，BGR={top:?}");
        assert!(
            bottom[0] > 150 && bottom[2] < 100,
            "下半应为蓝，BGR={bottom:?}"
        );

        let f1 = r.frame(1).expect("第 1 帧").to_vec();
        let f23 = r.frame(23).expect("第 23 帧").to_vec();
        assert_eq!(f1.len(), 64 * 64 * 4);
        assert_eq!(f23.len(), 64 * 64 * 4);

        // 倒退 seek 回 0，长度与 alpha 仍正确。
        let back = r.frame(0).expect("seek 回 0").to_vec();
        assert_eq!(back.len(), 64 * 64 * 4);
        assert!(back.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn rejects_out_of_range_frame() {
        let mut r = VideoReader::open(fixture(), 64, 64, 24.0, 24).expect("打开 tiny.fishvid");
        assert!(r.frame(24).is_err());
    }

    #[test]
    fn decodes_real_sample_when_present() {
        // 原型产物在 target/ 下，不入库；有就跑，没就跳过，保证干净 clone 也能过。
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/proto_h264/watermelon_color_crf20.mp4");
        if !path.exists() {
            return;
        }
        let mut r = VideoReader::open(&path, 201, 240, 24.0, 241).expect("打开真实样本");
        for idx in [0u32, 60, 120, 240] {
            let f = r.frame(idx).unwrap_or_else(|e| panic!("第 {idx} 帧：{e}"));
            assert_eq!(f.len(), 201 * 240 * 4, "第 {idx} 帧应裁到 201×240");
        }
    }
}
