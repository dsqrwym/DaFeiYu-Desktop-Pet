//! 预览 GIF：给黑底素材补一份「直通色」。
//!
//! 上游 webm 是黑底合成（`C = F × a`），单帧无法把颜色和 alpha 拆开。
//! 同一个上游仓库还带 `assets/preview/*.gif`——220×124、12fps、**带 1-bit 透明**，
//! 画的是同一个角色同一段时间，只是没有黑底。于是：
//!
//! ```text
//! a = |C| / |F|      C 来自 webm（黑底预乘），F 来自 GIF（直通色）
//! ```
//!
//! 两个已知代价：
//! * GIF 只有 220×124，颜色经过 256 色调色板量化——只让它管「低频」，
//!   轮廓那几像素仍由 `mask::extract` 全分辨率算；
//! * GIF 是 12fps、webm 是 24fps，按帧号线性映射（241 帧 ↔ 120 帧），
//!   漂移一两帧对低频 alpha 没影响。
//!
//! 文件名是拼音（`fang-yanhua.gif`），和我们的 slug 对不上，所以配对靠
//! **首帧图像匹配**：把 webm 首帧缩到 220×124，与每个 GIF 首帧（合成到黑底）
//! 比 MSE，取误差最小且明显领先的那一个。

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

use anyhow::{ensure, Context, Result};

const SMALL_W: usize = 220;
const SMALL_H: usize = 124;

/// 一个候选预览：文件名 + 首帧（220×124 RGB，已合成到黑底）。
pub struct Candidate {
    pub file: PathBuf,
    small: Vec<u8>,
}

/// 扫一遍预览目录，把每个 GIF 的首帧读出来备用。
pub fn scan(ffmpeg: &str, dir: &Path) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("读不了预览目录 {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("gif") {
            continue;
        }
        let frame = first_frame_premultiplied(ffmpeg, &path)?;
        out.push(Candidate {
            file: path,
            small: frame,
        });
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(out)
}

fn first_frame_premultiplied(ffmpeg: &str, gif: &Path) -> Result<Vec<u8>> {
    let filter = format!(
        "format=rgba,premultiply=inplace=1,scale={SMALL_W}:{SMALL_H}:flags=area,format=rgba"
    );
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(gif)
        .args(["-vf", &filter, "-frames:v", "1", "-f", "rawvideo", "-"])
        .output()
        .with_context(|| format!("无法启动 ffmpeg（{ffmpeg}）"))?;
    ensure!(
        out.status.success(),
        "解 {} 首帧失败：{}",
        gif.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let want = SMALL_W * SMALL_H * 4;
    ensure!(
        out.stdout.len() >= want,
        "{} 首帧字节数不对：{} < {want}",
        gif.display(),
        out.stdout.len()
    );
    Ok(out.stdout[..want].to_vec())
}

/// 把整帧盒式缩小到 220×124（alpha 一律按 1，webm 帧本来就是预乘色）。
pub fn shrink(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; SMALL_W * SMALL_H * 3];
    for sy in 0..SMALL_H {
        let y0 = sy * height / SMALL_H;
        let y1 = ((sy + 1) * height / SMALL_H).max(y0 + 1).min(height);
        for sx in 0..SMALL_W {
            let x0 = sx * width / SMALL_W;
            let x1 = ((sx + 1) * width / SMALL_W).max(x0 + 1).min(width);
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let o = (y * width + x) * 4;
                    for c in 0..3 {
                        sum[c] += u32::from(rgba[o + c]);
                    }
                    n += 1;
                }
            }
            let o = (sy * SMALL_W + sx) * 3;
            for c in 0..3 {
                out[o + c] = (sum[c] / n.max(1)) as u8;
            }
        }
    }
    out
}

/// 首帧匹配：返回 `(候选下标, 每像素均方误差)`。
pub fn best_match(candidates: &[Candidate], small: &[u8]) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for (i, c) in candidates.iter().enumerate() {
        let mut err = 0.0f64;
        for (a, b) in c.small.chunks_exact(4).zip(small.chunks_exact(3)) {
            // 预览是预乘值：直接和 webm 的黑底值比。
            for ch in 0..3 {
                let d = f64::from(a[ch]) - f64::from(b[ch]);
                err += d * d;
            }
        }
        let mse = err / (SMALL_W * SMALL_H) as f64;
        if best.is_none_or(|(_, m)| mse < m) {
            best = Some((i, mse));
        }
    }
    best
}

/// 已配对的预览流：按目标尺寸逐帧给「预乘 RGB + 软 alpha」。
pub struct Stream {
    child: Child,
    stdout: BufReader<ChildStdout>,
    frame_len: usize,
    /// 已经流出多少帧（下一次读取对应这个下标）。
    cursor: usize,
    last: Vec<u8>,
}

impl Stream {
    pub fn spawn(ffmpeg: &str, gif: &Path, width: usize, height: usize) -> Result<Self> {
        let filter = format!(
            "format=rgba,premultiply=inplace=1,scale={width}:{height}:flags=lanczos,format=rgba"
        );
        let mut child = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
            .arg(gif)
            .args(["-vf", &filter, "-f", "rawvideo", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("无法启动 ffmpeg（{ffmpeg}）"))?;
        let stdout = child.stdout.take().context("预览流没有 stdout 管道")?;
        Ok(Self {
            child,
            stdout: BufReader::with_capacity(1 << 20, stdout),
            frame_len: width * height * 4,
            cursor: 0,
            last: Vec::new(),
        })
    }

    /// 取第 `target` 帧（预乘 RGBA）。流放完了就沿用最后一帧。
    pub fn frame_at(&mut self, target: usize, buf: &mut Vec<u8>) -> Result<()> {
        while self.cursor <= target {
            let mut frame = vec![0u8; self.frame_len];
            let mut filled = 0;
            while filled < self.frame_len {
                let n = self.stdout.read(&mut frame[filled..])?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled < self.frame_len {
                // 流结束：用最后一帧顶上（帧数比 webm 少一帧是常态）。
                if self.last.is_empty() {
                    return Ok(());
                }
                *buf = self.last.clone();
                return Ok(());
            }
            self.cursor += 1;
            self.last = frame;
        }
        if !self.last.is_empty() {
            *buf = self.last.clone();
        }
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 双源解混：用预览的直通色把「内部」的 alpha 从黑底颜色里解出来。
///
/// * `edge` 为真的像素（轮廓过渡带）保持 `mask::extract` 的全分辨率结果；
/// * 其余像素只在预览说「这里有不透明内容」时才动；
/// * 解出来的 alpha 低于 `SOLID` 连成一片时才算半透明（面积 ≥ `min_area`），
///   零散像素是调色板量化噪声，保持不透明。
pub struct Solve {
    /// 低于这个 alpha 的像素才考虑改成半透明。
    pub solid: f32,
    /// 半透明连通块的最小面积（像素）。
    pub min_area: usize,
}

impl Default for Solve {
    fn default() -> Self {
        Self {
            solid: 0.88,
            min_area: 40,
        }
    }
}

/// 就地修改 alpha 与直通色，返回被改成半透明的像素数。
///
/// 参数多是因为它就是一个像素变换：输入两张图 + 输出两份张量 + 尺寸与参数。
#[allow(clippy::too_many_arguments)]
pub fn solve(
    preview: &[u8],
    color: &[u8],
    alpha: &mut [u8],
    straight: &mut [u8],
    edge: &[bool],
    width: usize,
    height: usize,
    opts: &Solve,
) -> usize {
    let n = width * height;
    // 预览只有 1-bit alpha：半透明的东西（烟雾、火花尾巴）在 GIF 里干脆被抠掉了。
    // 对这些像素，“哪里有内容”由 webm 告诉我们，“内容长什么样”只能借用最近的
    // 预览不透明色——否则它们永远只能是一块黑。
    let fill = nearest_opaque(preview, width, height);
    let mut cand = vec![false; n];
    let mut value = vec![1.0f32; n];
    for i in 0..n {
        if alpha[i] == 0 {
            continue;
        }
        let o = i * 4;
        let pa = preview[o + 3];
        let f = if pa >= 64 {
            let a = f32::from(pa) / 255.0;
            [
                f32::from(preview[o]) / a,
                f32::from(preview[o + 1]) / a,
                f32::from(preview[o + 2]) / a,
            ]
        } else {
            match fill[i] {
                Some(c) => [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])],
                None => continue,
            }
        };
        let fm = f[0].max(f[1]).max(f[2]);
        if fm < 8.0 {
            continue;
        }
        let cm = f32::from(color[o].max(color[o + 1]).max(color[o + 2]));
        let ratio = (cm / fm).clamp(0.0, 1.0);
        if ratio < opts.solid {
            cand[i] = true;
            value[i] = ratio;
        }
    }

    // 面积过滤：调色板量化会在边界上留零散的低比值像素，不能当半透明。
    // 轮廓过渡带例外：那里的低比值就是抗锯齿本身，而且弧线/烟雾边缘很细，
    // 一阵面积过滤会把它们全滤掉，留下一圈黑点。
    let mut keep = vec![false; n];
    for i in 0..n {
        if cand[i] && edge[i] {
            keep[i] = true;
        }
    }
    let mut seen = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..n {
        if !cand[start] || seen[start] {
            continue;
        }
        stack.clear();
        stack.push(start);
        seen[start] = true;
        let mut members: Vec<usize> = Vec::new();
        while let Some(i) = stack.pop() {
            members.push(i);
            let x = i % width;
            let y = i / width;
            let visit = |nx: usize, ny: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
                let j = ny * width + nx;
                if cand[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(x - 1, y, &mut seen, &mut stack);
            }
            if x + 1 < width {
                visit(x + 1, y, &mut seen, &mut stack);
            }
            if y > 0 {
                visit(x, y - 1, &mut seen, &mut stack);
            }
            if y + 1 < height {
                visit(x, y + 1, &mut seen, &mut stack);
            }
        }
        if members.len() >= opts.min_area {
            for i in members {
                keep[i] = true;
            }
        }
    }

    let mut changed = 0;
    for i in 0..n {
        if !keep[i] {
            continue;
        }
        let o = i * 4;
        let a = ((value[i] * 255.0).round() as u32).clamp(1, 254) as u8;
        alpha[i] = a;
        // 直通色只给「H.264 里还看得见」的像素（alpha ≥ 128）。
        // 被掩码遮掉的像素保持原值：把它们的颜色也提亮，编码器会白花一大截码率。
        if a >= 128 {
            let inv = 255.0 / f32::from(a);
            for c in 0..3 {
                straight[o + c] = (f32::from(color[o + c]) * inv).min(255.0).round() as u8;
            }
        }
        changed += 1;
    }
    changed
}

/// 每个像素最近的「预览不透明」颜色。整帧都没有不透明像素时返回全 `None`。
fn nearest_opaque(preview: &[u8], width: usize, height: usize) -> Vec<Option<[u8; 3]>> {
    let n = width * height;
    let mut out = vec![None; n];
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for i in 0..n {
        if preview[i * 4 + 3] >= 64 {
            let o = i * 4;
            out[i] = Some([preview[o], preview[o + 1], preview[o + 2]]);
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let Some(c) = out[i] else { continue };
        let x = i % width;
        let y = i / width;
        let push = |nx: usize,
                    ny: usize,
                    out: &mut Vec<Option<[u8; 3]>>,
                    q: &mut std::collections::VecDeque<usize>| {
            let j = ny * width + nx;
            if out[j].is_none() {
                out[j] = Some(c);
                q.push_back(j);
            }
        };
        if x > 0 {
            push(x - 1, y, &mut out, &mut queue);
        }
        if x + 1 < width {
            push(x + 1, y, &mut out, &mut queue);
        }
        if y > 0 {
            push(x, y - 1, &mut out, &mut queue);
        }
        if y + 1 < height {
            push(x, y + 1, &mut out, &mut queue);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预乘色 `C = F × a`，输入直通色与 alpha。
    fn premultiplied(f: [u8; 3], a: u8) -> [u8; 4] {
        let k = u16::from(a);
        [
            ((u16::from(f[0]) * k) / 255) as u8,
            ((u16::from(f[1]) * k) / 255) as u8,
            ((u16::from(f[2]) * k) / 255) as u8,
            a,
        ]
    }

    #[test]
    fn solves_half_transparent_region() {
        let (w, h) = (12, 12);
        let n = w * h;
        let mut color = vec![0u8; n * 4];
        let mut preview = vec![0u8; n * 4];
        let mut alpha = vec![255u8; n];
        let mut straight = vec![0u8; n * 4];
        let edge = vec![false; n];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let o = i * 4;
                let in_patch = (3..9).contains(&x) && (3..9).contains(&y);
                let (f, a) = if in_patch {
                    ([200u8, 120, 60], 64u8)
                } else {
                    ([200u8, 120, 60], 255u8)
                };
                let c = premultiplied(f, a);
                color[o..o + 4].copy_from_slice(&c);
                preview[o..o + 4].copy_from_slice(&[f[0], f[1], f[2], 255]);
            }
        }
        let changed = solve(
            &preview,
            &color,
            &mut alpha,
            &mut straight,
            &edge,
            w,
            h,
            &Solve {
                solid: 0.88,
                min_area: 20,
            },
        );
        assert_eq!(changed, 36, "6×6 的斑块应整块被解出来");
        assert!(alpha[4 * w + 4] < 80, "斑块内应变成半透明");
        assert_eq!(alpha[0], 255, "斑块外保持不透明");
    }

    #[test]
    fn ignores_small_speckles() {
        let (w, h) = (12, 12);
        let n = w * h;
        let mut color = vec![0u8; n * 4];
        let mut preview = vec![0u8; n * 4];
        let mut alpha = vec![255u8; n];
        let mut straight = vec![0u8; n * 4];
        let edge = vec![false; n];
        // 一个 2×2 的低比值点，面积不够，必须保持不透明。
        for i in 0..n {
            let o = i * 4;
            color[o..o + 4].copy_from_slice(&[200, 120, 60, 255]);
            preview[o..o + 4].copy_from_slice(&[200, 120, 60, 255]);
        }
        for y in 5..7 {
            for x in 5..7 {
                let o = (y * w + x) * 4;
                color[o..o + 3].copy_from_slice(&[40, 24, 12]);
            }
        }
        let changed = solve(
            &preview,
            &color,
            &mut alpha,
            &mut straight,
            &edge,
            w,
            h,
            &Solve::default(),
        );
        assert_eq!(changed, 0);
        assert!(alpha.iter().all(|&a| a == 255));
    }

    #[test]
    fn edge_pixels_bypass_the_area_filter() {
        // 同样是一个孤立像素：在轮廓带上要改（那是抗锯齿），在内部不改（那是噪声）。
        let (w, h) = (8, 8);
        let n = w * h;
        let mut color = vec![0u8; n * 4];
        let mut preview = vec![0u8; n * 4];
        let mut alpha = vec![255u8; n];
        let mut straight = vec![0u8; n * 4];
        let mut edge = vec![false; n];
        for i in 0..n {
            let o = i * 4;
            color[o..o + 4].copy_from_slice(&[200, 120, 60, 255]);
            preview[o..o + 4].copy_from_slice(&[200, 120, 60, 255]);
        }
        // (1,1) 标记成轮廓带，(5,5) 不标记，两个位置都压暗成 10% 亮度。
        edge[w + 1] = true;
        for i in [w + 1, 5 * w + 5] {
            let o = i * 4;
            color[o..o + 3].copy_from_slice(&[20, 12, 6]);
        }
        let changed = solve(
            &preview,
            &color,
            &mut alpha,
            &mut straight,
            &edge,
            w,
            h,
            &Solve::default(),
        );
        assert_eq!(changed, 1, "只有轮廓带那个像素应被改");
        assert!(alpha[w + 1] < 100, "轮廓带像素变半透明");
        assert_eq!(alpha[5 * w + 5], 255, "内部孤立像素保持不透明");
    }
}
