//! Pixel helpers: mirror, seam distance, two-frame blend.

/// 把一帧预乘 BGRA 水平镜像（行内倒过来）写进 `dst`，返回它。
///
/// 素材只画了一套朝向，往反方向走就靠这个：眼睛、头发、衣摆全都跟着翻，
/// 比「倒着走」自然得多。
pub(super) fn mirror_bgra<'a>(src: &[u8], width: usize, dst: &'a mut Vec<u8>) -> &'a [u8] {
    dst.clear();
    dst.reserve(src.len());
    let row = width.max(1) * 4;
    for line in src.chunks_exact(row) {
        for px in line.rchunks_exact(4) {
            dst.extend_from_slice(px);
        }
    }
    dst
}

/// 两帧的平均像素差。用来判断一个循环接缝硬不硬。
///
/// 每 16 个字节采一个样（一行 384 像素 = 1536 字节，即一行采 96 点），
/// 二十四万多个采样点，四百 KB 的帧算一遍不到 0.1ms——只在接缝处跑一次。
pub(super) fn seam_mad(a: &[u8], b: &[u8]) -> f32 {
    let (mut sum, mut n) = (0u64, 0u64);
    for (x, y) in a.iter().zip(b).step_by(16) {
        sum += u64::from((*x).abs_diff(*y));
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum as f32 / n as f32
    }
}

/// 把 `from` 和 `to` 按 `num/den` 线性混合，写进 `out`。
///
/// 帧像素是**预乘** BGRA，逐通道线性插值就是正确的那种混合（预乘之后
/// 透明度已经乘进颜色里，插值不会出现半透明边缘发黑）。
pub(super) fn blend_frame(from: &[u8], to: &[u8], num: u32, den: u32, out: &mut Vec<u8>) {
    let den = den.max(1);
    let num = num.min(den);
    out.clear();
    out.extend(to.iter().enumerate().map(|(i, b)| {
        // 长度不一致时（换了素材还没重画）就以新帧为准，不越界。
        let a = u32::from(from.get(i).copied().unwrap_or(*b));
        let b = u32::from(*b);
        ((a * (den - num) + b * num) / den) as u8
    }));
}

/// 保证区间左小右大；退化（窗口比工作区还大）时压成中点。
pub(super) fn ordered(lo: f32, hi: f32) -> (f32, f32) {
    if lo <= hi {
        (lo, hi)
    } else {
        let mid = (lo + hi) / 2.0;
        (mid, mid)
    }
}
