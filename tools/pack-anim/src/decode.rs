//! 解码：调外部 `ffmpeg` 把 webm 解成定尺寸 RGBA 裸流，逐帧读出。
//!
//! 为什么不用 Rust 侧的视频库：透明/抠像链路只需要「解一帧、拿像素」，
//! ffmpeg 的 `scale` 滤镜已经把缩放与色彩转换做到位（且与上游 Python 链路同源），
//! 自己引 libav 绑定只会让构建复杂化。

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

use anyhow::{ensure, Context, Result};

/// 逐帧读取的 RGBA 裸流。
pub struct FrameSource {
    child: Child,
    stdout: BufReader<ChildStdout>,
    frame_len: usize,
    width: u32,
    height: u32,
}

impl FrameSource {
    /// 起一个 ffmpeg 进程，把 `input` 等比缩放到 `height` 高、RGBA 裸流输出到管道。
    pub fn spawn(ffmpeg: &str, input: &Path, width: u32, height: u32) -> Result<Self> {
        let filter = format!("scale={width}:{height}:flags=lanczos");
        let mut child = Command::new(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .arg("-i")
            .arg(input)
            .args(["-vf", &filter])
            .args(["-pix_fmt", "rgba", "-f", "rawvideo", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("无法启动 ffmpeg（{ffmpeg}）"))?;

        let stdout = child.stdout.take().context("ffmpeg 没有 stdout 管道")?;
        Ok(Self {
            child,
            stdout: BufReader::with_capacity(1 << 20, stdout),
            frame_len: width as usize * height as usize * 4,
            width,
            height,
        })
    }

    /// 读下一帧到 `buf`（整帧覆盖）。返回 `false` 表示流结束。
    pub fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool> {
        buf.clear();
        buf.resize(self.frame_len, 0);
        let mut filled = 0;
        while filled < self.frame_len {
            let n = self.stdout.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            return Ok(false);
        }
        ensure!(
            filled == self.frame_len,
            "帧数据不完整：{filled}/{} 字节（{}×{}）",
            self.frame_len,
            self.width,
            self.height
        );
        Ok(true)
    }

    /// 输出尺寸（已缩放后的）。
    #[allow(dead_code)]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for FrameSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 用 `ffprobe` 读源视频宽高。
pub fn probe_size(ffprobe: &str, input: &Path) -> Result<(u32, u32)> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output()
        .with_context(|| format!("无法启动 ffprobe（{ffprobe}）"))?;
    ensure!(
        out.status.success(),
        "ffprobe 失败：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    let mut it = line.split(',');
    let w: u32 = it
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .context("ffprobe 宽解析失败")?;
    let h: u32 = it
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .context("ffprobe 高解析失败")?;
    ensure!(w > 0 && h > 0, "源视频尺寸非法：{w}×{h}");
    Ok((w, h))
}

/// 按目标高度换算宽度，并向下取偶数（避免奇数宽导致的半像素问题）。
pub fn width_for_height(src_w: u32, src_h: u32, height: u32) -> u32 {
    let w = (src_w as f64 * height as f64 / src_h as f64).round() as u32;
    (w.max(2)) & !1
}
