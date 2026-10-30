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
//!
//! ## alpha 只有 0/255 时的两个毛病
//!
//! 泛洪只回答「是不是背景」，不回答「有多不透明」。抗锯齿边缘与半透明特效
//! （气泡、烟雾、火星）的像素都是 `C = F × a` 且 `a < 1`，直接给 255 就会出现：
//!
//! * 角色轮廓一圈**黑边**（边缘的 `C` 只有内色的五到八成）；
//! * 气泡中心、烟花烟雾这类**半透明区域整块发黑**。
//!
//! 现在按「离背景多远、属于哪个连通块」分三段处理：
//!
//! 1. **核心**（离背景 ≥ `rim` 像素）：`a = 255`，颜色即原值；
//! 2. **过渡带**（核心外不满 `rim` 像素）：用最近的核心色 `F` 反推
//!    `a = lum(C) / lum(F)`，颜色换成直通色（H.264 编码要用直通色，否则 4:2:0
//!    采样会把黑边渗进角色）；
//! 3. **小连通块**（面积 `effect_min..=主块×effect_rel`）：按该块自己的 90 分位
//!    亮度归一化，气泡/火星于是变成半透明而不是黑圆盘。
//!
//! 主连通块里大面积的半透明烟雾仍然无法与「本来就画得深的衣服」区分，
//! 这是黑底合成本身的歧义，不做自动猜测。

/// 单像素亮度取三通道最大值（比加权亮度更保守：不会把饱和色误判成暗）。
#[inline]
fn lum(r: u8, g: u8, b: u8) -> u8 {
    r.max(g).max(b)
}

/// 抠像参数。
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// 亮度不超过它才算「可能的背景」。
    pub thresh: u8,
    /// 过渡带宽度（像素）。0 = 不做过渡（旧行为，边缘仍是硬边）。
    pub rim: u8,
    /// 小连通块按自身亮度归一化的最小面积。
    pub effect_min: usize,
    /// 小连通块相对主连通块的最大面积比例。
    pub effect_rel: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            thresh: 12,
            rim: 2,
            effect_min: 64,
            effect_rel: 0.25,
        }
    }
}

/// 一帧的抠像结果。
pub struct Extraction {
    /// 逐像素 alpha（`width × height`）。
    pub alpha: Vec<u8>,
    /// 逐像素**直通**颜色（`width × height × 4`，RGBA）。H.264 编码用它：
    /// 半透明像素的颜色已经按 `a` 反预乘回来了，配上 1-bit 掩码就不会发黑。
    pub straight: Vec<u8>,
}

struct Comp {
    area: usize,
    /// 亮度直方图（0..=255），用来算 90 分位。
    hist: [u32; 256],
    pixels: Vec<u32>,
}

impl Comp {
    fn p90(&self) -> u8 {
        let want = (self.area as f32 * 0.9).ceil() as u32;
        let mut cum = 0u32;
        for (v, n) in self.hist.iter().enumerate() {
            cum += *n;
            if cum >= want {
                return v as u8;
            }
        }
        255
    }
}

/// 生成 alpha 与直通颜色。
pub fn extract(rgba: &[u8], width: usize, height: usize, p: &Params) -> Extraction {
    let n = width * height;
    debug_assert_eq!(rgba.len(), n * 4);

    let mut alpha = vec![0u8; n];
    let mut straight = vec![0u8; n * 4];
    if n == 0 {
        return Extraction { alpha, straight };
    }

    let bg = background_mask(rgba, width, height, p.thresh);
    let fg: Vec<bool> = bg.iter().map(|b| !b).collect();
    let (_, comps) = label_components(rgba, &fg, width, height);
    let (main_id, main_area) = comps
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|(_, c)| c.area)
        .map(|(i, c)| (i as u32, c.area))
        .unwrap_or((0, 0));

    // 1) 打底：主块不透明；面积小、又没多少亮部的小块按自身亮度归一化
    //    （气泡、火星这类半透明特效）。
    let effect_max = (main_area as f32 * p.effect_rel) as usize;
    for (id, comp) in comps.iter().enumerate().skip(1) {
        let effect = (id as u32) != main_id
            && comp.area >= p.effect_min.max(1)
            && comp.area <= effect_max.max(p.effect_min);
        let scale = comp.p90().max(16) as u32;
        for &px in &comp.pixels {
            let i = px as usize;
            let o = i * 4;
            let (r, g, b) = (rgba[o], rgba[o + 1], rgba[o + 2]);
            let a = if effect {
                (((lum(r, g, b) as u32) * 255) / scale).clamp(1, 255) as u8
            } else {
                255
            };
            alpha[i] = a;
            write_straight(&mut straight, o, r, g, b, a);
        }
    }

    // 2) 过渡带：从背景往内推 `rim` 圈，用**里侧最亮的邻居**当 F 反推 alpha。
    //
    // 为什么不拿“整个连通块的核心色”：辫子、草叶、深色描边这些一两个像素宽的
    // 结构整体都在过渡带里，拿远处的亮色反推会把它们淡成半透明。
    for _ in 0..p.rim {
        let snapshot = alpha.clone();
        for i in 0..n {
            if snapshot[i] == 0 {
                continue;
            }
            let x = i % width;
            let y = i / width;
            let touches_bg =
                neighbours(x, y, width, height).any(|(nx, ny)| snapshot[ny * width + nx] == 0);
            if !touches_bg {
                continue;
            }
            // 里侧最亮的邻居：它的直通色就是这一像素本该有的颜色。
            let mut best: Option<usize> = None;
            let mut best_lum = 0u8;
            for (nx, ny) in neighbours(x, y, width, height) {
                let j = ny * width + nx;
                if snapshot[j] == 0 {
                    continue;
                }
                let o = j * 4;
                let l = straight[o].max(straight[o + 1]).max(straight[o + 2]);
                if best.is_none() || l > best_lum {
                    best = Some(o);
                    best_lum = l;
                }
            }
            // 一像素宽的细结构：里侧没有邻居（比如草叶），保持不透明。
            if best.is_none() {
                continue;
            }
            let o = i * 4;
            let (r, g, b) = (rgba[o], rgba[o + 1], rgba[o + 2]);
            let lf = best_lum.max(8) as u32;
            let a = (((lum(r, g, b) as u32) * 255) / lf).clamp(1, 255) as u8;
            if a < alpha[i] {
                alpha[i] = a;
                write_straight(&mut straight, o, r, g, b, a);
            }
        }
    }

    // 3) 背景颜色向外扩一像素：H.264 的色度往边上渗，填成纯黑就是一圈黑晕。
    //    只扩一像素——色度是 2×2 采样，再往外扩只是白花码率。
    let mut filled: Vec<bool> = alpha.iter().map(|a| *a != 0).collect();
    for _ in 0..1 {
        let snapshot = filled.clone();
        for i in 0..n {
            if snapshot[i] {
                continue;
            }
            let x = i % width;
            let y = i / width;
            if let Some((nx, ny)) =
                neighbours(x, y, width, height).find(|(nx, ny)| snapshot[ny * width + nx])
            {
                let from = (ny * width + nx) * 4;
                let o = i * 4;
                straight[o] = straight[from];
                straight[o + 1] = straight[from + 1];
                straight[o + 2] = straight[from + 2];
                straight[o + 3] = 255;
                filled[i] = true;
            }
        }
    }

    Extraction { alpha, straight }
}

/// 从四条边泛洪：只有连通到边框的暗区才是背景。
fn background_mask(rgba: &[u8], width: usize, height: usize, thresh: u8) -> Vec<bool> {
    let n = width * height;
    let mut bg = vec![false; n];
    let mut stack: Vec<u32> = Vec::with_capacity(1024);
    let dark = |idx: usize| {
        let o = idx * 4;
        lum(rgba[o], rgba[o + 1], rgba[o + 2]) <= thresh
    };
    let seed = |idx: usize, bg: &mut Vec<bool>, stack: &mut Vec<u32>| {
        if !bg[idx] && dark(idx) {
            bg[idx] = true;
            stack.push(idx as u32);
        }
    };
    for x in 0..width {
        seed(x, &mut bg, &mut stack);
        seed((height - 1) * width + x, &mut bg, &mut stack);
    }
    for y in 0..height {
        seed(y * width, &mut bg, &mut stack);
        seed(y * width + width - 1, &mut bg, &mut stack);
    }
    while let Some(idx) = stack.pop() {
        let i = idx as usize;
        let x = i % width;
        let y = i / width;
        for (nx, ny) in neighbours(x, y, width, height) {
            let j = ny * width + nx;
            if !bg[j] {
                let o = j * 4;
                if lum(rgba[o], rgba[o + 1], rgba[o + 2]) <= thresh {
                    bg[j] = true;
                    stack.push(j as u32);
                }
            }
        }
    }
    bg
}

/// 四邻域 BFS 标连通块；`labels[0]` 是占位，`comps[0]` 不用。
fn label_components(
    rgba: &[u8],
    fg: &[bool],
    width: usize,
    height: usize,
) -> (Vec<u32>, Vec<Comp>) {
    let n = width * height;
    let mut labels = vec![0u32; n];
    let mut comps: Vec<Comp> = vec![Comp {
        area: 0,
        hist: [0; 256],
        pixels: Vec::new(),
    }];
    let mut stack: Vec<u32> = Vec::with_capacity(1024);
    for start in 0..n {
        if !fg[start] || labels[start] != 0 {
            continue;
        }
        let id = comps.len() as u32;
        let mut comp = Comp {
            area: 0,
            hist: [0; 256],
            pixels: Vec::new(),
        };
        labels[start] = id;
        stack.clear();
        stack.push(start as u32);
        while let Some(px) = stack.pop() {
            let i = px as usize;
            comp.area += 1;
            comp.pixels.push(px);
            let o = i * 4;
            comp.hist[lum(rgba[o], rgba[o + 1], rgba[o + 2]) as usize] += 1;
            let x = i % width;
            let y = i / width;
            for (nx, ny) in neighbours(x, y, width, height) {
                let j = ny * width + nx;
                if fg[j] && labels[j] == 0 {
                    labels[j] = id;
                    stack.push(j as u32);
                }
            }
        }
        comps.push(comp);
    }
    (labels, comps)
}

/// 四邻域邻居（越界的不返回）。
#[inline]
fn neighbours(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let mut out = [(0usize, 0usize); 4];
    let mut len = 0;
    if x > 0 {
        out[len] = (x - 1, y);
        len += 1;
    }
    if x + 1 < width {
        out[len] = (x + 1, y);
        len += 1;
    }
    if y > 0 {
        out[len] = (x, y - 1);
        len += 1;
    }
    if y + 1 < height {
        out[len] = (x, y + 1);
        len += 1;
    }
    out.into_iter().take(len)
}

/// 预乘色 → 直通色：`F = C × 255 / a`。
#[inline]
fn write_straight(out: &mut [u8], o: usize, r: u8, g: u8, b: u8, a: u8) {
    if a == 255 {
        out[o] = r;
        out[o + 1] = g;
        out[o + 2] = b;
    } else {
        let a = a.max(1) as u32;
        let f = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
        out[o] = f(r);
        out[o + 1] = f(g);
        out[o + 2] = f(b);
    }
    out[o + 3] = 255;
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

    fn alpha_of(rgba: &[u8], w: usize, h: usize, p: Params) -> Vec<u8> {
        extract(rgba, w, h, &p).alpha
    }

    #[test]
    fn all_black_becomes_transparent() {
        let (w, h) = (5, 4);
        let a = alpha_of(&solid(w, h, [0, 0, 0]), w, h, Params::default());
        assert!(a.iter().all(|&v| v == 0));
    }

    #[test]
    fn enclosed_dark_pixels_stay_opaque() {
        // 9×9：外圈黑、次圈亮、中心一个黑点——中心被亮环围住，必须保留。
        let (w, h) = (9, 9);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 1..8 {
            for x in 1..8 {
                let o = (y * w + x) * 4;
                img[o..o + 3].copy_from_slice(&[200, 180, 160]);
            }
        }
        let o = (4 * w + 4) * 4;
        img[o..o + 3].copy_from_slice(&[0, 0, 0]);

        let a = alpha_of(&img, w, h, Params::default());
        assert_eq!(a[4 * w + 4], 255, "被包围的暗像素不能被抠掉");
        assert_eq!(a[0], 0, "边框暗像素应透明");
        assert_eq!(a[8], 0, "边框暗像素应透明");
    }

    #[test]
    fn bright_border_is_kept() {
        let (w, h) = (4, 4);
        let a = alpha_of(&solid(w, h, [120, 120, 120]), w, h, Params::default());
        assert!(a.iter().all(|&v| v == 255));
    }

    #[test]
    fn rim_band_turns_dark_edge_translucent() {
        // 11×11：外边黑、最外一圈亮块是「被压暗的边」（C=80），内部 C=255。
        // 过渡带像素应半透明，内部必须是 255。
        let (w, h) = (11, 11);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 3..8 {
            for x in 3..8 {
                let o = (y * w + x) * 4;
                img[o..o + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        for y in 2..9 {
            for x in 2..9 {
                let o = (y * w + x) * 4;
                if img[o] == 0 {
                    img[o..o + 3].copy_from_slice(&[80, 80, 80]);
                }
            }
        }
        let a = alpha_of(&img, w, h, Params::default());
        assert_eq!(a[5 * w + 5], 255, "核心必须不透明");
        let edge = a[2 * w + 5];
        assert!(edge > 0 && edge < 255, "过渡带应半透明，实际 {edge}");
    }

    #[test]
    fn dark_interior_past_rim_stays_opaque() {
        // 亮底上一块足够厚的深色「衣服」：只有最外一圈受影响，内部仍是 255。
        let (w, h) = (15, 15);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 1..14 {
            for x in 1..14 {
                let o = (y * w + x) * 4;
                let dark = (5..10).contains(&x) && (5..10).contains(&y);
                let c = if dark { 40 } else { 220 };
                img[o..o + 3].copy_from_slice(&[c, c, c]);
            }
        }
        let a = alpha_of(&img, w, h, Params::default());
        assert_eq!(a[7 * w + 7], 255, "离背景够远的深色衣服不能被抠成半透明");
    }

    #[test]
    fn small_soft_blob_is_normalized_by_its_own_peak() {
        // 主体（大块亮） + 一块 10×8 的小块：边缘亮、中心暗，模拟气泡。
        let (w, h) = (40, 20);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 1..19 {
            for x in 1..25 {
                let o = (y * w + x) * 4;
                img[o..o + 3].copy_from_slice(&[200, 200, 200]);
            }
        }
        for y in 6..14 {
            for x in 28..38 {
                let o = (y * w + x) * 4;
                let rim = y == 6 || y == 13 || x == 28 || x == 37;
                let c = if rim { 180 } else { 20 };
                img[o..o + 3].copy_from_slice(&[c, c, c]);
            }
        }
        let a = alpha_of(&img, w, h, Params::default());
        let centre = a[9 * w + 32];
        assert!(
            centre > 0 && centre < 128,
            "气泡中心应半透明，实际 {centre}"
        );
        assert!(
            a[9 * w + 28] >= 200,
            "气泡亮边应基本不透明，实际 {}",
            a[9 * w + 28]
        );
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
