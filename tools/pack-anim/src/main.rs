//! 素材打包工具：webm → 黑底泛洪抠像 → 预乘 BGRA → `.fishanim`
//!
//! 用法（单文件或整目录）：
//!
//! ```text
//! cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets
//! cargo run -p pack-anim --release -- --src path/to/一段.webm --out target/measure --height 306
//! ```
//!
//! 链路里的每一步都刻意保持「可复现 + 零第三方依赖」：解码/缩放交给外部 ffmpeg，
//! 抠像与打包在本程序内完成，产物只依赖 `feiyu-core` 的格式定义。

mod decode;
mod mask;

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use feiyu_core::anim::format::{self, AnimHeader};
use feiyu_core::anim::AnimReader;

/// 中文素材名 → ASCII 文件名。中文只留在 `manifest.json` 里做显示，
/// 文件名一律 ASCII：避免 git、zip、`ffmpeg` 三处的中文路径隐患。
const NAME_MAP: &[(&str, &str)] = &[
    ("待机呼吸休闲", "idle"),
    ("东张西望", "turn"),
    ("被鼠标拖拽悬空反馈", "drag"),
    ("螃蟹走路", "move_crab"),
    ("原地漂浮踏步", "move_float"),
    ("原地左转奔跑", "move_run"),
    ("点击回应-开心跃动", "click_happy"),
    ("点击回应-害羞惊讶", "click_shy"),
    ("点击回应-傲娇生气", "click_pout"),
    ("原地小憩沉眠", "sleep"),
    ("超大伸懒腰", "stretch"),
    ("照镜子", "mirror"),
];

struct Opts {
    src: PathBuf,
    out: PathBuf,
    height: u32,
    fps: f32,
    level: i32,
    /// 帧间差分的关键帧间隔（0/1 = 不差分）。
    delta: u16,
    /// 每隔几帧导出一张 PNG 做肉眼抽检（0 = 不导出）。
    dump: u32,
    /// 裁到全片不透明像素的并集包围盒（默认开）。
    crop: bool,
    thresh: u8,
    feather: u8,
    /// 丢掉色彩通道的低 n 位（0 = 不量化）。噪点大多是低位的抖动，砍掉不影响观感但能显著降低熵。
    quant: u8,
    ffmpeg: String,
    ffprobe: String,
    limit: Option<usize>,
    /// 只对目录内的 `.fishanim` 做自检，不打包。
    verify: Option<PathBuf>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            src: PathBuf::new(),
            out: PathBuf::from("assets"),
            height: 306,
            fps: 24.0,
            level: 3,
            delta: 16,
            dump: 0,
            crop: true,
            thresh: 12,
            feather: 0,
            quant: 3,
            ffmpeg: "ffmpeg".to_string(),
            ffprobe: "ffprobe".to_string(),
            limit: None,
            verify: None,
        }
    }
}

/// 单个素材的打包结果，用于汇总打印。
struct Row {
    slug: String,
    name: String,
    width: u32,
    height: u32,
    frames: u32,
    raw_bytes: u64,
    packed_bytes: u64,
    baseline: u16,
    elapsed_ms: u128,
    /// 不透明像素的并集包围盒 (x0, y0, x1, y1)。
    bbox: (u32, u32, u32, u32),
    /// 单帧不透明像素数的 (最少, 最多)。
    opaque: (u32, u32),
    /// 裁切帧在原画布中的左上角。
    origin: (u32, u32),
    /// 原画布尺寸（未裁切）。
    canvas: (u32, u32),
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let opts = parse_args()?;
    if let Some(dir) = &opts.verify {
        return verify(dir);
    }
    fs::create_dir_all(&opts.out)
        .with_context(|| format!("无法创建输出目录 {}", opts.out.display()))?;

    let jobs = collect(&opts)?;
    if jobs.is_empty() {
        bail!("没有找到可打包的素材：{}", opts.src.display());
    }

    println!(
        "打包 {} 段素材 → {}（高 {}px / {}fps / zstd {} / 差分 {} / 量化 {} 位 / 阈值 {} / 羽化 {} / 裁切 {}）\n",
        jobs.len(),
        opts.out.display(),
        opts.height,
        opts.fps,
        opts.level,
        opts.delta,
        opts.quant,
        opts.thresh,
        opts.feather,
        if opts.crop { "开" } else { "关" }
    );

    let mut rows = Vec::with_capacity(jobs.len());
    for (path, slug, name) in jobs {
        let row = pack_one(&opts, &path, &slug, &name)?;
        print_row(&row);
        rows.push(row);
    }

    print_summary(&rows);
    write_manifest(&opts, &rows)?;
    Ok(())
}

fn parse_args() -> Result<Opts> {
    let mut opts = Opts::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || -> Result<String> {
            args.next().with_context(|| format!("{arg} 缺少取值"))
        };
        match arg.as_str() {
            "--src" => opts.src = PathBuf::from(value()?),
            "--out" => opts.out = PathBuf::from(value()?),
            "--height" => opts.height = value()?.parse().context("--height 需要正整数")?,
            "--fps" => opts.fps = value()?.parse().context("--fps 需要数字")?,
            "--level" => opts.level = value()?.parse().context("--level 需要整数")?,
            "--delta" => opts.delta = value()?.parse().context("--delta 需要 0 或正整数")?,
            "--dump" => opts.dump = value()?.parse().context("--dump 需要正整数")?,
            "--no-crop" => opts.crop = false,
            "--thresh" => opts.thresh = value()?.parse().context("--thresh 需要 0~255")?,
            "--feather" => opts.feather = value()?.parse().context("--feather 需要 0~255")?,
            "--quant" => opts.quant = value()?.parse().context("--quant 需要 0~7")?,
            "--ffmpeg" => opts.ffmpeg = value()?,
            "--ffprobe" => opts.ffprobe = value()?,
            "--limit" => opts.limit = Some(value()?.parse().context("--limit 需要正整数")?),
            "--verify" => opts.verify = Some(PathBuf::from(value()?)),
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("未知参数 {other}（--help 查看用法）"),
        }
    }
    if opts.src.as_os_str().is_empty() && opts.verify.is_none() {
        bail!("必须指定 --src");
    }
    if opts.height < 16 {
        bail!("--height 太小（最小 16）");
    }
    Ok(opts)
}

fn print_help() {
    println!(
        "pack-anim —— webm → .fishanim\n\n\
         参数：\n\
         \x20 --src <文件|目录>   输入 webm\n\
         \x20 --out <目录>       输出目录（默认 assets）\n\
         \x20 --height <px>      渲染高度（默认 306）\n\
         \x20 --fps <n>          写入帧头的帧率（默认 24）\n\
         \x20 --level <n>        zstd 级别，0 = 不压缩（默认 3）\n\
         \x20 --delta <n>        帧间差分的关键帧间隔，0/1 = 关闭（默认 16）\n\
         \x20 --dump <n>         每隔 n 帧导出一张 PNG 抽检（默认 0 = 不导出）\n\
         \x20 --no-crop          不裁切（保留原画布，用于对比体积）\n\
         \x20 --thresh <0-255>   黑底亮度阈值（默认 12）\n\
         \x20 --feather <0-255>  边缘羽化带宽度，0 = 硬边（默认 0）\n\
         \x20 --quant <0-7>      砍掉色彩通道低 n 位（0 = 不量化，默认 3）\n\
         \x20 --ffmpeg/--ffprobe 可执行文件路径\n\
         \x20 --limit <n>        只处理前 n 个（调试用）\n\
         \x20 --verify <目录>    只做自检：逐个解帧、校验差分链一致性"
    );
}

/// 收集待打包的 (路径, slug, 中文名)。目录模式下只认 `NAME_MAP` 里登记过的素材。
fn collect(opts: &Opts) -> Result<Vec<(PathBuf, String, String)>> {
    let mut jobs = Vec::new();
    if opts.src.is_file() {
        let stem = opts
            .src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "anim".to_string());
        let slug = NAME_MAP
            .iter()
            .find(|(cn, _)| *cn == stem)
            .map(|(_, en)| (*en).to_string())
            .unwrap_or_else(|| "single".to_string());
        jobs.push((opts.src.clone(), slug, stem));
        return Ok(jobs);
    }

    let dir = fs::read_dir(&opts.src)
        .with_context(|| format!("无法读取目录 {}", opts.src.display()))?;
    let mut found: Vec<(PathBuf, String, String)> = Vec::new();
    for entry in dir {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("webm") {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match NAME_MAP.iter().find(|(cn, _)| *cn == stem) {
            Some((_, slug)) => found.push((path, (*slug).to_string(), stem)),
            None => eprintln!("跳过未登记素材：{stem}"),
        }
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    if let Some(limit) = opts.limit {
        found.truncate(limit);
    }
    jobs.extend(found);
    Ok(jobs)
}

fn pack_one(opts: &Opts, src: &Path, slug: &str, name: &str) -> Result<Row> {
    let started = Instant::now();
    let (src_w, src_h) = decode::probe_size(&opts.ffprobe, src)?;
    let height = opts.height;
    let width = decode::width_for_height(src_w, src_h, height);

    let mut source = decode::FrameSource::spawn(&opts.ffmpeg, src, width, height)?;
    let w = width as usize;
    let h = height as usize;
    let mut buf = Vec::new();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    // 从 0 起累加最大值：初始值若给 height，`max` 会永远被压在底边。
    let mut baseline = 0u16;
    let mut bbox = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut opaque_min = u32::MAX;
    let mut opaque_max = 0u32;
    let mut idx = 0u32;

    while source.next_frame(&mut buf)? {
        let alpha = mask::build_alpha(&buf, w, h, opts.thresh, opts.feather);
        for (i, a) in alpha.iter().enumerate() {
            buf[i * 4 + 3] = *a;
        }
        if opts.quant > 0 {
            mask::quantize(&mut buf, opts.quant);
        }
        if let Some(bottom) = mask::last_opaque_row(&alpha, w, h) {
            baseline = baseline.max(bottom.min(h) as u16);
        }
        // 统计不透明像素与包围盒：用来发现抠像噪声（真角色不会铺满整帧）。
        let mut opaque = 0u32;
        for (i, a) in alpha.iter().enumerate() {
            if *a == 0 {
                continue;
            }
            opaque += 1;
            let x = (i % w) as u32;
            let y = (i / w) as u32;
            bbox.0 = bbox.0.min(x);
            bbox.1 = bbox.1.min(y);
            bbox.2 = bbox.2.max(x);
            bbox.3 = bbox.3.max(y);
        }
        opaque_min = opaque_min.min(opaque);
        opaque_max = opaque_max.max(opaque);

        if opts.dump > 0 && idx % opts.dump == 0 {
            dump_png(&opts.out, slug, idx, width, height, &buf)?;
        }
        idx += 1;
        frames.push(buf.clone());
    }

    if frames.is_empty() {
        bail!("{} 没有解出任何帧", src.display());
    }
    if bbox.0 == u32::MAX {
        bail!("{} 整段没有不透明像素，抠像阈值（--thresh {}）可能不对", src.display(), opts.thresh);
    }

    // 裁到全片不透明像素的并集：角色只占 16:9 画面的三成，空白纯属浪费。
    // 裁切信息记在帧头的 origin，渲染端据此还原原画布上的位置，换动画不会跳位。
    let (origin_x, origin_y, frame_w, frame_h) = if opts.crop {
        let (x0, y0) = (bbox.0 as usize, bbox.1 as usize);
        let (cw, ch) = ((bbox.2 + 1 - bbox.0) as usize, (bbox.3 + 1 - bbox.1) as usize);
        for f in frames.iter_mut() {
            *f = crop(f, w, x0, y0, cw, ch);
        }
        (x0 as u32, y0 as u32, cw as u32, ch as u32)
    } else {
        (0, 0, width, height)
    };
    // baseline 原是画布坐标，裁切后换成帧内坐标。
    let baseline = baseline.saturating_sub(origin_y as u16);

    // ffmpeg 输出的是 RGBA，容器约定的是 BGRA（与 Windows DIB 同序，渲染时零转换）。
    // 在这里统一交换，避免把通道顺序的坑留给渲染端。
    for f in frames.iter_mut() {
        to_bgra(f);
    }

    let frame_count = frames.len() as u32;
    let raw_bytes = frame_count as u64 * (frame_w as u64 * frame_h as u64 * 4);
    let header = AnimHeader::new(frame_w as u16, frame_h as u16, opts.fps, baseline)
        .with_origin(origin_x as u16, origin_y as u16);
    let packed = format::encode_with(header, frames, opts.level, opts.delta)?;

    let path = opts.out.join(format!("{slug}.fishanim"));
    let mut out = BufWriter::new(File::create(&path)?);
    format::write(&mut out, &packed)?;

    Ok(Row {
        slug: slug.to_string(),
        name: name.to_string(),
        width: frame_w,
        height: frame_h,
        frames: frame_count,
        raw_bytes,
        packed_bytes: fs::metadata(&path)?.len(),
        baseline,
        elapsed_ms: started.elapsed().as_millis(),
        bbox,
        opaque: (opaque_min, opaque_max),
        origin: (origin_x, origin_y),
        canvas: (width, height),
    })
}

/// 从画布帧里取出 `(x0, y0)` 起、`w × h` 的子矩形（逐行拷，保留预乘 BGRA）。
fn crop(frame: &[u8], canvas_w: usize, x0: usize, y0: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for y in y0..y0 + h {
        let start = (y * canvas_w + x0) * 4;
        out.extend_from_slice(&frame[start..start + w * 4]);
    }
    out
}

/// RGBA → BGRA（就地对调 R/B）。alpha 与预乘关系不受影响。
fn to_bgra(frame: &mut [u8]) {
    for px in frame.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// 导出抽检 PNG：把预乘像素合成到棋盘底上（透明区一看就知道）。
fn dump_png(dir: &Path, slug: &str, idx: u32, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let mut img = image::RgbaImage::new(w, h);
    for (i, px) in img.pixels_mut().enumerate() {
        let o = i * 4;
        let a = rgba[o + 3] as u32;
        let x = (i as u32) % w;
        let y = (i as u32) / w;
        let bg = if ((x / 12) + (y / 12)) % 2 == 0 { 90u32 } else { 190u32 };
        let mix = |c: u8| ((c as u32 * a + bg * (255 - a)) / 255) as u8;
        *px = image::Rgba([mix(rgba[o]), mix(rgba[o + 1]), mix(rgba[o + 2]), 255]);
    }
    img.save(dir.join(format!("{slug}_f{idx:03}.png")))?;
    Ok(())
}

fn print_row(row: &Row) {
    println!(
        "  {:<12} {:>4} 帧  {:>3}×{:<3}  底 {:>3}  原始 {:>7}  →  {:>7}  ({:>4.1}×)  {:>5}ms",
        row.slug,
        row.frames,
        row.width,
        row.height,
        row.baseline,
        kib(row.raw_bytes),
        kib(row.packed_bytes),
        row.raw_bytes as f64 / row.packed_bytes.max(1) as f64,
        row.elapsed_ms
    );
    println!(
        "  {:<12} 画布 {}×{} 偏移 ({},{})  包围盒 x{}-{} y{}-{}  不透明像素 {}-{}",
        "",
        row.canvas.0,
        row.canvas.1,
        row.origin.0,
        row.origin.1,
        row.bbox.0,
        row.bbox.2,
        row.bbox.1,
        row.bbox.3,
        row.opaque.0,
        row.opaque.1
    );
}

fn print_summary(rows: &[Row]) {
    let raw: u64 = rows.iter().map(|r| r.raw_bytes).sum();
    let packed: u64 = rows.iter().map(|r| r.packed_bytes).sum();
    let frames: u32 = rows.iter().map(|r| r.frames).sum();
    println!("\n合计 {} 段 / {} 帧", rows.len(), frames);
    println!("原始 {:>7}  →  打包 {:>7}   （压缩 {:.1}×）", kib(raw), kib(packed), raw as f64 / packed.max(1) as f64);
}

fn kib(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{}K", bytes / 1024)
    }
}

/// 写出 `manifest.json`：运行时只看这一个索引就能定位每段动画及其在原画布上的位置。
fn write_manifest(opts: &Opts, rows: &[Row]) -> Result<()> {
    let canvas = rows.first().map(|r| r.canvas).unwrap_or((0, 0));
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("  \"format\": 1,\n");
    s.push_str(&format!(
        "  \"canvas\": {{ \"width\": {}, \"height\": {} }},\n",
        canvas.0, canvas.1
    ));
    s.push_str(&format!(
        "  \"native_height\": {},\n  \"fps\": {},\n  \"animations\": [\n",
        opts.height, opts.fps
    ));
    for (i, row) in rows.iter().enumerate() {
        s.push_str("    {\n");
        s.push_str(&format!("      \"id\": \"{}\",\n", row.slug));
        s.push_str(&format!("      \"name\": \"{}\",\n", json_escape(&row.name)));
        s.push_str(&format!("      \"file\": \"{}.fishanim\",\n", row.slug));
        s.push_str(&format!(
            "      \"size\": [{}, {}],\n      \"origin\": [{}, {}],\n",
            row.width, row.height, row.origin.0, row.origin.1
        ));
        s.push_str(&format!(
            "      \"baseline\": {},\n      \"frames\": {},\n      \"bytes\": {}\n",
            row.baseline, row.frames, row.packed_bytes
        ));
        s.push_str(if i + 1 == rows.len() { "    }\n" } else { "    },\n" });
    }
    s.push_str("  ]\n}\n");

    let path = opts.out.join("manifest.json");
    fs::write(&path, s).with_context(|| format!("无法写入 {}", path.display()))?;
    println!("\n清单已写入 {}", path.display());
    Ok(())
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 自检：逐个文件顺序解帧，再倒序重取一遍，确认差分链从任意位置重算都一致。
fn verify(dir: &Path) -> Result<()> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("无法读取目录 {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("fishanim"))
        .collect();
    files.sort();
    if files.is_empty() {
        bail!("{} 里没有 .fishanim", dir.display());
    }

    let mut total = 0u64;
    let mut total_frames = 0u64;
    println!("自检 {} 个文件\n", files.len());
    for path in &files {
        let started = Instant::now();
        let mut reader = AnimReader::open(path)?;
        let h = *reader.header();
        let mut prints = Vec::with_capacity(h.frame_count as usize);
        let (mut min_op, mut max_op) = (usize::MAX, 0usize);

        for i in 0..h.frame_count {
            let f = reader.frame(i)?;
            ensure!(f.len() == h.byte_len(), "{} 第 {i} 帧长度不符", path.display());
            let op = f.chunks_exact(4).filter(|p| p[3] != 0).count();
            ensure!(op > 0, "{} 第 {i} 帧全透明", path.display());
            min_op = min_op.min(op);
            max_op = max_op.max(op);
            prints.push(fnv1a(f));
        }
        // 倒序重取：每次都会从链首重算，结果必须与顺序解出的一致。
        for i in (0..h.frame_count).rev() {
            ensure!(
                fnv1a(reader.frame(i)?) == prints[i as usize],
                "{} 第 {i} 帧重算结果与顺序解码不一致（差分链有 bug）",
                path.display()
            );
        }

        let bytes = fs::metadata(path)?.len();
        total += bytes;
        total_frames += h.frame_count as u64;
        println!(
            "  {:<12} {:>4}帧 {}×{} 底{:>3} 偏移({},{}) 不透明 {}-{}  {}  ✓ {:>4}ms",
            path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            h.frame_count,
            h.width,
            h.height,
            h.baseline,
            h.origin_x,
            h.origin_y,
            min_op,
            max_op,
            kib(bytes),
            started.elapsed().as_millis()
        );
    }
    println!(
        "\n自检通过：{} 个文件 / {} 帧 / {}，全部帧可随机访问且差分链可重算",
        files.len(),
        total_frames,
        kib(total)
    );
    Ok(())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
