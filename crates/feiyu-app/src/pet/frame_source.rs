//! 帧源抽象：`Pet` 只认「第几帧 → 预乘 BGRA」，不关心是 `.fishanim` 还是
//! `.fishvid + .fishmask`。
//!
//! 这样播放器、接缝淡入、镜像、点击命中都不用改；换编码器只换一个实现。

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use feiyu_core::anim::{AnimEntry, AnimReader, ClipCodec, MaskSet};
use feiyu_platform::VideoReader;

/// 一段动画的帧来源。
pub trait FrameSource {
    fn frame_count(&self) -> u32;
    /// 返回预乘 BGRA，尺寸 = 清单里的 `size`。
    fn frame(&mut self, idx: u32) -> Result<&[u8]>;
}

type FishAnimReader = AnimReader<BufReader<File>>;

/// 旧 `.fishanim`：预乘 BGRA + zstd。
struct FishAnimSource {
    reader: FishAnimReader,
}

impl FishAnimSource {
    fn open(path: &Path, entry: &AnimEntry) -> Result<Self> {
        let reader =
            AnimReader::open(path).with_context(|| format!("打不开动画 {}", path.display()))?;
        let header = *reader.header();
        ensure!(
            header.width == entry.width() && header.height == entry.height(),
            "清单与文件尺寸不符（{}）：清单 {}×{}，文件 {}×{}；请重新生成 manifest",
            entry.id,
            entry.width(),
            entry.height(),
            header.width,
            header.height
        );
        ensure!(
            header.frame_count == entry.frames,
            "清单与文件帧数不符（{}）：清单 {}，文件 {}；请重新生成 manifest",
            entry.id,
            entry.frames,
            header.frame_count
        );
        Ok(Self { reader })
    }
}

impl FrameSource for FishAnimSource {
    fn frame_count(&self) -> u32 {
        self.reader.frame_count()
    }

    fn frame(&mut self, idx: u32) -> Result<&[u8]> {
        Ok(self.reader.frame(idx)?)
    }
}

/// H.264 颜色 + 1-bit 掩码。
struct VideoSource {
    video: VideoReader,
    mask: MaskSet,
    out: Vec<u8>,
    width: u16,
    height: u16,
}

impl VideoSource {
    fn open(video: &Path, mask: &Path, entry: &AnimEntry, fps: f32) -> Result<Self> {
        let mask = MaskSet::load(mask).with_context(|| format!("打不开掩码 {}", mask.display()))?;
        ensure!(
            mask.matches(entry.width(), entry.height(), entry.frames),
            "掩码与清单不符（{}）：清单 {}×{} / {} 帧，掩码 {}×{} / {} 帧",
            entry.id,
            entry.width(),
            entry.height(),
            entry.frames,
            mask.width(),
            mask.height(),
            mask.frame_count()
        );
        let video = VideoReader::open(video, entry.width(), entry.height(), fps, entry.frames)
            .with_context(|| format!("打不开视频 {}", video.display()))?;
        let out = vec![0u8; entry.width() as usize * entry.height() as usize * 4];
        Ok(Self {
            video,
            mask,
            out,
            width: entry.width(),
            height: entry.height(),
        })
    }
}

impl FrameSource for VideoSource {
    fn frame_count(&self) -> u32 {
        self.video.frame_count()
    }

    fn frame(&mut self, idx: u32) -> Result<&[u8]> {
        let Self {
            video,
            mask,
            out,
            width,
            height,
        } = self;
        let color = video.frame(idx)?;
        let bits = mask.frame_bits(idx)?;
        let pixels = *width as usize * *height as usize;
        ensure!(color.len() >= pixels * 4, "视频帧长度不足");
        for i in 0..pixels {
            let o = i * 4;
            if bits[i / 8] & (1 << (i % 8)) != 0 {
                out[o..o + 4].copy_from_slice(&color[o..o + 4]);
                out[o + 3] = 255;
            } else {
                out[o..o + 4].fill(0);
            }
        }
        Ok(out)
    }
}

/// 按 manifest 的 `codec` 打开对应帧源。
pub fn open(dir: &Path, entry: &AnimEntry, fps: f32) -> Result<Box<dyn FrameSource>> {
    let file = entry.path_in(dir);
    match entry.codec {
        ClipCodec::FishAnim => Ok(Box::new(FishAnimSource::open(&file, entry)?)),
        ClipCodec::H264 => {
            let mask = entry
                .mask_path_in(dir)
                .with_context(|| format!("{} 是 h264，但清单没有 mask", entry.id))?;
            Ok(Box::new(VideoSource::open(&file, &mask, entry, fps)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../feiyu-platform/tests/fixtures/tiny.fishvid")
    }

    fn entry() -> AnimEntry {
        AnimEntry {
            id: "tiny".to_owned(),
            name: "tiny".to_owned(),
            file: "tiny.fishvid".to_owned(),
            codec: ClipCodec::H264,
            mask: Some("tiny.fishmask".to_owned()),
            size: [64, 64],
            origin: [0, 0],
            baseline: 64,
            frames: 24,
            bytes: 0,
        }
    }

    #[test]
    fn video_source_applies_mask_and_keeps_orientation() {
        let mask_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fish_source_tiny.fishmask");
        let alpha = {
            // 左半不透明、右半透明：同时验证「拷贝颜色」和「清零透明区」。
            let mut a = vec![0u8; 64 * 64];
            for y in 0..64 {
                for x in 0..32 {
                    a[y * 64 + x] = 255;
                }
            }
            a
        };
        let frames = vec![alpha; 24];
        MaskSet::from_alpha_frames(64, 64, &frames)
            .unwrap()
            .save(&mask_path)
            .unwrap();

        let mut source = VideoSource::open(&fixture(), &mask_path, &entry(), 24.0).unwrap();
        assert_eq!(source.frame_count(), 24);
        let f0 = source.frame(0).unwrap().to_vec();
        assert_eq!(f0.len(), 64 * 64 * 4);
        assert_eq!(f0[(8 * 64 + 8) * 4 + 3], 255, "左半应不透明");
        assert_eq!(f0[(8 * 64 + 56) * 4 + 3], 0, "右半应透明");
        assert_eq!(&f0[(8 * 64 + 56) * 4..(8 * 64 + 56) * 4 + 4], &[0, 0, 0, 0]);
        let top = &f0[(8 * 64 + 8) * 4..];
        let bottom = &f0[(56 * 64 + 8) * 4..];
        assert!(top[0] < 100 && top[2] > 150, "上半应为红，BGR={top:?}");
        assert!(
            bottom[0] > 150 && bottom[2] < 100,
            "下半应为蓝，BGR={bottom:?}"
        );

        let f23 = source.frame(23).unwrap().to_vec();
        assert_eq!(f23.len(), 64 * 64 * 4);
        let back = source.frame(0).unwrap().to_vec();
        assert_eq!(back.len(), 64 * 64 * 4);
    }
}
