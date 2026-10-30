//! `.fishanim` 容器格式：定长帧头 + 帧表 + 分帧压缩负载。
//!
//! 设计目标：运行时零转换成本取单帧——随机跳转、只解压目标帧，不把整段动画读进内存。
//! 像素统一为**预乘 BGRA8**，每帧字节数 = `width × height × 4`，与 Windows
//! `UpdateLayeredWindow` 要求的位图格式一致，取到帧即可直接提交。
//!
//! 字节布局（小端，全部定长）：
//!
//! ```text
//! 0   8  magic        "FISHANM1"
//! 8   2  version
//! 10  2  flags        bit0 = 已预乘 alpha
//! 12  2  width
//! 14  2  height
//! 16  4  frame_count
//! 20  4  fps          f32 的位模式
//! 24  2  baseline     脚底线（帧内 y），用于贴底对齐
//! 26  2  key_every   差分关键帧间隔（0/1 = 不差分）
//! 28  4  table_off    帧表偏移（恒为 48）
//! 32  4  data_off     负载起始偏移
//! 36  4  payload_len  负载总字节
//! 40  2  origin_x     本帧左上角在原画布中的 x
//! 42  2  origin_y     本帧左上角在原画布中的 y
//! 44  4  reserved1
//! 48  ×N 帧表项：offset u32 | len u32 | codec u8 | pad[3]
//! ```
//!
//! 帧表项的 `offset` 是**文件内绝对偏移**，因此读取端只需一次 `seek` 即可定位单帧。
//!
//! ## 裁切与坐标系
//!
//! 打包时会把每段动画裁到「全片不透明像素的并集包围盒」——角色只占 16:9 画面的
//! 三成左右，裁掉空白能同时省下磁盘和每帧解码量。为了保证换动画时角色不跳位，
//! `origin_x/origin_y` 记录裁切帧左上角在**同一张原画布**上的位置（所有动画共享同
//! 一张画布），渲染端把窗口按 `origin × 缩放比` 平移即可还原原始对齐。
//!
//! 因此 `baseline` 是**帧内**的脚底线；换算到画布坐标要自己加上 `origin_y`。

use std::io::{self, Read, Write};

/// 文件魔数，末位数字变化即表示不兼容。
pub const MAGIC: &[u8; 8] = b"FISHANM1";
/// 当前格式版本。
pub const VERSION: u16 = 1;
/// `flags` 位：像素已预乘 alpha。
pub const FLAG_PREMULTIPLIED: u16 = 0x0001;
/// `flags` 位：帧负载为「与前一帧的逐字节差」，每 `key_every` 帧插一个关键帧。
pub const FLAG_DELTA: u16 = 0x0002;
/// 帧头固定字节数。
pub const HEADER_SIZE: u32 = 48;
/// 帧表每项字节数。
pub const ENTRY_SIZE: u32 = 12;

/// 单帧像素数上限，用于拒绝明显损坏的头部（约 64MP）。
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// 帧数上限，防止损坏头部导致巨量分配。
const MAX_FRAMES: u32 = 100_000;

/// 帧负载的编码方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    /// `zstd` 压缩。
    Zstd = 0,
    /// 未压缩原样存放（压缩无收益时自动选用）。
    Raw = 1,
}

impl Codec {
    fn from_u8(v: u8) -> Result<Self, AnimError> {
        match v {
            0 => Ok(Self::Zstd),
            1 => Ok(Self::Raw),
            other => Err(AnimError::UnknownCodec(other)),
        }
    }

    fn as_u8(self) -> u8 {
        self as u8
    }
}

/// 帧头。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimHeader {
    /// 见 [`FLAG_PREMULTIPLIED`]。
    pub flags: u16,
    /// 帧宽（像素）。
    pub width: u16,
    /// 帧高（像素）。
    pub height: u16,
    /// 帧数。
    pub frame_count: u32,
    /// 播放帧率。
    pub fps: f32,
    /// 脚底线（**帧内** y 坐标），贴底对齐用；画布坐标 = `origin_y + baseline`。
    pub baseline: u16,
    /// 帧间差分的关键帧间隔（>1 时启用差分；0/1 表示每帧独立）。
    /// 解码第 i 帧只需从 `i / key_every * key_every` 逐帧累加，最多 `key_every` 步。
    pub key_every: u16,
    /// 本帧左上角在原画布中的 x（未裁切时为 0）。
    pub origin_x: u16,
    /// 本帧左上角在原画布中的 y（未裁切时为 0）。
    pub origin_y: u16,
}

impl AnimHeader {
    /// 按宽高与 fps 构造，默认标记为已预乘。
    pub fn new(width: u16, height: u16, fps: f32, baseline: u16) -> Self {
        Self {
            flags: FLAG_PREMULTIPLIED,
            width,
            height,
            frame_count: 0,
            fps,
            baseline,
            key_every: 0,
            origin_x: 0,
            origin_y: 0,
        }
    }

    /// 标记裁切帧在原画布中的位置。
    pub fn with_origin(mut self, x: u16, y: u16) -> Self {
        self.origin_x = x;
        self.origin_y = y;
        self
    }

    /// 启用帧间差分，每 `key_every` 帧插入一个关键帧。
    pub fn with_key_every(mut self, key_every: u16) -> Self {
        self.key_every = key_every;
        if key_every > 1 {
            self.flags |= FLAG_DELTA;
        }
        self
    }

    /// 是否使用帧间差分。
    pub fn uses_delta(&self) -> bool {
        self.flags & FLAG_DELTA != 0 && self.key_every > 1
    }

    /// 单帧字节数（预乘 BGRA8）。
    pub fn byte_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// 像素是否已预乘 alpha。
    pub fn is_premultiplied(&self) -> bool {
        self.flags & FLAG_PREMULTIPLIED != 0
    }

    fn validate(&self) -> Result<(), AnimError> {
        if self.width == 0 || self.height == 0 {
            return Err(AnimError::Corrupt("宽或高为 0"));
        }
        if self.width as u64 * self.height as u64 > MAX_PIXELS {
            return Err(AnimError::Corrupt("分辨率超限"));
        }
        if !self.fps.is_finite() || self.fps <= 0.0 {
            return Err(AnimError::Corrupt("fps 非法"));
        }
        if self.baseline > self.height {
            return Err(AnimError::Corrupt("脚底线越界"));
        }
        Ok(())
    }
}

/// 帧表项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameEntry {
    /// 负载在文件内的绝对偏移。
    pub offset: u32,
    /// 负载字节数。
    pub len: u32,
    /// 编码方式。
    pub codec: Codec,
}

/// 一段待写出的动画：头 + 帧表 + 连续负载。
#[derive(Debug, Clone)]
pub struct AnimFile {
    pub header: AnimHeader,
    pub entries: Vec<FrameEntry>,
    pub payload: Vec<u8>,
}

/// 把一串帧编码成 [`AnimFile`]。
///
/// `level` 为 zstd 压缩级别；`<= 0` 表示不压缩（全部走 [`Codec::Raw`]）。
/// 单帧压缩后若不小于原始大小，也会自动退回 [`Codec::Raw`]——避免为已高度压缩的
/// 内容白白付解压成本。
pub fn encode(
    header: AnimHeader,
    frames: impl IntoIterator<Item = Vec<u8>>,
    level: i32,
) -> Result<AnimFile, AnimError> {
    encode_with(header, frames, level, 0)
}

/// 同 [`encode`]，但开启帧间差分：每 `key_every` 帧插一个关键帧，其余帧存
/// 「与前一帧的逐字节差」。
///
/// 为什么值得：动画相邻帧的透明区与静止背景占比极大，差分后大量字节为 0，
/// 实测能把压缩比从 7× 提到 20× 以上；代价是随机取帧要先从链首累加，
/// 最多 `key_every` 步（顺序播放时只需累加一步）。`key_every <= 1` 即不启用。
pub fn encode_with(
    header: AnimHeader,
    frames: impl IntoIterator<Item = Vec<u8>>,
    level: i32,
    key_every: u16,
) -> Result<AnimFile, AnimError> {
    let mut header = header.with_key_every(key_every);
    header.validate()?;
    header.flags |= FLAG_PREMULTIPLIED;

    let key = if header.key_every > 1 {
        header.key_every as u32
    } else {
        1
    };
    let expected = header.byte_len();
    let mut entries = Vec::new();
    let mut payload = Vec::new();
    let mut count = 0u32;
    let mut prev: Option<Vec<u8>> = None;

    for (idx, frame) in frames.into_iter().enumerate() {
        let idx = idx as u32;
        if frame.len() != expected {
            return Err(AnimError::FrameSize {
                expected,
                got: frame.len(),
                idx,
            });
        }
        // 链首（idx % key == 0）存原图，其余存差值。
        let source: Vec<u8> = if key > 1 && idx % key != 0 {
            let base = prev.as_ref().expect("差分帧必然有前一帧");
            frame
                .iter()
                .zip(base.iter())
                .map(|(a, b)| a.wrapping_sub(*b))
                .collect()
        } else {
            frame.clone()
        };
        prev = Some(frame);

        let (bytes, codec) = if level > 0 {
            match zstd::bulk::compress(&source, level) {
                Ok(packed) if packed.len() < source.len() => (packed, Codec::Zstd),
                // 压不动就直接存原样，读的时候省一次解压。
                _ => (source, Codec::Raw),
            }
        } else {
            (source, Codec::Raw)
        };
        entries.push(FrameEntry {
            offset: 0, // 下面统一回填
            len: bytes.len() as u32,
            codec,
        });
        payload.extend_from_slice(&bytes);
        count += 1;
    }

    if count == 0 {
        return Err(AnimError::Corrupt("帧数为 0"));
    }
    header.frame_count = count;

    // 回填绝对偏移：负载紧跟在「头部 + 帧表」之后。
    let mut cursor = HEADER_SIZE + count * ENTRY_SIZE;
    for entry in &mut entries {
        entry.offset = cursor;
        cursor += entry.len;
    }

    Ok(AnimFile {
        header,
        entries,
        payload,
    })
}

/// 把 [`AnimFile`] 写出到目标。
pub fn write<W: Write>(w: &mut W, file: &AnimFile) -> Result<(), AnimError> {
    let mut head = [0u8; HEADER_SIZE as usize];
    head[0..8].copy_from_slice(MAGIC);
    head[8..10].copy_from_slice(&VERSION.to_le_bytes());
    head[10..12].copy_from_slice(&file.header.flags.to_le_bytes());
    head[12..14].copy_from_slice(&file.header.width.to_le_bytes());
    head[14..16].copy_from_slice(&file.header.height.to_le_bytes());
    head[16..20].copy_from_slice(&file.header.frame_count.to_le_bytes());
    head[20..24].copy_from_slice(&file.header.fps.to_bits().to_le_bytes());
    head[24..26].copy_from_slice(&file.header.baseline.to_le_bytes());
    head[26..28].copy_from_slice(&file.header.key_every.to_le_bytes());
    head[28..32].copy_from_slice(&HEADER_SIZE.to_le_bytes());
    head[32..36]
        .copy_from_slice(&(HEADER_SIZE + file.header.frame_count * ENTRY_SIZE).to_le_bytes());
    head[36..40].copy_from_slice(&(file.payload.len() as u32).to_le_bytes());
    head[40..42].copy_from_slice(&file.header.origin_x.to_le_bytes());
    head[42..44].copy_from_slice(&file.header.origin_y.to_le_bytes());
    w.write_all(&head)?;

    for entry in &file.entries {
        let mut item = [0u8; ENTRY_SIZE as usize];
        item[0..4].copy_from_slice(&entry.offset.to_le_bytes());
        item[4..8].copy_from_slice(&entry.len.to_le_bytes());
        item[8] = entry.codec.as_u8();
        w.write_all(&item)?;
    }

    w.write_all(&file.payload)?;
    Ok(())
}

/// 读取帧头与帧表，并做一致性校验；不触碰帧负载。
pub fn read_header<R: Read>(r: &mut R) -> Result<(AnimHeader, Vec<FrameEntry>), AnimError> {
    let mut head = [0u8; HEADER_SIZE as usize];
    r.read_exact(&mut head)?;

    if &head[0..8] != MAGIC {
        return Err(AnimError::BadMagic);
    }
    let version = u16::from_le_bytes([head[8], head[9]]);
    if version != VERSION {
        return Err(AnimError::BadVersion(version));
    }

    let header = AnimHeader {
        flags: u16::from_le_bytes([head[10], head[11]]),
        width: u16::from_le_bytes([head[12], head[13]]),
        height: u16::from_le_bytes([head[14], head[15]]),
        frame_count: u32::from_le_bytes([head[16], head[17], head[18], head[19]]),
        fps: f32::from_bits(u32::from_le_bytes([head[20], head[21], head[22], head[23]])),
        baseline: u16::from_le_bytes([head[24], head[25]]),
        key_every: u16::from_le_bytes([head[26], head[27]]),
        origin_x: u16::from_le_bytes([head[40], head[41]]),
        origin_y: u16::from_le_bytes([head[42], head[43]]),
    };
    header.validate()?;

    if header.frame_count == 0 || header.frame_count > MAX_FRAMES {
        return Err(AnimError::Corrupt("帧数非法"));
    }

    let table_off = u32::from_le_bytes([head[28], head[29], head[30], head[31]]);
    let data_off = u32::from_le_bytes([head[32], head[33], head[34], head[35]]);
    let payload_len = u32::from_le_bytes([head[36], head[37], head[38], head[39]]);

    if table_off != HEADER_SIZE {
        return Err(AnimError::Corrupt("帧表偏移非法"));
    }
    if data_off != HEADER_SIZE + header.frame_count * ENTRY_SIZE {
        return Err(AnimError::Corrupt("负载偏移非法"));
    }

    let frame_bytes = header.byte_len() as u64;
    let mut entries = Vec::with_capacity(header.frame_count as usize);
    for _ in 0..header.frame_count {
        let mut item = [0u8; ENTRY_SIZE as usize];
        r.read_exact(&mut item)?;
        let entry = FrameEntry {
            offset: u32::from_le_bytes([item[0], item[1], item[2], item[3]]),
            len: u32::from_le_bytes([item[4], item[5], item[6], item[7]]),
            codec: Codec::from_u8(item[8])?,
        };
        // 负载必须落在数据区内，且长度不超过「原始大小 + zstd 最坏膨胀余量」。
        if entry.len == 0 || entry.offset < data_off {
            return Err(AnimError::Corrupt("帧偏移越界"));
        }
        if (entry.offset as u64 + entry.len as u64) > data_off as u64 + payload_len as u64 {
            return Err(AnimError::Corrupt("帧长度越界"));
        }
        if entry.len as u64 > frame_bytes + 65_536 {
            return Err(AnimError::Corrupt("帧负载超限"));
        }
        entries.push(entry);
    }

    Ok((header, entries))
}

/// 素材容器的全部错误。
#[derive(Debug, thiserror::Error)]
pub enum AnimError {
    #[error("读写出错: {0}")]
    Io(#[from] io::Error),
    #[error("魔数不匹配，不是 .fishanim 文件")]
    BadMagic,
    #[error("不支持的格式版本 {0}")]
    BadVersion(u16),
    #[error("未知的帧编码 {0}")]
    UnknownCodec(u8),
    #[error("文件损坏: {0}")]
    Corrupt(&'static str),
    #[error("第 {idx} 帧大小不符：期望 {expected} 字节，实际 {got}")]
    FrameSize {
        expected: usize,
        got: usize,
        idx: u32,
    },
    #[error("帧号 {idx} 越界（共 {count} 帧）")]
    FrameOutOfRange { idx: u32, count: u32 },
    #[error("解压失败: {0}")]
    Zstd(String),
    #[error("素材清单有问题: {0}")]
    Manifest(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(width: u16, height: u16, seed: u8) -> Vec<u8> {
        // 造一块有 alpha 渐变的像素，压缩后体积明显小于原始大小。
        let n = width as usize * height as usize;
        let mut v = Vec::with_capacity(n * 4);
        for i in 0..n {
            let a = (i % 251) as u8;
            v.extend_from_slice(&[seed, seed.wrapping_add(1), seed.wrapping_add(2), a]);
        }
        v
    }

    #[test]
    fn roundtrip_header_and_table() {
        let header = AnimHeader::new(320, 180, 24.0, 175).with_origin(12, 7);
        let frames = vec![frame(320, 180, 1), frame(320, 180, 2), frame(320, 180, 3)];
        let original = frames.clone();

        let file = encode(header, frames, 3).expect("encode");
        let mut buf = Vec::new();
        write(&mut buf, &file).expect("write");

        let (got, entries) = read_header(&mut Cursor::new(&buf)).expect("read_header");
        assert_eq!(got.frame_count, 3);
        assert_eq!(got.width, 320);
        assert_eq!(got.height, 180);
        assert_eq!(got.fps, 24.0);
        assert_eq!(got.baseline, 175);
        assert_eq!((got.origin_x, got.origin_y), (12, 7), "裁切原点必须能往返");
        assert!(got.is_premultiplied());
        assert_eq!(entries.len(), 3);

        // 偏移连续且落在负载区内。
        let mut expect = HEADER_SIZE + 3 * ENTRY_SIZE;
        for e in &entries {
            assert_eq!(e.offset, expect);
            expect += e.len;
        }
        assert_eq!(
            file.payload.len() as u32,
            expect - (HEADER_SIZE + 3 * ENTRY_SIZE)
        );
        assert_eq!(buf.len() as u32, expect);

        // 解压回来必须逐字节一致。
        for (e, want) in entries.iter().zip(original.iter()) {
            let start = e.offset as usize;
            let raw = &buf[start..start + e.len as usize];
            let out = match e.codec {
                Codec::Raw => raw.to_vec(),
                Codec::Zstd => zstd::bulk::decompress(raw, want.len()).expect("decompress"),
            };
            assert_eq!(&out, want);
        }
    }

    #[test]
    fn level_zero_stores_raw() {
        let file = encode(AnimHeader::new(8, 8, 30.0, 8), vec![frame(8, 8, 7)], 0).expect("encode");
        assert_eq!(file.entries[0].codec, Codec::Raw);
        assert_eq!(file.entries[0].len as usize, 8 * 8 * 4);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = Vec::new();
        write(
            &mut buf,
            &encode(AnimHeader::new(8, 8, 24.0, 8), vec![frame(8, 8, 0)], 0).unwrap(),
        )
        .unwrap();
        buf[0] = b'X';
        match read_header(&mut Cursor::new(&buf)) {
            Err(AnimError::BadMagic) => {}
            other => panic!("期望 BadMagic，实际 {other:?}"),
        }
    }

    #[test]
    fn rejects_frame_size_mismatch() {
        let bad = vec![0u8; 10];
        match encode(AnimHeader::new(8, 8, 24.0, 8), vec![bad], 0) {
            Err(AnimError::FrameSize {
                expected: 256,
                got: 10,
                idx: 0,
            }) => {}
            other => panic!("期望 FrameSize，实际 {other:?}"),
        }
    }

    #[test]
    fn rejects_zero_frames() {
        let empty: Vec<Vec<u8>> = Vec::new();
        match encode(AnimHeader::new(8, 8, 24.0, 8), empty, 0) {
            Err(AnimError::Corrupt(msg)) => assert_eq!(msg, "帧数为 0"),
            other => panic!("期望 Corrupt，实际 {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_fps() {
        match encode(AnimHeader::new(8, 8, f32::NAN, 8), vec![frame(8, 8, 0)], 0) {
            Err(AnimError::Corrupt(_)) => {}
            other => panic!("期望 Corrupt，实际 {other:?}"),
        }
    }
}
