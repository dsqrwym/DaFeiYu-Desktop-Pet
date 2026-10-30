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

// MSRV 1.80：`as_chunks` / `is_multiple_of` 还没稳定，保留手写写法。
#![allow(clippy::chunks_exact_to_as_chunks, clippy::manual_is_multiple_of)]

mod decode;
mod mask;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use feiyu_core::anim::{format, AnimHeader, AnimReader, MaskSet};

/// 中文素材名 → ASCII 文件名。中文只留在 `manifest.json` 里做显示，
/// 文件名一律 ASCII：避免 git、zip、`ffmpeg` 三处的中文路径隐患。
///
/// 表里**只登记桌宠会用的 100 段**：余额事件 6 段（`余额-*`）是 DSH 余额接口的
/// 专属演出，本项目不接 DSH，所以刻意不登记——目录模式下会被「跳过未登记素材」跳过。
/// 顺序按分类排，和 `pet.rs` 的 `CATEGORIES` 一一对应，改名字时两边一起改。
const NAME_MAP: &[(&str, &str)] = &[
    // ---- 系统状态（不进菜单、不进随机池）----
    ("待机呼吸休闲", "idle"),
    ("原地小憩沉眠", "sleep"),
    ("被鼠标拖拽悬空反馈", "drag"),
    // ---- 转向 / 移动 ----
    ("东张西望", "turn"),
    ("螃蟹走路", "move_crab"),
    ("原地漂浮踏步", "move_float"),
    ("原地左转奔跑", "move_run"),
    // ---- 点击回应 ----
    ("点击回应-开心跃动", "click_happy"),
    ("点击回应-害羞惊讶", "click_shy"),
    ("点击回应-傲娇生气", "click_pout"),
    ("点击回应-挠痒咯咯笑", "click_tickle"),
    ("点击回应-元气挥手", "click_wave"),
    // ---- 小动作 ----
    ("悠闲哼歌", "hum"),
    ("超大伸懒腰", "stretch"),
    ("原地敲击桌面互动", "tap_desk"),
    ("原地重力下蹲压缩", "squat"),
    ("哈欠连天", "yawn"),
    ("女仆屈膝礼仪", "curtsey"),
    ("被吓一跳", "startle"),
    ("小幅度原地360度旋转展示", "spin360"),
    ("偷吃零食被抓住", "snack_caught"),
    ("用鲸鱼尾巴拍打地面", "tail_pat"),
    ("打瞌睡被惊醒", "doze_startle"),
    ("照镜子", "mirror"),
    ("整体换装试色", "dressup"),
    ("轻快记录", "note"),
    ("写代码", "code"),
    ("摇扇纳凉", "fan"),
    ("晨间刷牙", "brush_teeth"),
    // ---- 玩耍 ----
    ("原地专心玩魔方", "rubik"),
    ("原地蹲下玩玩具汽车", "toy_car"),
    ("鲸鱼吐泡泡特效", "bubbles"),
    ("原地跳跃抓碎头顶物品", "jump_grab"),
    ("玩游戏气急败坏", "game_rage"),
    ("玩水枪", "water_gun"),
    ("小提琴演奏", "violin"),
    ("蓝鲸现世", "whale"),
    ("优雅女仆舞", "maid_dance"),
    ("轻快摇摆舞", "sway_dance"),
    ("可爱宅舞", "otaku_dance"),
    ("吹气球", "balloon"),
    ("动物环绕", "animals"),
    ("放风筝", "kite"),
    ("拆礼物", "gift"),
    ("变鸽子", "dove"),
    ("扑克魔术", "poker"),
    ("抽陀螺", "top"),
    ("吹笛子", "flute"),
    ("蝴蝶蜜蜂环绕头顶开花", "butterflies"),
    ("撸猫", "pet_cat"),
    ("凭空生花", "flowers"),
    ("骑木马", "hobby_horse"),
    ("三球抛接", "juggle"),
    ("踢毽子", "shuttlecock"),
    ("下五子棋", "gomoku"),
    ("荡秋千", "swing"),
    // ---- 吃什么 ----
    ("吃白饭", "eat_rice"),
    ("大口吃零食", "eat_snack"),
    ("吃Token", "eat_token"),
    ("吃早餐", "eat_breakfast"),
    ("吃午餐", "eat_lunch"),
    ("吃晚餐", "eat_dinner"),
    ("吃冰淇淋融化", "eat_icecream"),
    ("吃大闸蟹", "eat_crab"),
    ("吃糖葫芦", "eat_tanghulu"),
    ("吃长寿面", "eat_noodles"),
    ("吃西瓜", "eat_watermelon"),
    ("涮火锅", "hotpot"),
    // ---- 时节 ----
    ("被落叶淹没", "leaves"),
    ("中秋赏月吃月饼", "mooncake"),
    ("堆雪人", "snowman"),
    ("放烟花", "fireworks"),
    ("吃粽子", "eat_zongzi"),
    ("吃年糕", "eat_niangao"),
    ("吃青团", "eat_qingtuan"),
    ("吃腊八粥", "eat_laba"),
    ("吃重阳糕", "eat_chongyang"),
    ("收红包", "red_packet"),
    ("写福字", "fu_character"),
    ("穿针乞巧", "needle"),
    ("舞狮头", "lion_dance"),
    ("讨糖南瓜灯", "halloween"),
    ("插茱萸赏菊", "chrysanthemum"),
    ("放河灯", "river_lantern"),
    ("萌化小幽灵", "ghost"),
    ("装点圣诞树", "christmas"),
    ("放孔明灯", "sky_lantern"),
    ("吃汤圆", "eat_tangyuan"),
    ("吃饺子", "eat_dumpling"),
    // ---- 文字 ----
    ("是啊，吃什么", "what_to_eat"),
    ("深度思考碎碎念", "deep_think"),
    // ---- 碎碎念（原 DSH 事件动画，现在只由右键菜单点播）----
    ("碎碎念-擦桌碎碎念", "whisper_wipe"),
    ("碎碎念-发呆碎碎念", "whisper_daze"),
    ("碎碎念-对屏碎碎念", "whisper_screen"),
    // ---- 工作状态（原 DSH 事件动画，现在只由右键菜单点播）----
    ("工作状态-思考冒泡", "work_think"),
    ("工作状态-忙碌点按", "work_busy"),
    ("工作状态-清点归档", "work_tidy"),
    ("工作状态-原地踱步张望", "work_pace"),
    ("工作状态-雀跃庆祝", "work_cheer"),
    ("工作状态-垂头叹气冒汗", "work_sigh"),
];

struct Opts {
    src: PathBuf,
    out: PathBuf,
    height: u32,
    fps: f32,
    /// 输出格式：旧的 `.fishanim` 或多段 H.264 + `.fishmask`。
    codec: PackCodec,
    /// H.264 的 CRF。
    crf: i32,
    level: i32,
    /// 帧间差分的关键帧间隔（0/1 = 不差分）。
    delta: u16,
    /// 每隔几帧导出一张 PNG 做肉眼抽检（0 = 不导出）。
    dump: u32,
    /// 裁到全片不透明像素的并集包围盒（默认开）。
    crop: bool,
    thresh: u8,
    /// 边缘过渡带宽度（像素）：核心外的这几圈按「最近核心色」反推 alpha。
    rim: u8,
    /// 丢掉色彩通道的低 n 位（0 = 不量化）。噪点大多是低位的抖动，砍掉不影响观感但能显著降低熵。
    quant: u8,
    ffmpeg: String,
    ffprobe: String,
    limit: Option<usize>,
    /// 只对目录内的 `.fishanim` 做自检，不打包。
    verify: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackCodec {
    FishAnim,
    H264,
}

impl PackCodec {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "fishanim" => Ok(Self::FishAnim),
            "h264" => Ok(Self::H264),
            other => bail!("--codec 只能是 fishanim 或 h264（收到 {other}）"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FishAnim => "fishanim",
            Self::H264 => "h264",
        }
    }
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            src: PathBuf::new(),
            out: PathBuf::from("assets"),
            height: 306,
            fps: 24.0,
            codec: PackCodec::FishAnim,
            crf: 20,
            level: 3,
            delta: 16,
            dump: 0,
            crop: true,
            thresh: 12,
            rim: 2,
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
        if opts.codec == PackCodec::H264 {
            bail!("--verify 目前只支持 fishanim；h264 的完整解码由应用自检验证");
        }
        return verify(dir);
    }
    fs::create_dir_all(&opts.out)
        .with_context(|| format!("无法创建输出目录 {}", opts.out.display()))?;

    let jobs = collect(&opts)?;
    if jobs.is_empty() {
        bail!("没有找到可打包的素材：{}", opts.src.display());
    }

    println!(
        "打包 {} 段素材 → {}（高 {}px / {}fps / codec {} / h264 CRF {} / zstd {} / 差分 {} / 量化 {} 位 / 阈值 {} / 过渡带 {}px / 裁切 {}）\n",
        jobs.len(),
        opts.out.display(),
        opts.height,
        opts.fps,
        opts.codec.label(),
        opts.crf,
        opts.level,
        opts.delta,
        opts.quant,
        opts.thresh,
        opts.rim,
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
        let mut value =
            || -> Result<String> { args.next().with_context(|| format!("{arg} 缺少取值")) };
        match arg.as_str() {
            "--src" => opts.src = PathBuf::from(value()?),
            "--out" => opts.out = PathBuf::from(value()?),
            "--height" => opts.height = value()?.parse().context("--height 需要正整数")?,
            "--fps" => opts.fps = value()?.parse().context("--fps 需要数字")?,
            "--codec" => opts.codec = PackCodec::parse(&value()?)?,
            "--crf" => opts.crf = value()?.parse().context("--crf 需要 0~51")?,
            "--level" => opts.level = value()?.parse().context("--level 需要整数")?,
            "--delta" => opts.delta = value()?.parse().context("--delta 需要 0 或正整数")?,
            "--dump" => opts.dump = value()?.parse().context("--dump 需要正整数")?,
            "--no-crop" => opts.crop = false,
            "--thresh" => opts.thresh = value()?.parse().context("--thresh 需要 0~255")?,
            "--rim" => opts.rim = value()?.parse().context("--rim 需要 0~255")?,
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
    if opts.codec == PackCodec::H264 && !(0..=51).contains(&opts.crf) {
        bail!("--crf 必须在 0~51 之间");
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
         \x20 --codec <name>     fishanim（默认）或 h264\n\
         \x20 --crf <n>          h264 的 CRF（默认 20）\n\
         \x20 --level <n>        zstd 级别，0 = 不压缩（默认 3）\n\
         \x20 --delta <n>        帧间差分的关键帧间隔，0/1 = 关闭（默认 16）\n\
         \x20 --dump <n>         每隔 n 帧导出一张 PNG 抽检（默认 0 = 不导出）\n\
         \x20 --no-crop          不裁切（保留原画布，用于对比体积）\n\
         \x20 --thresh <0-255>   黑底亮度阈值（默认 12）\n\
         \x20 --rim <0-255>      边缘过渡带宽度，0 = 硬边（默认 2）\n\
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

    let dir =
        fs::read_dir(&opts.src).with_context(|| format!("无法读取目录 {}", opts.src.display()))?;
    let mut found: Vec<(PathBuf, String, String)> = Vec::new();
    for entry in dir {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("webm") {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
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
    // alpha 单独留一份：`.fishanim` 还会把 alpha 写进帧里，而 H.264 的颜色帧
    // 是直通色（alpha 位不用），掩码只能从这份 alpha 建。
    let mut alphas: Vec<Vec<u8>> = Vec::new();
    // 从 0 起累加最大值：初始值若给 height，`max` 会永远被压在底边。
    let mut baseline = 0u16;
    // 每行“最底不透明行”出现在多少帧里。脚底线取它的**众数**：
    // 烟花火星 / 水花 / 气球只在一小部分帧飘到画面最底，取最大值会把整段抬起来。
    let mut bottoms = vec![0u32; h + 1];
    let mut bbox = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut opaque_min = u32::MAX;
    let mut opaque_max = 0u32;
    let mut idx = 0u32;

    let mix_params = mask::Params {
        thresh: opts.thresh,
        rim: opts.rim,
        ..mask::Params::default()
    };

    while source.next_frame(&mut buf)? {
        let ex = mask::extract(&buf, w, h, &mix_params);
        let alpha = ex.alpha;
        // `.fishanim` 存**预乘**色（原值 + alpha）。H.264 只有颜色通道，
        // 必须存**直通**色：1-bit 掩码配上预乘色就是一圈黑边。
        let mut frame = match opts.codec {
            PackCodec::FishAnim => {
                let mut f = buf.clone();
                for (i, a) in alpha.iter().enumerate() {
                    f[i * 4 + 3] = *a;
                }
                f
            }
            PackCodec::H264 => ex.straight,
        };
        if opts.quant > 0 {
            mask::quantize(&mut frame, opts.quant);
        }
        if let Some(bottom) = mask::last_opaque_row(&alpha, w, h) {
            let bottom = bottom.min(h);
            baseline = baseline.max(bottom as u16);
            bottoms[bottom] += 1;
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
            let mut preview = frame.clone();
            for (i, a) in alpha.iter().enumerate() {
                let o = i * 4;
                if opts.codec == PackCodec::H264 {
                    // 抽检图按预乘压平：直通色得先乘回 alpha，否则看着偏亮。
                    let k = *a as u32;
                    for c in 0..3 {
                        preview[o + c] = ((preview[o + c] as u32 * k + 127) / 255) as u8;
                    }
                }
                preview[o + 3] = *a;
            }
            dump_png(&opts.out, slug, idx, width, height, &preview)?;
        }
        idx += 1;
        alphas.push(alpha);
        frames.push(frame);
    }

    if frames.is_empty() {
        bail!("{} 没有解出任何帧", src.display());
    }
    if bbox.0 == u32::MAX {
        bail!(
            "{} 整段没有不透明像素，抠像阈值（--thresh {}）可能不对",
            src.display(),
            opts.thresh
        );
    }

    // 脚底线 = 逐帧最底行的众数（票数相同时取更靠下的一行）。
    // 取最大值做过一版：烟花（火星落到底）、气球、水枪（水花）会把整段抬 20 像素，
    // 因为最大值只在那几十帧里成立，而角色自己一直站在 330 行（源 360 高）。
    let ground = bottoms
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .max_by_key(|(row, n)| (**n, *row))
        .map(|(row, _)| row as u16)
        .unwrap_or(baseline);
    let baseline = ground.min(height as u16);

    // 裁到全片不透明像素的并集：角色只占 16:9 画面的三成，空白纯属浪费。
    // 裁切信息记在帧头的 origin，渲染端据此还原原画布上的位置，换动画不会跳位。
    let (origin_x, origin_y, frame_w, frame_h) = if opts.crop {
        let (x0, y0) = (bbox.0 as usize, bbox.1 as usize);
        let (cw, ch) = (
            (bbox.2 + 1 - bbox.0) as usize,
            (bbox.3 + 1 - bbox.1) as usize,
        );
        for f in frames.iter_mut() {
            *f = crop(f, w, x0, y0, cw, ch);
        }
        for a in alphas.iter_mut() {
            *a = crop_alpha(a, w, x0, y0, cw, ch);
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
    let packed_bytes = match opts.codec {
        PackCodec::FishAnim => {
            let header = AnimHeader::new(frame_w as u16, frame_h as u16, opts.fps, baseline)
                .with_origin(origin_x as u16, origin_y as u16);
            let packed = format::encode_with(header, frames, opts.level, opts.delta)?;
            let path = opts.out.join(format!("{slug}.fishanim"));
            let mut out = BufWriter::new(File::create(&path)?);
            format::write(&mut out, &packed)?;
            fs::metadata(&path)?.len()
        }
        PackCodec::H264 => pack_h264(opts, slug, &frames, &alphas, frame_w, frame_h)?,
    };

    Ok(Row {
        slug: slug.to_string(),
        name: name.to_string(),
        width: frame_w,
        height: frame_h,
        frames: frame_count,
        raw_bytes,
        packed_bytes,
        baseline,
        elapsed_ms: started.elapsed().as_millis(),
        bbox,
        opaque: (opaque_min, opaque_max),
        origin: (origin_x, origin_y),
        canvas: (width, height),
    })
}

/// H.264 颜色 + 1-bit 掩码：颜色走 ffmpeg/libx264，掩码由 core 的 MaskSet 写成。
///
/// `frames` 必须是**直通色**：H.264 不携带 alpha，播放端按掩码给 1/0，
/// 颜色若还是预乘值，半透明的边就会发黑。
fn pack_h264(
    opts: &Opts,
    slug: &str,
    frames: &[Vec<u8>],
    alphas: &[Vec<u8>],
    w: u32,
    h: u32,
) -> Result<u64> {
    ensure!(
        w <= u16::MAX as u32 && h <= u16::MAX as u32,
        "帧尺寸超出 u16：{w}×{h}"
    );
    ensure!(
        frames.len() == alphas.len(),
        "帧数与 alpha 数不一致：{} / {}",
        frames.len(),
        alphas.len()
    );
    let video_path = opts.out.join(format!("{slug}.fishvid"));
    let mask_path = opts.out.join(format!("{slug}.fishmask"));

    let masks = MaskSet::from_alpha_frames(w as u16, h as u16, alphas)?;
    masks.save(&mask_path)?;

    let raw_path =
        std::env::temp_dir().join(format!("feiyu-pack-{}-{slug}.bgra", std::process::id()));
    {
        let mut raw = BufWriter::new(File::create(&raw_path)?);
        for frame in frames {
            raw.write_all(frame)?;
        }
        raw.flush()?;
    }

    let even_w = w + (w % 2);
    let even_h = h + (h % 2);
    let gop = opts.fps.round().max(1.0) as u32;
    let size = format!("{w}x{h}");
    let fps = opts.fps.to_string();
    let crf = opts.crf.to_string();
    let gop = gop.to_string();
    let pad = format!("pad={even_w}:{even_h}:0:0:black,format=yuv420p");
    let raw_arg = raw_path.to_string_lossy().into_owned();
    let output = Command::new(&opts.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "bgra"])
        .args(["-s", &size])
        .args(["-framerate", &fps])
        .args(["-i", &raw_arg])
        .args([
            "-vf",
            &pad,
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &crf,
            "-pix_fmt",
            "yuv420p",
            "-r",
            &fps,
            "-g",
            &gop,
            "-keyint_min",
            &gop,
            "-sc_threshold",
            "0",
            "-an",
            "-f",
            "mp4",
        ])
        .arg(&video_path)
        .output()
        .with_context(|| format!("无法启动 ffmpeg（{}）", opts.ffmpeg))?;
    let _ = fs::remove_file(&raw_path);
    if !output.status.success() {
        bail!(
            "ffmpeg 编码 {slug} 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let bytes = fs::metadata(&video_path)?.len() + fs::metadata(&mask_path)?.len();
    println!(
        "  {:<12} h264 颜色 {}  + 掩码 {}",
        slug,
        kib(fs::metadata(&video_path)?.len()),
        kib(fs::metadata(&mask_path)?.len())
    );
    Ok(bytes)
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

/// 裁 alpha 平面（与 [`crop`] 同一套坐标）。
fn crop_alpha(alpha: &[u8], canvas_w: usize, x0: usize, y0: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for y in y0..y0 + h {
        let start = y * canvas_w + x0;
        out.extend_from_slice(&alpha[start..start + w]);
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
        let bg = if ((x / 12) + (y / 12)) % 2 == 0 {
            90u32
        } else {
            190u32
        };
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
    println!(
        "原始 {:>7}  →  打包 {:>7}   （压缩 {:.1}×）",
        kib(raw),
        kib(packed),
        raw as f64 / packed.max(1) as f64
    );
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
        s.push_str(&format!(
            "      \"name\": \"{}\",\n",
            json_escape(&row.name)
        ));
        if opts.codec == PackCodec::H264 {
            s.push_str("      \"codec\": \"h264\",\n");
            s.push_str(&format!("      \"file\": \"{}.fishvid\",\n", row.slug));
            s.push_str(&format!("      \"mask\": \"{}.fishmask\",\n", row.slug));
        } else {
            s.push_str(&format!("      \"file\": \"{}.fishanim\",\n", row.slug));
        }
        s.push_str(&format!(
            "      \"size\": [{}, {}],\n      \"origin\": [{}, {}],\n",
            row.width, row.height, row.origin.0, row.origin.1
        ));
        s.push_str(&format!(
            "      \"baseline\": {},\n      \"frames\": {},\n      \"bytes\": {}\n",
            row.baseline, row.frames, row.packed_bytes
        ));
        s.push_str(if i + 1 == rows.len() {
            "    }\n"
        } else {
            "    },\n"
        });
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
            ensure!(
                f.len() == h.byte_len(),
                "{} 第 {i} 帧长度不符",
                path.display()
            );
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
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
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
