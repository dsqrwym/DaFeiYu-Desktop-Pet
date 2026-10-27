//! 黑底抠像：只把「连通到画面边缘的暗像素」判为背景。
//!
//! 上游分发的 webm 是 640×360 `yuv420p`——**没有 alpha 通道**，角色被合成在纯黑上
//! （实测角点 #000000，整帧 82.5% 为暗像素）。这带来一个关键性质：
//!
//! > 黑底合成 = `rgb = fg × a + 0`，即画面本身已经是**预乘 alpha** 的形式。
//!
//! 所以不需要反预乘、也不需要还原色彩：颜色原样保留，只补一个 alpha 通道。
//! 唯一要算对的是 alpha，而它**不能**用全局亮度阈值——角色内部有大量深色像素
//! （深蓝头发、黑鞋、裙摆阴影），全局阈值会在角色身上打洞。正确做法是**边界泛洪**：
//! 只清除与画面边框连通的暗区，被角色围住的暗像素留在体内。

/// 单像素亮度取三通道最大值（比加权亮度更保守：不会把饱和色误判成暗）。
#[inline]
fn lum(r: u8, g: u8, b: u8) -> u8 {
    r.max(g).max(b)
}

/// 生成 alpha 通道（长度 = `width × height`）。
///
/// * `thresh` —— 亮度不超过它才算「可能的背景」；
/// * `feather` —— 0 表示硬边；大于 0 时，紧邻背景且亮度在 `thresh..thresh+feather`
///   的像素按亮度给出半透明，用来柔化锯齿。
pub fn build_alpha(rgba: &[u8], width: usize, height: usize, thresh: u8, feather: u8) -> Vec<u8> {
    let n = width * height;
    debug_assert_eq!(rgba.len(), n * 4);
    let mut bg = vec![false; n];
    let mut stack: Vec<u32> = Vec::with_capacity(1024);

    let is_dark = |idx: usize| {
        let o = idx * 4;
        lum(rgba[o], rgba[o + 1], rgba[o + 2]) <= thresh
    };

    // 从四条边播种：只有连通到边框的暗区才是背景。
    for x in 0..width {
        for y in [0, height - 1] {
            let idx = y * width + x;
            if !bg[idx] && is_dark(idx) {
                bg[idx] = true;
                stack.push(idx as u32);
            }
        }
    }
    for y in 0..height {
        for x in [0, width - 1] {
            let idx = y * width + x;
            if !bg[idx] && is_dark(idx) {
                bg[idx] = true;
                stack.push(idx as u32);
            }
        }
    }

    // 四邻域泛洪。
    while let Some(idx) = stack.pop() {
        let idx = idx as usize;
        let x = idx % width;
        let y = idx / width;
        let push = |nx: usize, ny: usize, bg: &mut Vec<bool>, stack: &mut Vec<u32>| {
            let ni = ny * width + nx;
            if !bg[ni] {
                let o = ni * 4;
                if lum(rgba[o], rgba[o + 1], rgba[o + 2]) <= thresh {
                    bg[ni] = true;
                    stack.push(ni as u32);
                }
            }
        };
        if x > 0 {
            push(x - 1, y, &mut bg, &mut stack);
        }
        if x + 1 < width {
            push(x + 1, y, &mut bg, &mut stack);
        }
        if y > 0 {
            push(x, y - 1, &mut bg, &mut stack);
        }
        if y + 1 < height {
            push(x, y + 1, &mut bg, &mut stack);
        }
    }

    let mut alpha = vec![0u8; n];
    for idx in 0..n {
        if bg[idx] {
            continue;
        }
        let o = idx * 4;
        let l = lum(rgba[o], rgba[o + 1], rgba[o + 2]);
        alpha[idx] = if feather > 0 && l < thresh.saturating_add(feather) {
            // 紧贴背景的暗边：按亮度给 1..255 的过渡。
            let span = feather as u32;
            (((l.saturating_sub(thresh)) as u32 * 255) / span).clamp(1, 255) as u8
        } else {
            255
        };
    }
    alpha
}

/// 砍掉色彩通道的低 `bits` 位，alpha 原样保留。
///
/// 上游 webm 是损失压缩产物，帧间充满低位抖动；这些位对肉眼几乎无影响，
/// 却把熵值拉满。只保留高位能让 zstd 的匹配长度大幅变长。
pub fn quantize(rgba: &mut [u8], bits: u8) {
    if bits == 0 {
        return;
    }
    let keep = 0xFFu8 << bits.min(7);
    for px in rgba.chunks_exact_mut(4) {
        px[0] &= keep;
        px[1] &= keep;
        px[2] &= keep;
    }
}

/// 找整帧最下方的不透明行（返回 `y + 1`），用作脚底线。
pub fn last_opaque_row(alpha: &[u8], width: usize, height: usize) -> Option<usize> {
    for y in (0..height).rev() {
        let row = &alpha[y * width..(y + 1) * width];
        if row.iter().any(|&a| a > 0) {
            return Some(y + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张 `w×h` 的纯色图。
    fn solid(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for px in v.chunks_exact_mut(4) {
            px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        v
    }

    #[test]
    fn all_black_becomes_transparent() {
        let (w, h) = (5, 4);
        let a = build_alpha(&solid(w, h, [0, 0, 0]), w, h, 12, 0);
        assert!(a.iter().all(|&v| v == 0));
    }

    #[test]
    fn enclosed_dark_pixels_stay_opaque() {
        // 7×7：外圈黑、次外圈亮、中心一个黑点——中心被亮环围住，必须保留。
        let (w, h) = (7, 7);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 1..6 {
            for x in 1..6 {
                let o = (y * w + x) * 4;
                img[o..o + 3].copy_from_slice(&[200, 180, 160]);
            }
        }
        let o = (3 * w + 3) * 4;
        img[o..o + 3].copy_from_slice(&[0, 0, 0]);

        let a = build_alpha(&img, w, h, 12, 0);
        assert_eq!(a[3 * w + 3], 255, "被包围的暗像素不能被抠掉");
        assert_eq!(a[0], 0, "边框暗像素应透明");
        assert_eq!(a[6], 0, "边框暗像素应透明");
    }

    #[test]
    fn bright_border_is_kept() {
        let (w, h) = (4, 4);
        let a = build_alpha(&solid(w, h, [120, 120, 120]), w, h, 12, 0);
        assert!(a.iter().all(|&v| v == 255));
    }

    #[test]
    fn feather_gives_soft_edge() {
        // 5×5：左边三列黑（连通边框）、右边两列暗灰（亮度 20，thresh 12 + feather 16）。
        let (w, h) = (5, 5);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 0..h {
            for x in 3..5 {
                let o = (y * w + x) * 4;
                img[o..o + 3].copy_from_slice(&[20, 20, 20]);
            }
        }
        let a = build_alpha(&img, w, h, 12, 16);
        assert_eq!(a[0], 0);
        assert!(a[3] > 0 && a[3] < 255, "过渡带应为半透明，实际 {}", a[3]);
    }

    #[test]
    fn quantize_keeps_alpha_and_high_bits() {
        let mut px = vec![0xFFu8, 0x7F, 0x81, 0x30];
        quantize(&mut px, 3);
        assert_eq!(px, vec![0xF8, 0x78, 0x80, 0x30], "高位保留、alpha 不动");
        quantize(&mut px, 0);
        assert_eq!(px[0], 0xF8, "bits=0 必须不改变任何像素");
    }

    #[test]
    fn baseline_is_bottom_of_content() {
        let (w, h) = (3, 5);
        let mut a = vec![0u8; w * h];
        a[2 * w] = 255; // 第 2 行有内容
        assert_eq!(last_opaque_row(&a, w, h), Some(3));
        assert_eq!(last_opaque_row(&vec![0u8; w * h], w, h), None);
    }
}
