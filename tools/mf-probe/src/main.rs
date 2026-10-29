//! H.264 + Media Foundation 原型探针（只用于验证，不随应用发布）。
//!
//! 输入是 ffmpeg 生成的「左半颜色、右半 alpha」H.264，以及 ffmpeg 导出的原始
//! RGBA 帧序列。它做四件事：
//!   1. 用 IMFSourceReader 解码成 RGB32；
//!   2. 把右半的灰度当 alpha，左半的颜色按预乘/直通还原成 BGRA；
//!   3. 和原始 RGBA 逐帧比较，重点看 alpha 边缘；
//!   4. 写几张深色/浅色/棋盘背景的 BMP，供人眼判断彩边。
//!
//! 这不是生产解码器：没有缓存、没有异步、没有错误恢复。

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFSourceReader, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFShutdown, MFStartup, MFVideoFormat_RGB32,
    MFSTARTUP_FULL, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Variant::VT_I8;

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

fn main() -> Result<()> {
    let args = Args::parse()?;
    if let Some(export) = &args.export {
        let raw = args.raw_out.as_ref().context("--export 需要 --raw-out")?;
        export_fishanim(
            export,
            raw,
            args.mask_out.as_deref(),
            args.enc_out.as_deref(),
            args.dilate,
        )?;
        return Ok(());
    }
    fs::create_dir_all(&args.out).with_context(|| format!("建不了 {}", args.out.display()))?;

    let mf = Mf::start()?;
    let mut reader = Reader::open(&args.video, args.video_w, args.color_h)?;
    let masks = args.mask.as_ref().map(|p| MaskSet::load(p)).transpose()?;
    let mut alpha_reader = if masks.is_some() {
        None
    } else {
        args.alpha_video
            .as_ref()
            .map(|p| Reader::open(p, args.video_w, args.color_h))
            .transpose()?
    };

    let mut ref_file = File::open(&args.reference)
        .with_context(|| format!("读不了参考帧 {}", args.reference.display()))?;
    let ref_frame_bytes = args.color_w as usize * args.color_h as usize * 4;

    let first = Instant::now();
    let decoded = reader.next_frame()?;
    let Some(mut decoded) = decoded else {
        bail!("第一帧解码失败");
    };
    let mut alpha_decoded = match alpha_reader.as_mut() {
        Some(r) => Some(r.next_frame()?.context("alpha 流第一帧解码失败")?),
        None => None,
    };
    let first_ms = first.elapsed().as_secs_f64() * 1000.0;

    // RGB32 的行方向由解码器决定；用第 0 帧和参考帧比一下，自动选正/倒。
    let flip = if let Some(masks) = &masks {
        detect_flip_mask(&decoded, masks, &mut ref_file, ref_frame_bytes, &args)?
    } else {
        match &alpha_decoded {
            Some(alpha) => detect_flip_two(&decoded, alpha, &mut ref_file, ref_frame_bytes, &args)?,
            None => detect_flip(&decoded, &mut ref_file, ref_frame_bytes, &args)?,
        }
    };
    eprintln!(
        "行方向：{}",
        if flip {
            "bottom-up（翻转）"
        } else {
            "top-down"
        }
    );

    let mut stats = Stats {
        first_ms,
        mp4_bytes: fs::metadata(&args.video).map(|m| m.len()).unwrap_or(0),
        frames: args.frames,
        ..Default::default()
    };

    for idx in 0..args.frames {
        let started = Instant::now();
        if idx > 0 {
            let Some(next) = reader.next_frame()? else {
                bail!("第 {idx} 帧提前结束（期望 {} 帧）", args.frames);
            };
            decoded = next;
            if let Some(r) = alpha_reader.as_mut() {
                alpha_decoded = Some(r.next_frame()?.context("alpha 流提前结束")?);
            }
        }
        let bgra = if let Some(masks) = &masks {
            if args.feather {
                apply_mask_soft(
                    &decoded,
                    masks.frame(idx),
                    masks,
                    args.color_w,
                    args.color_h,
                    args.video_w,
                    flip,
                )
            } else {
                apply_mask(
                    &decoded,
                    masks.frame(idx),
                    masks,
                    args.color_w,
                    args.color_h,
                    args.video_w,
                    flip,
                )
            }
        } else {
            match &alpha_decoded {
                Some(alpha) => merge_two(
                    &decoded,
                    alpha,
                    args.color_w,
                    args.color_h,
                    args.video_w,
                    flip,
                ),
                None => to_bgra_premul(&decoded, args.color_w, args.color_h, flip),
            }
        };
        stats.decode_ms += started.elapsed().as_secs_f64() * 1000.0;

        let reference = read_ref(&mut ref_file, idx, ref_frame_bytes)?;
        if args.feather {
            compare_feather(&bgra, &reference, &mut stats, args.color_w, args.color_h)?;
        } else {
            compare(&bgra, &reference, &mut stats, args.color_w, args.color_h)?;
        }

        if idx == 0 || idx == 60 || idx == 120 || idx == 180 || idx == 240 {
            write_samples(
                &bgra,
                &reference,
                idx,
                args.color_w,
                args.color_h,
                &args.out,
            )?;
        }
    }
    ensure!(stats.frames > 0, "没有帧");
    stats.decode_avg_ms = stats.decode_ms / stats.frames as f64;

    // 随机 seek：用新的 reader，避免上一轮 EOF 状态影响。
    let mut seek_reader = Reader::open(&args.video, args.video_w, args.color_h)?;
    let mut seek_alpha = args
        .alpha_video
        .as_ref()
        .map(|p| Reader::open(p, args.video_w, args.color_h))
        .transpose()?;
    for target in [50u32, 110, 170, 230] {
        let started = Instant::now();
        let ts = seek_reader.seek_to_frame(target, args.fps)?;
        if let Some(r) = seek_alpha.as_mut() {
            let _ = r.seek_to_frame(target, args.fps)?;
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        stats.seek_ms.push((target, ts, ms));
    }

    stats.print(&args);
    drop(seek_alpha);
    drop(seek_reader);
    drop(alpha_reader);
    drop(reader);
    drop(ref_file);
    drop(mf);
    Ok(())
}

struct Mf {
    com: bool,
}

impl Mf {
    fn start() -> Result<Self> {
        // MFStartup 内部会初始化 COM；显式 CoInitializeEx 是 Media Foundation 示例的常规做法。
        // 如果当前线程已经是 MTA，CoInitializeEx 返回 RPC_E_CHANGED_MODE；这时不能配对
        // 调 CoUninitialize，否则会把别人的 COM 引用计数减掉。
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.context("MFStartup")?;
        Ok(Self { com })
    }
}

impl Drop for Mf {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

struct Reader {
    reader: IMFSourceReader,
    width: u32,
    height: u32,
}

impl Reader {
    fn open(path: &Path, width: u32, height: u32) -> Result<Self> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut attrs: Option<IMFAttributes> = None;
        unsafe { MFCreateAttributes(&mut attrs, 2) }.context("MFCreateAttributes")?;
        let attrs = attrs.context("MFCreateAttributes 返回空")?;
        unsafe {
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        }

        let reader = unsafe { MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs) }
            .with_context(|| format!("打开 {}", path.display()))?;
        unsafe { reader.SetStreamSelection(STREAM, true) }?;

        let media_type = unsafe { MFCreateMediaType() }?;
        unsafe {
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        }
        unsafe { reader.SetCurrentMediaType(STREAM, None, &media_type) }
            .context("设置 RGB32 输出失败")?;

        Ok(Self {
            reader,
            width,
            height,
        })
    }

    fn next_frame(&mut self) -> Result<Option<Frame>> {
        let mut actual = 0u32;
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample = None;
        unsafe {
            self.reader.ReadSample(
                STREAM,
                0,
                Some(&mut actual),
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )?
        };
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(None);
        }
        if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            eprintln!("注意：解码中途媒体类型变了");
        }
        let sample = sample.context("ReadSample 返回空帧且不是 EOS")?;
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let (ptr, len) = lock(&buffer)?;
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
        unsafe { buffer.Unlock()? };

        let stride = (len as usize / self.height as usize).max(self.width as usize * 4);
        ensure!(
            bytes.len() >= stride * self.height as usize,
            "解码缓冲不够一整帧：{} < {}",
            bytes.len(),
            stride * self.height as usize
        );
        Ok(Some(Frame {
            bytes,
            stride,
            timestamp: ts,
        }))
    }

    fn seek_to_frame(&mut self, frame: u32, fps: f32) -> Result<i64> {
        let target_s = frame as f64 / fps as f64;
        let target_100ns = (target_s * 10_000_000.0) as i64;

        let mut position = PROPVARIANT::default();
        unsafe {
            // PROPVARIANT 是“union + ManuallyDrop”的 C 布局；逐字段赋值会触发
            // DerefMut 的析构警告，所以直接 write 一个初始化好的 PROPVARIANT_0_0。
            std::ptr::write(
                &mut position.Anonymous.Anonymous,
                ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_I8,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { hVal: target_100ns },
                }),
            );
        }
        unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &position) }
            .context("SetCurrentPosition")?;

        // 从 keyframe 解到目标时间；返回实际拿到的帧时间戳。
        for _ in 0..240 {
            let Some(frame) = self.next_frame()? else {
                bail!("seek 到 {frame} 后提前 EOF");
            };
            if frame.timestamp + 1 >= target_100ns {
                return Ok(frame.timestamp);
            }
        }
        bail!("seek 到第 {frame} 帧失败（解码 240 帧仍未到目标时间）")
    }
}

struct Frame {
    bytes: Vec<u8>,
    stride: usize,
    timestamp: i64,
}

fn lock(buffer: &IMFMediaBuffer) -> Result<(*mut u8, u32)> {
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }?;
    ensure!(!ptr.is_null(), "IMFMediaBuffer::Lock 返回空指针");
    Ok((ptr, len))
}

/// RGB32（BGRA）→ 预乘 BGRA。右半是 alpha；左半是颜色（已经是预乘值）。
fn to_bgra_premul(frame: &Frame, color_w: u32, color_h: u32, flip: bool) -> Vec<u8> {
    let w = color_w as usize;
    let h = color_h as usize;
    let stride = frame.stride.max(w * 2 * 4);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if flip { h - 1 - y } else { y };
        let row = &frame.bytes[sy * stride..];
        for x in 0..w {
            let o = x * 4;
            let alpha = row[(w + x) * 4]; // 右半灰度；B/G/R 相等
            let d = (y * w + x) * 4;
            if alpha == 0 {
                out[d..d + 4].fill(0);
            } else {
                out[d] = row[o];
                out[d + 1] = row[o + 1];
                out[d + 2] = row[o + 2];
                out[d + 3] = alpha;
            }
        }
    }
    out
}

fn detect_flip(
    frame: &Frame,
    ref_file: &mut File,
    ref_frame_bytes: usize,
    args: &Args,
) -> Result<bool> {
    let reference = read_ref(ref_file, 0, ref_frame_bytes)?;
    let normal = to_bgra_premul(frame, args.color_w, args.color_h, false);
    let flipped = to_bgra_premul(frame, args.color_w, args.color_h, true);
    Ok(visible_error(&flipped, &reference) < visible_error(&normal, &reference))
}

fn detect_flip_two(
    color: &Frame,
    alpha: &Frame,
    ref_file: &mut File,
    ref_frame_bytes: usize,
    args: &Args,
) -> Result<bool> {
    let reference = read_ref(ref_file, 0, ref_frame_bytes)?;
    let normal = merge_two(
        color,
        alpha,
        args.color_w,
        args.color_h,
        args.video_w,
        false,
    );
    let flipped = merge_two(color, alpha, args.color_w, args.color_h, args.video_w, true);
    Ok(visible_error(&flipped, &reference) < visible_error(&normal, &reference))
}

/// 颜色/alpha 两路解码后合并。颜色流可以是 202 宽（补了一列），只取前 201 列。
fn merge_two(
    color: &Frame,
    alpha: &Frame,
    color_w: u32,
    color_h: u32,
    video_w: u32,
    flip: bool,
) -> Vec<u8> {
    let w = color_w as usize;
    let h = color_h as usize;
    let vw = video_w as usize;
    let cstride = color.stride.max(vw * 4);
    let astride = alpha.stride.max(vw * 4);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if flip { h - 1 - y } else { y };
        let crow = &color.bytes[sy * cstride..];
        let arow = &alpha.bytes[sy * astride..];
        for x in 0..w {
            let o = x * 4;
            let a = arow[o]; // 灰度 alpha 流，B/G/R 相等
            let d = (y * w + x) * 4;
            if a == 0 {
                out[d..d + 4].fill(0);
            } else {
                out[d] = crow[o];
                out[d + 1] = crow[o + 1];
                out[d + 2] = crow[o + 2];
                out[d + 3] = a;
            }
        }
    }
    out
}

fn detect_flip_mask(
    color: &Frame,
    masks: &MaskSet,
    ref_file: &mut File,
    ref_frame_bytes: usize,
    args: &Args,
) -> Result<bool> {
    let reference = read_ref(ref_file, 0, ref_frame_bytes)?;
    let normal = apply_mask(
        color,
        masks.frame(0),
        masks,
        args.color_w,
        args.color_h,
        args.video_w,
        false,
    );
    let flipped = apply_mask(
        color,
        masks.frame(0),
        masks,
        args.color_w,
        args.color_h,
        args.video_w,
        true,
    );
    Ok(visible_error(&flipped, &reference) < visible_error(&normal, &reference))
}

struct MaskSet {
    data: Vec<u8>,
    w: usize,
    h: usize,
    frames: u32,
    mask_len: usize,
}

impl MaskSet {
    fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("读不了 {}", path.display()))?;
        ensure!(
            bytes.len() > 16 && &bytes[..8] == b"FISHMSK1",
            "mask 文件头不对"
        );
        let w = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let h = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
        let frames = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        ensure!(w > 0 && h > 0 && frames > 0, "mask 头尺寸非法");
        let mask_len = (w * h).div_ceil(8);
        let expected = frames as usize * mask_len;
        let data = zstd::bulk::decompress(&bytes[16..], expected).context("解压 mask 失败")?;
        ensure!(data.len() == expected, "mask 解压长度不对");
        Ok(Self {
            data,
            w,
            h,
            frames,
            mask_len,
        })
    }

    fn frame(&self, idx: u32) -> &[u8] {
        debug_assert!(idx < self.frames);
        let at = idx as usize * self.mask_len;
        &self.data[at..at + self.mask_len]
    }
}

fn apply_mask(
    color: &Frame,
    mask: &[u8],
    masks: &MaskSet,
    color_w: u32,
    color_h: u32,
    video_w: u32,
    flip: bool,
) -> Vec<u8> {
    let w = color_w as usize;
    let h = color_h as usize;
    let vw = video_w as usize;
    let stride = color.stride.max(vw * 4);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if flip { h - 1 - y } else { y };
        let row = &color.bytes[sy * stride..];
        for x in 0..w {
            let bit = if x < masks.w && y < masks.h {
                let i = y * masks.w + x;
                mask[i / 8] & (1 << (i % 8)) != 0
            } else {
                false
            };
            if !bit {
                continue;
            }
            let o = x * 4;
            let d = (y * w + x) * 4;
            out[d] = row[o];
            out[d + 1] = row[o + 1];
            out[d + 2] = row[o + 2];
            out[d + 3] = 255;
        }
    }
    out
}

fn visible_error(bgra: &[u8], reference: &[u8]) -> f64 {
    let (mut sum, mut n) = (0u64, 0u64);
    for (a, b) in bgra.chunks_exact(4).zip(reference.chunks_exact(4)) {
        if b[3] == 0 {
            continue;
        }
        for c in 0..3 {
            sum += u64::from(a[c].abs_diff(b[c]));
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        sum as f64 / n as f64
    }
}

fn read_ref(file: &mut File, idx: u32, frame_bytes: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(idx as u64 * frame_bytes as u64))?;
    let mut out = vec![0u8; frame_bytes];
    file.read_exact(&mut out)
        .with_context(|| format!("参考帧第 {idx} 帧不完整"))?;
    Ok(out)
}

fn apply_mask_soft(
    color: &Frame,
    mask: &[u8],
    masks: &MaskSet,
    color_w: u32,
    color_h: u32,
    video_w: u32,
    flip: bool,
) -> Vec<u8> {
    let w = color_w as usize;
    let h = color_h as usize;
    let stride = color.stride.max(video_w as usize * 4);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if flip { h - 1 - y } else { y };
        let row = &color.bytes[sy * stride..];
        for x in 0..w {
            let a = soft_alpha(mask, masks, x, y);
            if a == 0 {
                continue;
            }
            let o = x * 4;
            let d = (y * w + x) * 4;
            out[d] = (u32::from(row[o]) * u32::from(a) / 255) as u8;
            out[d + 1] = (u32::from(row[o + 1]) * u32::from(a) / 255) as u8;
            out[d + 2] = (u32::from(row[o + 2]) * u32::from(a) / 255) as u8;
            out[d + 3] = a;
        }
    }
    out
}

/// 1px 羽化：3x3 盒式平均二值掩码。包工具和运行时用同一个公式，
/// 所以不需要额外存 8-bit alpha。
fn soft_alpha(mask: &[u8], masks: &MaskSet, x: usize, y: usize) -> u8 {
    let mut sum = 0u32;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= masks.w as i32 || ny >= masks.h as i32 {
                continue;
            }
            let i = ny as usize * masks.w + nx as usize;
            if mask[i / 8] & (1 << (i % 8)) != 0 {
                sum += 255;
            }
        }
    }
    (sum / 9) as u8
}

fn compare_feather(bgra: &[u8], reference: &[u8], stats: &mut Stats, w: u32, h: u32) -> Result<()> {
    ensure!(bgra.len() == reference.len(), "帧尺寸不一致（{w}x{h}）");
    let w = w as usize;
    let h = h as usize;
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let a = &bgra[o..o + 4];
            let b = &reference[o..o + 4];
            let ref_alpha = b[3];
            let dec_alpha = a[3];
            stats.alpha_sum += u64::from(dec_alpha.abs_diff(ref_alpha));
            stats.alpha_n += 1;
            stats.alpha_max = stats.alpha_max.max(dec_alpha.abs_diff(ref_alpha));
            if ref_alpha == 0 || dec_alpha == 0 {
                continue;
            }

            // 解码颜色的 alpha 是羽化后的，先除回直通色，再和参考色比；
            // 这样测的是“颜色是否还在”，不会把羽化本身算成颜色错误。
            let c = [
                (u32::from(a[0]) * 255 / u32::from(dec_alpha)).min(255) as u8,
                (u32::from(a[1]) * 255 / u32::from(dec_alpha)).min(255) as u8,
                (u32::from(a[2]) * 255 / u32::from(dec_alpha)).min(255) as u8,
            ];
            for i in 0..3 {
                let d = c[i].abs_diff(b[i]);
                stats.all_hist[d as usize] += 1;
                stats.all_max = stats.all_max.max(d);
            }
            let mut edge = false;
            'edge: for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        edge = true;
                        break 'edge;
                    }
                    if reference[(ny as usize * w + nx as usize) * 4 + 3] == 0 {
                        edge = true;
                        break 'edge;
                    }
                }
            }
            if edge {
                for i in 0..3 {
                    let d = c[i].abs_diff(b[i]);
                    stats.edge_hist[d as usize] += 1;
                    stats.edge_max = stats.edge_max.max(d);
                }
            }
        }
    }
    Ok(())
}

fn compare(bgra: &[u8], reference: &[u8], stats: &mut Stats, w: u32, h: u32) -> Result<()> {
    ensure!(
        bgra.len() == reference.len(),
        "帧尺寸不一致：{} vs {}（{w}x{h}）",
        bgra.len(),
        reference.len()
    );
    let w = w as usize;
    let h = h as usize;
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let a = &bgra[o..o + 4];
            let b = &reference[o..o + 4];
            let ref_alpha = b[3];
            let dec_alpha = a[3];

            stats.alpha_sum += u64::from(dec_alpha.abs_diff(ref_alpha));
            stats.alpha_n += 1;
            stats.alpha_max = stats.alpha_max.max(dec_alpha.abs_diff(ref_alpha));
            if ref_alpha == 0 && dec_alpha > 8 {
                stats.alpha_fp += 1;
            }
            if ref_alpha == 255 && dec_alpha < 247 {
                stats.alpha_fn += 1;
            }

            if ref_alpha == 0 {
                continue;
            }
            for c in 0..3 {
                let d = a[c].abs_diff(b[c]);
                stats.all_hist[d as usize] += 1;
                stats.all_max = stats.all_max.max(d);
            }

            // 参考 alpha 是硬边（0/255）；“边缘带”定义为可见像素半径 2 内
            // 有透明像素。H.264 的色度抽样/有损 alpha 主要在这条带上露馅。
            let mut edge = false;
            'edge: for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        edge = true;
                        break 'edge;
                    }
                    if reference[(ny as usize * w + nx as usize) * 4 + 3] == 0 {
                        edge = true;
                        break 'edge;
                    }
                }
            }
            if edge {
                for c in 0..3 {
                    let d = a[c].abs_diff(b[c]);
                    stats.edge_hist[d as usize] += 1;
                    stats.edge_max = stats.edge_max.max(d);
                }
            }
        }
    }
    Ok(())
}

fn write_samples(
    rgba: &[u8],
    reference: &[u8],
    idx: u32,
    w: u32,
    h: u32,
    out: &Path,
) -> Result<()> {
    let backgrounds: [(&str, [u8; 3]); 3] = [
        ("dark", [32, 32, 32]),
        ("light", [240, 240, 240]),
        ("checker", [0, 0, 0]), // 棋盘单独算
    ];
    for (name, bg) in backgrounds {
        let mut dec = vec![0u8; (w * h * 4) as usize];
        let mut refc = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let color = if name == "checker" {
                    if ((x / 8) + (y / 8)) % 2 == 0 {
                        [80, 80, 80]
                    } else {
                        [200, 200, 200]
                    }
                } else {
                    bg
                };
                let o = (y * w as usize + x) * 4;
                composite(&rgba[o..o + 4], &color, &mut dec[o..o + 4]);
                composite(&reference[o..o + 4], &color, &mut refc[o..o + 4]);
            }
        }
        write_bmp(&out.join(format!("dec_{name}_{idx:03}.bmp")), w, h, &dec)?;
        write_bmp(&out.join(format!("ref_{name}_{idx:03}.bmp")), w, h, &refc)?;
    }
    Ok(())
}

fn composite(bgra: &[u8], bg: &[u8; 3], out: &mut [u8]) {
    // 预乘 BGRA：颜色已经是 color*alpha，合成时只加背景乘 (1-alpha)。
    let inv = 255 - u32::from(bgra[3]);
    out[0] = (u32::from(bgra[0]) + u32::from(bg[0]) * inv / 255).min(255) as u8;
    out[1] = (u32::from(bgra[1]) + u32::from(bg[1]) * inv / 255).min(255) as u8;
    out[2] = (u32::from(bgra[2]) + u32::from(bg[2]) * inv / 255).min(255) as u8;
    out[3] = 255;
}

fn write_bmp(path: &Path, width: u32, height: u32, bgra: &[u8]) -> Result<()> {
    let header_size = 54u32;
    let image_size = width * height * 4;
    let mut f = BufWriter::new(File::create(path)?);
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
    f.write_all(&head)?;
    f.write_all(bgra)?;
    f.flush()?;
    Ok(())
}

struct Stats {
    frames: u32,
    first_ms: f64,
    decode_ms: f64,
    decode_avg_ms: f64,
    mp4_bytes: u64,
    all_hist: Vec<u64>,
    all_max: u8,
    edge_hist: Vec<u64>,
    edge_max: u8,
    alpha_sum: u64,
    alpha_n: u64,
    alpha_max: u8,
    alpha_fp: u64,
    alpha_fn: u64,
    seek_ms: Vec<(u32, i64, f64)>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            frames: 0,
            first_ms: 0.0,
            decode_ms: 0.0,
            decode_avg_ms: 0.0,
            mp4_bytes: 0,
            all_hist: vec![0; 256],
            all_max: 0,
            edge_hist: vec![0; 256],
            edge_max: 0,
            alpha_sum: 0,
            alpha_n: 0,
            alpha_max: 0,
            alpha_fp: 0,
            alpha_fn: 0,
            seek_ms: Vec::new(),
        }
    }
}

impl Stats {
    fn print(&self, args: &Args) {
        let (all_mean, all_p99) = summary(&self.all_hist);
        let (edge_mean, edge_p99) = summary(&self.edge_hist);
        let alpha_mean = if self.alpha_n == 0 {
            0.0
        } else {
            self.alpha_sum as f64 / self.alpha_n as f64
        };
        let pct = |n: u64| {
            if self.alpha_n == 0 {
                0.0
            } else {
                n as f64 * 100.0 / self.alpha_n as f64
            }
        };
        println!("clip={}", args.label);
        println!("frames={} mp4_bytes={}", self.frames, self.mp4_bytes);
        println!(
            "first_ms={:.2} decode_avg_ms={:.3} decode_total_ms={:.1}",
            self.first_ms, self.decode_avg_ms, self.decode_ms
        );
        println!(
            "all_mean={:.3} all_p99={} all_max={}",
            all_mean, all_p99, self.all_max
        );
        println!(
            "edge_mean={:.3} edge_p99={} edge_max={}",
            edge_mean, edge_p99, self.edge_max
        );
        println!(
            "alpha_mean={:.3} alpha_max={} alpha_fp={:.3}% alpha_fn={:.3}%",
            alpha_mean,
            self.alpha_max,
            pct(self.alpha_fp),
            pct(self.alpha_fn)
        );
        for (frame, ts, ms) in &self.seek_ms {
            println!("seek frame={frame} ts={ts} ms={ms:.2}");
        }
    }
}

fn summary(hist: &[u64]) -> (f64, usize) {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return (0.0, 0);
    }
    let mean = hist
        .iter()
        .enumerate()
        .map(|(v, &c)| v as u64 * c)
        .sum::<u64>() as f64
        / total as f64;
    let mut acc = 0u64;
    for (value, &count) in hist.iter().enumerate() {
        acc += count;
        if acc * 100 >= total * 99 {
            return (mean, value);
        }
    }
    (mean, hist.len() - 1)
}

struct Args {
    video: PathBuf,
    alpha_video: Option<PathBuf>,
    mask: Option<PathBuf>,
    reference: PathBuf,
    out: PathBuf,
    frames: u32,
    fps: f32,
    color_w: u32,
    color_h: u32,
    video_w: u32,
    feather: bool,
    label: String,
    export: Option<PathBuf>,
    raw_out: Option<PathBuf>,
    mask_out: Option<PathBuf>,
    enc_out: Option<PathBuf>,
    dilate: u32,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut out = Self {
            video: PathBuf::new(),
            alpha_video: None,
            mask: None,
            reference: PathBuf::new(),
            out: PathBuf::from("target/proto_h264/decoded"),
            frames: 241,
            fps: 24.0,
            color_w: 544,
            color_h: 306,
            video_w: 0,
            feather: false,
            label: "clip".to_owned(),
            export: None,
            raw_out: None,
            mask_out: None,
            enc_out: None,
            dilate: 0,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            let value = || anyhow::anyhow!("{arg} 缺少值");
            match arg.as_str() {
                "--video" => out.video = PathBuf::from(it.next().ok_or_else(value)?),
                "--ref" => out.reference = PathBuf::from(it.next().ok_or_else(value)?),
                "--out" => out.out = PathBuf::from(it.next().ok_or_else(value)?),
                "--frames" => out.frames = it.next().ok_or_else(value)?.parse()?,
                "--fps" => out.fps = it.next().ok_or_else(value)?.parse()?,
                "--width" => out.color_w = it.next().ok_or_else(value)?.parse()?,
                "--height" => out.color_h = it.next().ok_or_else(value)?.parse()?,
                "--label" => out.label = it.next().ok_or_else(value)?,
                "--alpha-video" => {
                    out.alpha_video = Some(PathBuf::from(it.next().ok_or_else(value)?))
                }
                "--mask" => out.mask = Some(PathBuf::from(it.next().ok_or_else(value)?)),
                "--video-width" => out.video_w = it.next().ok_or_else(value)?.parse()?,
                "--feather" => out.feather = true,
                "--export" => out.export = Some(PathBuf::from(it.next().ok_or_else(value)?)),
                "--raw-out" => out.raw_out = Some(PathBuf::from(it.next().ok_or_else(value)?)),
                "--mask-out" => out.mask_out = Some(PathBuf::from(it.next().ok_or_else(value)?)),
                "--enc-out" => out.enc_out = Some(PathBuf::from(it.next().ok_or_else(value)?)),
                "--dilate" => out.dilate = it.next().ok_or_else(value)?.parse()?,
                other => bail!("未知参数 {other}"),
            }
        }
        if out.export.is_some() {
            ensure!(out.raw_out.is_some(), "--export 需要 --raw-out");
            return Ok(out);
        }
        if out.video_w == 0 {
            out.video_w = if out.alpha_video.is_some() || out.mask.is_some() {
                out.color_w
            } else {
                out.color_w * 2
            };
        }
        ensure!(!out.video.as_os_str().is_empty(), "缺 --video");
        ensure!(!out.reference.as_os_str().is_empty(), "缺 --ref");
        Ok(out)
    }
}

/// 把可见像素的颜色向透明区外扩 `iterations` 圈；alpha 不变。
/// 目的：H.264 的色度块不要在“角色色 / 透明黑”之间硬切，减少边缘黑边。
fn dilate_color(frame: &[u8], w: usize, h: usize, iterations: u32) -> Vec<u8> {
    let mut cur = frame.to_vec();
    if iterations == 0 {
        return cur;
    }
    for _ in 0..iterations {
        let prev = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                if prev[o + 3] != 0 {
                    continue;
                }
                let mut sum = [0u32; 3];
                let mut n = 0u32;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let q = (ny as usize * w + nx as usize) * 4;
                        if prev[q + 3] == 0 {
                            continue;
                        }
                        sum[0] += u32::from(prev[q]);
                        sum[1] += u32::from(prev[q + 1]);
                        sum[2] += u32::from(prev[q + 2]);
                        n += 1;
                    }
                }
                cur[o] = sum[0].checked_div(n).unwrap_or(0) as u8;
                cur[o + 1] = sum[1].checked_div(n).unwrap_or(0) as u8;
                cur[o + 2] = sum[2].checked_div(n).unwrap_or(0) as u8;
            }
        }
    }
    cur
}

/// 把生产 `.fishanim` 逐帧导出成预乘 BGRA 裸流；高度奇数时补一行透明，
/// 让 H.264 的 4:2:0 编码尺寸为偶数。
fn export_fishanim(
    fish: &Path,
    raw_out: &Path,
    mask_out: Option<&Path>,
    enc_out: Option<&Path>,
    dilate: u32,
) -> Result<()> {
    let mut reader =
        feiyu_core::AnimReader::open(fish).with_context(|| format!("读不了 {}", fish.display()))?;
    let w = reader.header().width as usize;
    let h = reader.header().height as usize;
    let h_pad = (h + 1) & !1;
    let mut out = BufWriter::new(File::create(raw_out)?);
    let mut enc = enc_out.map(File::create).transpose()?.map(BufWriter::new);
    let mask_len = (w * h).div_ceil(8);
    let mut prev = vec![0u8; mask_len];
    let mut mask_raw = 0u64;
    let mut mask_zstd = 0u64;
    let mut masks: Vec<Vec<u8>> = Vec::new();
    for idx in 0..reader.frame_count() {
        let frame = reader.frame(idx)?;
        ensure!(frame.len() >= w * h * 4, "{idx} 帧长度不对");
        out.write_all(&frame[..w * h * 4])?;
        if h_pad > h {
            out.write_all(&vec![0u8; w * 4])?;
        }
        if let Some(enc) = enc.as_mut() {
            let filled = dilate_color(&frame[..w * h * 4], w, h, dilate);
            enc.write_all(&filled)?;
            if h_pad > h {
                enc.write_all(&vec![0u8; w * 4])?;
            }
        }

        // 顺带量一下「alpha 存成 1-bit 掩码 + 帧间 XOR + zstd」的大小：
        // 这是 H.264 有损 alpha 之外的另一种候选，不要求编解码器支持 alpha。
        let mut mask = vec![0u8; mask_len];
        for i in 0..w * h {
            if frame[i * 4 + 3] >= 128 {
                mask[i / 8] |= 1 << (i % 8);
            }
        }
        let delta: Vec<u8> = mask.iter().zip(&prev).map(|(a, b)| a ^ b).collect();
        let packed = zstd::bulk::compress(&delta, 3)?;
        mask_raw += mask_len as u64;
        mask_zstd += packed.len() as u64;
        masks.push(mask.clone());
        prev = mask;
    }
    out.flush()?;
    if let Some(enc) = enc.as_mut() {
        enc.flush()?;
    };
    println!(
        "EXPORT width={} height={} frames={}",
        w,
        h_pad,
        reader.frame_count()
    );
    println!("MASK raw={mask_raw} zstd={mask_zstd}");
    if enc_out.is_some() {
        println!("ENC_OUT width={} height={} dilate={}", w, h_pad, dilate);
    }
    if let Some(path) = mask_out {
        let payload: Vec<u8> = masks.concat();
        let packed = zstd::bulk::compress(&payload, 3)?;
        let mut f = BufWriter::new(File::create(path)?);
        f.write_all(b"FISHMSK1")?;
        f.write_all(&(w as u16).to_le_bytes())?;
        f.write_all(&(h as u16).to_le_bytes())?;
        f.write_all(&reader.frame_count().to_le_bytes())?;
        f.write_all(&packed)?;
        f.flush()?;
        println!("MASKFILE bytes={} path={}", packed.len(), path.display());
    }
    Ok(())
}
