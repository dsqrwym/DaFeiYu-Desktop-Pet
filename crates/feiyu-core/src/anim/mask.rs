//! `.fishmask`：1-bit alpha 掩码。
//!
//! H.264 颜色视频不携带 alpha；透明信息单独存成 1 bit/像素。格式刻意做成
//! “整份解压、任意帧随机读”：100 段的掩码压缩后只有几十 KB，解压后 1~2MB，
//! 换段时重新加载完全可接受，比在视频里编 alpha 更稳（H.264 alpha 是有损的，
//! 硬边缘会出现光晕）。
//!
//! 字节布局（小端）：
//!
//! ```text
//! 0   8  magic        "FISHMSK1"
//! 8   2  width        u16
//! 10  2  height       u16
//! 12  4  frames       u32
//! 16  ... zstd(逐帧 1-bit 掩码，frame-major，LSB-first)
//! ```
//!
//! 每帧 `ceil(width × height / 8)` 字节；bit=1 表示 alpha ≥ 128。

use std::fs;
use std::io;
use std::path::Path;

use thiserror::Error;

/// 文件魔数，末位数字变化即表示不兼容。
pub const MAGIC: &[u8; 8] = b"FISHMSK1";
/// 固定头长度。
pub const HEADER_SIZE: usize = 16;
/// 单帧掩码压缩级别；掩码本身很小，3 已经足够。
const ZSTD_LEVEL: i32 = 3;
/// 与 `.fishanim` 保持同量级的防损坏上限。
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_FRAMES: u32 = 100_000;

/// `.fishmask` 读写错误。
#[derive(Debug, Error)]
pub enum MaskError {
    #[error("读写出错: {0}")]
    Io(#[from] io::Error),
    #[error("魔数不匹配，不是 .fishmask 文件")]
    BadMagic,
    #[error("mask 头部损坏: {0}")]
    Corrupt(&'static str),
    #[error("mask 尺寸/帧数与清单不符")]
    SizeMismatch,
    #[error("mask 帧号 {idx} 越界（共 {count} 帧）")]
    FrameOutOfRange { idx: u32, count: u32 },
    #[error("第 {idx} 帧 alpha 长度不对：期望 {expected}，实际 {got}")]
    AlphaSize {
        idx: usize,
        expected: usize,
        got: usize,
    },
    #[error("mask 像素坐标越界: ({x}, {y}) 不在 {width}×{height} 内")]
    PixelOutOfRange {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
    #[error("mask 解压失败: {0}")]
    Zstd(String),
}

/// 一整段动画的 1-bit alpha 掩码。
#[derive(Debug, Clone)]
pub struct MaskSet {
    width: u16,
    height: u16,
    frames: u32,
    mask_len: usize,
    /// `frames × mask_len` 字节，frame-major。
    bits: Vec<u8>,
}

impl MaskSet {
    /// 从每帧 `width × height` 的 alpha 平面构造。alpha ≥ 128 记为不透明。
    pub fn from_alpha_frames(
        width: u16,
        height: u16,
        frames: &[Vec<u8>],
    ) -> Result<Self, MaskError> {
        if width == 0 || height == 0 {
            return Err(MaskError::Corrupt("宽或高为 0"));
        }
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(MaskError::Corrupt("分辨率超限"));
        }
        if frames.is_empty() {
            return Err(MaskError::Corrupt("没有任何帧"));
        }
        if frames.len() as u64 > u64::from(MAX_FRAMES) {
            return Err(MaskError::Corrupt("帧数超限"));
        }

        let pixels = width as usize * height as usize;
        let mask_len = pixels.div_ceil(8);
        let total = mask_len
            .checked_mul(frames.len())
            .ok_or(MaskError::Corrupt("掩码长度溢出"))?;
        let mut bits = Vec::with_capacity(total);
        for (idx, alpha) in frames.iter().enumerate() {
            if alpha.len() != pixels {
                return Err(MaskError::AlphaSize {
                    idx,
                    expected: pixels,
                    got: alpha.len(),
                });
            }
            let mut mask = vec![0u8; mask_len];
            for (i, &a) in alpha.iter().enumerate() {
                if a >= 128 {
                    mask[i / 8] |= 1 << (i % 8);
                }
            }
            bits.extend_from_slice(&mask);
        }

        Ok(Self {
            width,
            height,
            frames: frames.len() as u32,
            mask_len,
            bits,
        })
    }

    /// 从磁盘读取并解压。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, MaskError> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// 从完整文件字节解析。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MaskError> {
        if bytes.len() < HEADER_SIZE {
            return Err(MaskError::Corrupt("文件短于 16 字节头"));
        }
        if &bytes[..8] != MAGIC {
            return Err(MaskError::BadMagic);
        }
        let width = u16::from_le_bytes([bytes[8], bytes[9]]);
        let height = u16::from_le_bytes([bytes[10], bytes[11]]);
        let frames = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        if width == 0 || height == 0 {
            return Err(MaskError::Corrupt("宽或高为 0"));
        }
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(MaskError::Corrupt("分辨率超限"));
        }
        if frames == 0 || frames > MAX_FRAMES {
            return Err(MaskError::Corrupt("帧数非法"));
        }

        let mask_len = (width as usize * height as usize).div_ceil(8);
        let expected = mask_len
            .checked_mul(frames as usize)
            .ok_or(MaskError::Corrupt("掩码长度溢出"))?;
        let bits = zstd::bulk::decompress(&bytes[HEADER_SIZE..], expected)
            .map_err(|e| MaskError::Zstd(e.to_string()))?;
        if bits.len() != expected {
            return Err(MaskError::Corrupt("解压后长度不符"));
        }

        Ok(Self {
            width,
            height,
            frames,
            mask_len,
            bits,
        })
    }

    /// 编码成完整文件字节（含 16 字节头）。
    pub fn to_bytes(&self) -> Result<Vec<u8>, MaskError> {
        let packed = zstd::bulk::compress(&self.bits, ZSTD_LEVEL)
            .map_err(|e| MaskError::Zstd(e.to_string()))?;
        let mut out = Vec::with_capacity(HEADER_SIZE + packed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.frames.to_le_bytes());
        out.extend_from_slice(&packed);
        Ok(out)
    }

    /// 写入磁盘。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), MaskError> {
        fs::write(path, self.to_bytes()?)?;
        Ok(())
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn frame_count(&self) -> u32 {
        self.frames
    }

    /// 每帧掩码字节数。
    pub fn mask_len(&self) -> usize {
        self.mask_len
    }

    /// 某帧的原始位图（LSB-first）。
    pub fn frame_bits(&self, idx: u32) -> Result<&[u8], MaskError> {
        if idx >= self.frames {
            return Err(MaskError::FrameOutOfRange {
                idx,
                count: self.frames,
            });
        }
        let at = idx as usize * self.mask_len;
        Ok(&self.bits[at..at + self.mask_len])
    }

    /// 某帧某像素的 alpha：只有 0 或 255。
    pub fn alpha_at(&self, idx: u32, x: u16, y: u16) -> Result<u8, MaskError> {
        if x >= self.width || y >= self.height {
            return Err(MaskError::PixelOutOfRange {
                x,
                y,
                width: self.width,
                height: self.height,
            });
        }
        let bits = self.frame_bits(idx)?;
        let i = y as usize * self.width as usize + x as usize;
        Ok(if bits[i / 8] & (1 << (i % 8)) != 0 {
            255
        } else {
            0
        })
    }

    /// 尺寸/帧数是否与清单一致。
    pub fn matches(&self, width: u16, height: u16, frames: u32) -> bool {
        self.width == width && self.height == height && self.frames == frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_frame(width: u16, height: u16, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height as usize {
            for x in 0..width as usize {
                v.push(f(x, y));
            }
        }
        v
    }

    #[test]
    fn roundtrip_preserves_bits_and_alpha() {
        let frames = vec![
            alpha_frame(5, 3, |x, y| if (x + y) % 2 == 0 { 255 } else { 0 }),
            alpha_frame(5, 3, |x, _| if x == 2 { 255 } else { 0 }),
        ];
        let set = MaskSet::from_alpha_frames(5, 3, &frames).expect("构造");
        assert_eq!(set.width(), 5);
        assert_eq!(set.height(), 3);
        assert_eq!(set.frame_count(), 2);
        assert_eq!(set.mask_len(), 2);

        let bytes = set.to_bytes().expect("编码");
        let back = MaskSet::from_bytes(&bytes).expect("解析");
        for (f, frame) in frames.iter().enumerate() {
            for y in 0..3 {
                for x in 0..5 {
                    assert_eq!(
                        back.alpha_at(f as u32, x as u16, y as u16).unwrap(),
                        frame[y * 5 + x],
                        "frame {f} ({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn threshold_is_128_and_bit_order_is_lsb_first() {
        let frame = vec![255, 127, 128, 0, 255, 0, 0, 255];
        let set = MaskSet::from_alpha_frames(8, 1, &[frame]).expect("构造");
        assert_eq!(set.frame_bits(0).unwrap(), &[0b1001_0101]);
        assert_eq!(set.alpha_at(0, 0, 0).unwrap(), 255);
        assert_eq!(set.alpha_at(0, 1, 0).unwrap(), 0);
        assert_eq!(set.alpha_at(0, 2, 0).unwrap(), 255);
        assert_eq!(set.alpha_at(0, 7, 0).unwrap(), 255);
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(matches!(
            MaskSet::from_bytes(b"NOTMASK!"),
            Err(MaskError::Corrupt(_))
        ));

        let mut bytes = MaskSet::from_alpha_frames(1, 1, &[vec![255]])
            .unwrap()
            .to_bytes()
            .unwrap();
        bytes[0] = b'X';
        assert!(matches!(
            MaskSet::from_bytes(&bytes),
            Err(MaskError::BadMagic)
        ));

        let good = MaskSet::from_alpha_frames(1, 1, &[vec![255]])
            .unwrap()
            .to_bytes()
            .unwrap();
        assert!(matches!(
            MaskSet::from_bytes(&good[..10]),
            Err(MaskError::Corrupt(_))
        ));
    }

    #[test]
    fn rejects_wrong_alpha_length_and_empty_frames() {
        assert!(matches!(
            MaskSet::from_alpha_frames(2, 2, &[vec![0, 0, 0]]),
            Err(MaskError::AlphaSize { .. })
        ));
        assert!(matches!(
            MaskSet::from_alpha_frames(2, 2, &[]),
            Err(MaskError::Corrupt(_))
        ));
        assert!(matches!(
            MaskSet::from_alpha_frames(0, 2, &[vec![]]),
            Err(MaskError::Corrupt(_))
        ));
    }

    #[test]
    fn reads_prototype_mask_when_present() {
        // 原型产物在 target/ 下，不入库；有就验证格式兼容，没就跳过。
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/proto_h264/watermelon.mask");
        if !path.exists() {
            return;
        }
        let set = MaskSet::load(path).expect("读原型 mask");
        assert_eq!((set.width(), set.height()), (201, 239));
        assert_eq!(set.frame_count(), 241);
        assert_eq!(set.mask_len(), (201 * 239usize).div_ceil(8));
    }

    #[test]
    fn frame_and_pixel_bounds_are_checked() {
        let set = MaskSet::from_alpha_frames(2, 2, &[vec![0; 4]]).unwrap();
        assert!(matches!(
            set.frame_bits(1),
            Err(MaskError::FrameOutOfRange { .. })
        ));
        assert!(matches!(
            set.alpha_at(0, 2, 0),
            Err(MaskError::PixelOutOfRange { .. })
        ));
        assert!(set.matches(2, 2, 1));
        assert!(!set.matches(2, 2, 2));
    }
}
