//! 运行时取帧：随机访问 `.fishanim`，只解压目标帧。
//!
//! 一段动画 240 帧、每帧 200 KiB 时，全量加载要 50 MiB；随机取帧把峰值压到
//! 「1 帧暂存 + 1 帧成品」，这是内存目标能守住 60 MiB 的前提。
//!
//! 解压器是**复用**的：`zstd::bulk::decompress_to_buffer` 每次都新建上下文，
//! 反复分配解压表会让单帧多花一大截；复用一个 `Decompressor` 后单帧解压只要 0.16ms 量级。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::format::{self, AnimError, AnimHeader, Codec, FrameEntry};

/// 动画读取器。内部缓冲复用，连续取帧不产生分配。
pub struct AnimReader<R: Read + Seek> {
    r: R,
    header: AnimHeader,
    table: Vec<FrameEntry>,
    /// 成品帧（预乘 BGRA8），长度恒为 `header.byte_len()`。
    scratch: Vec<u8>,
    /// 磁盘上读出的原始负载（可能仍是压缩态）。
    staged: Vec<u8>,
    /// 差分帧解出的差值，累加到 `scratch` 上。
    delta: Vec<u8>,
    /// `scratch` 当前保存的是第几帧（`None` = 无效）。
    pos: Option<u32>,
    /// 累计解压次数（含差分的中间帧）。顺序播放时应当每帧只 +1，
    /// 明显偏大就说明在重复重解差分链。
    decodes: u32,
    /// 复用的解压上下文（每帧新建会反复分配解压表，实测拖慢 6 倍）。
    dec: zstd::bulk::Decompressor<'static>,
}

impl AnimReader<BufReader<File>> {
    /// 打开磁盘上的 `.fishanim`。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AnimError> {
        let file = File::open(path)?;
        Self::new(BufReader::with_capacity(64 * 1024, file))
    }
}

impl<R: Read + Seek> AnimReader<R> {
    /// 从任意可 seek 的源构造（测试用 [`std::io::Cursor`] 即可）。
    pub fn new(mut r: R) -> Result<Self, AnimError> {
        let (header, table) = format::read_header(&mut r)?;
        let len = header.byte_len();
        let dec = zstd::bulk::Decompressor::new().map_err(|e| AnimError::Zstd(e.to_string()))?;
        Ok(Self {
            r,
            header,
            table,
            scratch: vec![0; len],
            staged: Vec::new(),
            delta: Vec::new(),
            pos: None,
            decodes: 0,
            dec,
        })
    }

    /// 帧头。
    pub fn header(&self) -> &AnimHeader {
        &self.header
    }

    /// 帧表。
    pub fn table(&self) -> &[FrameEntry] {
        &self.table
    }

    /// 帧数。
    pub fn frame_count(&self) -> u32 {
        self.header.frame_count
    }

    /// 累计解压的帧次数。用于验证「顺序播放不重解差分链」。
    pub fn decodes(&self) -> u32 {
        self.decodes
    }

    /// 取第 `idx` 帧，返回预乘 BGRA8 像素（长度 = `width × height × 4`）。
    ///
    /// 启用帧间差分时，随机取帧会从本链的关键帧逐帧累加（最多 `key_every` 步）；
    /// 顺序播放（`idx` 恰为上一帧 +1）只累加一步。
    /// 返回的切片在下一次调用 `frame` 前有效。
    pub fn frame(&mut self, idx: u32) -> Result<&[u8], AnimError> {
        let count = self.table.len() as u32;
        if idx >= count {
            return Err(AnimError::FrameOutOfRange { idx, count });
        }
        if self.pos == Some(idx) {
            return Ok(&self.scratch);
        }

        let key = if self.header.uses_delta() {
            self.header.key_every as u32
        } else {
            0
        };
        if key > 1 {
            // 链号（而不是链首的帧号）必须同量纲才能比：早期版本拿
            // `p / key` 去比 `idx / key * key`，只有第 0 条链碰巧相等，
            // 其余每帧都退化成「从链首重解整条链」，慢 8 倍。
            let resume = matches!(self.pos, Some(p) if p.checked_add(1) == Some(idx) && p / key == idx / key);
            if resume {
                self.decode_one(idx, false)?;
            } else {
                let chain = idx / key * key;
                self.decode_one(chain, true)?;
                for step in (chain + 1)..=idx {
                    self.decode_one(step, false)?;
                }
            }
        } else {
            self.decode_one(idx, true)?;
        }
        self.pos = Some(idx);
        Ok(&self.scratch)
    }

    /// 解单帧：`is_base` 时直接写入 `scratch`，否则先解到 `delta` 再累加。
    fn decode_one(&mut self, idx: u32, is_base: bool) -> Result<(), AnimError> {
        let count = self.table.len() as u32;
        let entry = *self
            .table
            .get(idx as usize)
            .ok_or(AnimError::FrameOutOfRange { idx, count })?;
        let expected = self.header.byte_len();
        self.decodes += 1;

        self.staged.resize(entry.len as usize, 0);
        self.r.seek(SeekFrom::Start(entry.offset as u64))?;
        self.r.read_exact(&mut self.staged)?;

        let target = if is_base {
            &mut self.scratch
        } else {
            &mut self.delta
        };
        if target.len() != expected {
            target.resize(expected, 0);
        }
        match entry.codec {
            Codec::Raw => {
                if self.staged.len() != expected {
                    return Err(AnimError::FrameSize {
                        expected,
                        got: self.staged.len(),
                        idx,
                    });
                }
                target.copy_from_slice(&self.staged);
            }
            Codec::Zstd => {
                // 解压进预分配的缓冲，复用上下文，避免每帧新建解压表。
                let got = self
                    .dec
                    .decompress_to_buffer(&self.staged, target)
                    .map_err(|e| AnimError::Zstd(e.to_string()))?;
                if got != expected {
                    return Err(AnimError::FrameSize { expected, got, idx });
                }
            }
        }

        if !is_base {
            for (px, d) in self.scratch.iter_mut().zip(self.delta.iter()) {
                *px = px.wrapping_add(*d);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::format::{encode, encode_with, write, AnimHeader};
    use std::io::Cursor;

    const W: u16 = 32;
    const H: u16 = 18;
    const FRAMES: u32 = 3;

    /// 造一段有梯度的像素：第 n 帧整体亮度随 n 变化，便于断言取到了正确的帧。
    fn frame_bytes(seed: u8) -> Vec<u8> {
        let n = W as usize * H as usize;
        let mut v = Vec::with_capacity(n * 4);
        for i in 0..n {
            v.extend_from_slice(&[seed, i as u8, 0u8, 255u8]);
        }
        v
    }

    fn build(level: i32) -> Vec<u8> {
        let frames: Vec<Vec<u8>> = (0..FRAMES).map(|i| frame_bytes(i as u8 * 40)).collect();
        let file = encode(AnimHeader::new(W, H, 24.0, H), frames, level).expect("encode");
        let mut buf = Vec::new();
        write(&mut buf, &file).expect("write");
        buf
    }

    fn check_all_frames(level: i32) {
        let buf = build(level);
        let mut reader = AnimReader::new(Cursor::new(buf)).expect("open");
        assert_eq!(reader.frame_count(), FRAMES);
        assert_eq!(reader.header().fps, 24.0);

        for i in 0..FRAMES {
            let got = reader.frame(i).expect("frame").to_vec();
            assert_eq!(got.len(), W as usize * H as usize * 4);
            assert_eq!(got, frame_bytes(i as u8 * 40), "第 {i} 帧内容不符");
        }
    }

    #[test]
    fn reads_zstd_frames() {
        check_all_frames(3);
    }

    #[test]
    fn reads_raw_frames() {
        check_all_frames(0);
    }

    #[test]
    fn random_access_is_order_independent() {
        let mut reader = AnimReader::new(Cursor::new(build(3))).expect("open");
        let want2 = frame_bytes(80);
        let want0 = frame_bytes(0);
        assert_eq!(reader.frame(2).unwrap(), &want2[..]);
        assert_eq!(reader.frame(0).unwrap(), &want0[..]);
        assert_eq!(reader.frame(2).unwrap(), &want2[..]);
    }

    #[test]
    fn rejects_out_of_range() {
        let mut reader = AnimReader::new(Cursor::new(build(0))).expect("open");
        match reader.frame(FRAMES) {
            Err(AnimError::FrameOutOfRange { idx, count }) => {
                assert_eq!(idx, FRAMES);
                assert_eq!(count, FRAMES);
            }
            other => panic!("期望 FrameOutOfRange，实际 {other:?}"),
        }
    }

    #[test]
    fn rejects_truncated_file() {
        let mut buf = build(3);
        buf.truncate(buf.len() - 8);
        let mut reader = AnimReader::new(Cursor::new(buf)).expect("open");
        assert!(reader.frame(FRAMES - 1).is_err(), "截断后取最后一帧应失败");
    }

    /// 造一串“相邻帧略有变化”的画面，用于差分测试。
    fn smooth(n: u32) -> Vec<Vec<u8>> {
        let total = W as usize * H as usize;
        (0..n)
            .map(|f| {
                let mut v = Vec::with_capacity(total * 4);
                for i in 0..total {
                    let g = ((i + f as usize * 3) % 256) as u8;
                    v.extend_from_slice(&[g, g.wrapping_mul(3), 7, 255]);
                }
                v
            })
            .collect()
    }

    fn build_delta(frames: Vec<Vec<u8>>, key: u16) -> Vec<u8> {
        let file = encode_with(AnimHeader::new(W, H, 24.0, H), frames, 3, key).expect("encode");
        let mut buf = Vec::new();
        write(&mut buf, &file).expect("write");
        buf
    }

    #[test]
    fn reads_delta_frames_in_any_order() {
        let frames = smooth(10);
        let mut reader =
            AnimReader::new(Cursor::new(build_delta(frames.clone(), 4))).expect("open");
        assert!(reader.header().uses_delta());
        // 乱序取帧，包括链首、链中、链尾与跨链跳转。
        for idx in [7u32, 0, 3, 9, 1, 8, 4] {
            assert_eq!(
                reader.frame(idx).unwrap(),
                &frames[idx as usize][..],
                "第 {idx} 帧不符"
            );
        }
        // 顺序播放只累加一步。
        for idx in 0..10u32 {
            assert_eq!(reader.frame(idx).unwrap(), &frames[idx as usize][..]);
        }
    }

    #[test]
    fn sequential_playback_decodes_one_per_frame() {
        let frames = smooth(20);
        let mut reader =
            AnimReader::new(Cursor::new(build_delta(frames.clone(), 4))).expect("open");
        for idx in 0..20u32 {
            assert_eq!(
                reader.frame(idx).unwrap(),
                &frames[idx as usize][..],
                "第 {idx} 帧不符"
            );
        }
        // 这是回归测试：早期版本把「链号」和「链首帧号」相加比较，
        // 从第 2 条差分链起每帧都重解整条链（钉床 8 倍），这里钉死次数。
        assert_eq!(reader.decodes(), 20, "顺序播放重复解了差分链");

        // 重复要同一帧不应再解压（命中缓存）。
        let n = reader.decodes();
        let _ = reader.frame(19).unwrap();
        assert_eq!(reader.decodes(), n, "重复取同一帧不应再解压");
    }

    #[test]
    fn delta_pack_is_smaller_than_intra() {
        let frames = smooth(16);
        let intra = build_delta(frames.clone(), 0).len();
        let delta = build_delta(frames, 8).len();
        assert!(delta < intra, "差分应更小：intra={intra} delta={delta}");
    }
}
