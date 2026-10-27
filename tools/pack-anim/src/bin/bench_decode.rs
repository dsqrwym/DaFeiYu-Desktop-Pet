//! 解码耗时基准：把「读盘 / 解压 / 差累加」三段拆开量。
//!
//! 存在的理由：整帧解码慢时，猜是最没效率的做法。这个工具直接给出三段各自的
//! 每帧耗时，谁贵一眼可见。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use feiyu_core::anim::format::{read_header, Codec};
use feiyu_core::anim::AnimReader;

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("用法：bench-decode <文件.fishanim>")?;
    let file = File::open(&path).with_context(|| format!("打不开 {path}"))?;
    let mut r = BufReader::with_capacity(64 * 1024, file);
    let (header, table) = read_header(&mut r)?;
    let frame_len = header.byte_len();
    println!(
        "{}：{} 帧 {}×{} 关键帧间隔 {}\n",
        path, header.frame_count, header.width, header.height, header.key_every
    );

    let mut staged: Vec<u8> = Vec::new();
    let mut out = vec![0u8; frame_len];
    let mut dec = zstd::bulk::Decompressor::new()?;

    // 第一遍：只读盘（把文件读进 OS 缓存，也量 I/O 本身）。
    let t = Instant::now();
    for e in &table {
        staged.resize(e.len as usize, 0);
        r.seek(SeekFrom::Start(e.offset as u64))?;
        r.read_exact(&mut staged)?;
    }
    let io = t.elapsed().as_secs_f64() * 1000.0;

    // 第二遍：读盘 + 解压（复用解压上下文）。
    let t = Instant::now();
    for e in &table {
        staged.resize(e.len as usize, 0);
        r.seek(SeekFrom::Start(e.offset as u64))?;
        r.read_exact(&mut staged)?;
        match e.codec {
            Codec::Zstd => {
                let got = dec.decompress_to_buffer(&staged, &mut out[..])?;
                if got != frame_len {
                    bail!("解压长度不符：{got} != {frame_len}");
                }
            }
            Codec::Raw => out[..].copy_from_slice(&staged),
        }
    }
    let decomp = t.elapsed().as_secs_f64() * 1000.0;

    // 第三遍：单独的差分累加（内存带宽上限参考）。
    let delta = vec![1u8; frame_len];
    let t = Instant::now();
    for _ in 0..table.len() {
        for (a, b) in out.iter_mut().zip(delta.iter()) {
            *a = a.wrapping_add(*b);
        }
    }
    let add = t.elapsed().as_secs_f64() * 1000.0;

    let n = table.len() as f64;
    println!("①只读盘      {:>8.3} ms/帧", io / n);
    println!("②读盘+解压   {:>8.3} ms/帧（解压本身 {:.3}）", decomp / n, (decomp - io) / n);
    println!("③差分累加    {:>8.3} ms/帧", add / n);
    println!(
        "   帧字节 {} KiB，解压吞吐 {:.0} MB/s",
        frame_len / 1024,
        (frame_len as f64 * n) / ((decomp - io) / 1000.0) / 1e6
    );

    // 第四遍：走真正的运行时接口 AnimReader，顺序取完全部帧。
    // 只有这一步和桌宠实际跑的代码完全一致，上三遍是拆解用的下限参考。
    let mut reader = AnimReader::open(std::path::Path::new(&path))?;
    let mut per_frame = Vec::new();
    let t = Instant::now();
    for idx in 0..reader.frame_count() {
        let t1 = Instant::now();
        let px = reader.frame(idx)?;
        if px.len() != frame_len {
            bail!("帧长不符：{} != {frame_len}", px.len());
        }
        per_frame.push(t1.elapsed().as_secs_f64() * 1000.0);
    }
    let reader_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("④AnimReader  {:>8.3} ms/帧", reader_ms / n);
    // 每帧单独计时，看耗时是不是沿着帧号漂移（数据量/缓存影响）。
    let mut sorted = per_frame.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    println!(
        "   分布 min {:.3} / p50 {:.3} / p90 {:.3} / max {:.3} ms",
        sorted[0],
        pick(0.5),
        pick(0.9),
        sorted[sorted.len() - 1]
    );
    // 每 16 帧（一个差分链）的平均，链首重解会在这里显现。
    let groups: Vec<String> = per_frame
        .chunks(16)
        .enumerate()
        .take(8)
        .map(|(g, c)| format!("{:>2}:{:.2}", g * 16, c.iter().sum::<f64>() / c.len() as f64))
        .collect();
    println!("   每 16 帧均耗时：{}", groups.join("  "));
    Ok(())
}
