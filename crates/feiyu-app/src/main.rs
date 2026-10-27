//! 桌宠主程序。
//!
//! P1 阶段只做「活过来」这件事：一个能逐像素透明的窗口，播一段 `.fishanim`，
//! 透明区鼠标穿透，拖动能走，右键退出（菜单在 P2 换成真菜单）。
//!
//! 三个运行模式：
//! * 无参数 —— 正常当桌宠跑；
//! * `--selftest` —— 无人值守自检，逐项打印通过/失败，退出码即结论；
//! * `--shot <文件>` —— 跑几帧后截屏存 BMP，用于肉眼验收。

mod selftest;

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use feiyu_core::anim::AnimReader;
use feiyu_platform::{pump, quit, Event, LayeredWindow, SingleInstance};

/// 默认播放的动画（先固定待机，P2 接入 manifest 与状态机）。
const DEFAULT_ANIM: &str = "assets/anim/idle.fishanim";

/// 计时器周期。8ms 足够驱动 24/50fps 两种节奏，且空闲时也不至于空转太凶。
const TICK_MS: u32 = 8;

/// 贴在任务栏上沿之上留的空隙（像素）。
const BOTTOM_GAP: i32 = 4;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--selftest") => {
            let ok = selftest::run()?;
            if ok {
                Ok(())
            } else {
                bail!("自检未全部通过")
            }
        }
        Some("--help" | "-h") => {
            print_help();
            Ok(())
        }
        _ => run_pet(parse_args(&args)?),
    }
}

struct Args {
    anim: PathBuf,
    shot: Option<PathBuf>,
    /// 只画一次后挂着不动，用于测量「静止时」的 CPU 底噪。
    r#static: bool,
    /// 基准模式：连续提交 n 帧并报告单帧耗时。
    bench: Option<u32>,
}

fn parse_args(args: &[String]) -> Result<Args> {
    let mut out = Args { anim: PathBuf::from(DEFAULT_ANIM), shot: None, r#static: false, bench: None };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--anim" => {
                out.anim = PathBuf::from(it.next().context("--anim 缺少路径")?);
            }
            "--shot" => {
                out.shot = Some(PathBuf::from(it.next().context("--shot 缺少路径")?));
            }
            "--static" => out.r#static = true,
            "--bench" => {
                out.bench = Some(it.next().context("--bench 缺少帧数")?.parse().context("--bench 需要整数")?)
            }
            other => bail!("未知参数 {other}（--help 查看用法）"),
        }
    }
    Ok(out)
}

fn print_help() {
    println!(
        "feiyu —— 桌宠\n\n\
         \x20 --anim <文件>    要播放的 .fishanim（默认 {DEFAULT_ANIM}）\n\
         \x20 --shot <文件>    跑 0.5 秒后截屏存 BMP 并退出\n\
         \x20 --static        画一帧后不再推进（测静止底噪）\n\
         \x20 --bench <n>     连续提交 n 帧，报告单帧耗时\n\
         \x20 --selftest       跑自检并打印逐项结果"
    );
}

/// 一只桌宠的运行时状态。
struct Pet {
    win: LayeredWindow,
    anim: AnimReader<BufReader<File>>,
    fps: f32,
    /// 当前帧号。
    frame: u32,
    /// 不足一帧的时间余量。
    acc: f32,
    last: Instant,
    drawn: u64,
}

impl Pet {
    fn open(path: &PathBuf, shot: bool) -> Result<Self> {
        let anim = AnimReader::open(path)
            .with_context(|| format!("打不开动画 {}", path.display()))?;
        let header = *anim.header();

        // 先算位置：贴主屏工作区（已排除任务栏）底部居中。
        let (left, _, right, bottom) = feiyu_platform::primary_work_area()
            .context("取屏幕工作区失败，无法定位窗口")?;
        let w = header.width as i32;
        let h = header.height as i32;
        let x = (left + right) / 2 - w / 2;
        let y = bottom - h - BOTTOM_GAP;
        // 截屏模式下挪到屏幕中上部，避免被任务栏/其他窗口挡住取样点。
        let (x, y) = if shot { (x, (bottom - h) / 2) } else { (x, y) };

        let win = LayeredWindow::create(header.width as u32, header.height as u32, x, y, "飞鱼")
            .context("创建分层窗口失败")?;

        Ok(Self { win, anim, fps: header.fps, frame: 0, acc: 0.0, last: Instant::now(), drawn: 0 })
    }

    /// 按时间推进帧并提交。
    fn tick(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.acc += dt * self.fps;
        if self.acc < 1.0 {
            return Ok(());
        }
        let count = self.anim.frame_count().max(1);
        let step = self.acc.floor() as u32;
        self.frame = (self.frame + step) % count;
        self.acc -= step as f32;
        self.draw()
    }

    fn draw(&mut self) -> Result<()> {
        let header = *self.anim.header();
        let idx = self.frame % header.frame_count.max(1);
        let pixels = self.anim.frame(idx).with_context(|| format!("解第 {idx} 帧失败"))?;
        self.win
            .draw(header.width as u32, header.height as u32, pixels)
            .context("提交画面失败")?;
        self.drawn += 1;
        Ok(())
    }
}

fn run_pet(args: Args) -> Result<()> {
    // 第二次双击时安静退出：用户看不到任何提示，桌宠也不会变成两只。
    let Some(_instance) = SingleInstance::acquire(SingleInstance::DEFAULT_NAME)
        .context("单实例锁创建失败")?
    else {
        println!("已经有一只飞鱼在桌面上了。");
        return Ok(());
    };

    let dpi = feiyu_platform::enable_per_monitor_v2();
    if !dpi {
        eprintln!("提示：未能开启 DPI v2 感知，高缩放下画面可能发糊");
    }

    let shot = args.shot.clone();
    let mut pet = Pet::open(&args.anim, shot.is_some())?;
    pet.draw()?;
    pet.win.show();

    // 基准模式：拆开量「解一帧」与「提交一帧」，否则优化没有靶子。
    if let Some(n) = args.bench {
        let header = *pet.anim.header();
        let (mut decode_ns, mut draw_ns) = (0u128, 0u128);
        let started = Instant::now();
        for _ in 0..n {
            pet.frame = (pet.frame + 1) % pet.anim.frame_count().max(1);
            let idx = pet.frame;
            let t = Instant::now();
            let pixels = pet.anim.frame(idx)?;
            decode_ns += t.elapsed().as_nanos();
            let t = Instant::now();
            pet.win.draw(header.width as u32, header.height as u32, pixels)?;
            draw_ns += t.elapsed().as_nanos();
        }
        let total = started.elapsed().as_secs_f64() * 1000.0;
        let per = total / n as f64;
        println!(
            "基准：{n} 帧共 {total:.0}ms，单帧 {per:.3}ms（解码 {:.3}ms + 提交 {:.3}ms）→ 24fps 约 {:.1}% 单核",
            decode_ns as f64 / n as f64 / 1e6,
            draw_ns as f64 / n as f64 / 1e6,
            per * 24.0 / 10.0
        );
        return Ok(());
    }

    // 静止模式：只画一帧，然后用一个长周期定时器挂着，测「不动的底噪」。
    if args.r#static {
        pet.win.set_tick(1000);
        let mut ticks = 0u32;
        pump(|ev| {
            if ev == Event::Tick {
                ticks += 1;
                if ticks >= 3 {
                    quit();
                }
            }
        })?;
        println!("静止 3 秒后退出（共提交 {} 帧）", pet.drawn);
        return Ok(());
    }

    pet.win.set_tick(TICK_MS);

    // 截屏模式：累计约 0.5 秒（TICK 8ms → 62 次）后抓图退出。
    let mut ticks = 0u32;
    let mut captured = false;
    pump(|ev| match ev {
        Event::Tick => {
            if let Err(e) = pet.tick() {
                eprintln!("错误：{e:#}");
                quit();
                return;
            }
            if let Some(path) = &shot {
                ticks += 1;
                if ticks >= 62 {
                    match feiyu_platform::shot::capture_to_bmp(pet.win.hwnd(), path) {
                        Ok((w, h)) => println!("已截屏 {w}×{h} → {}", path.display()),
                        Err(e) => eprintln!("截屏失败：{e}"),
                    }
                    captured = true;
                    quit();
                }
            }
        }
        Event::MouseDown { .. } => {}
        Event::MouseUp { .. } => {}
        // TODO(P2)：右键弹原生菜单。当前先用它退出，免得只能去任务管理器杀。
        Event::RightClick { .. } => quit(),
        Event::MouseMove { .. } | Event::Moved { .. } => {}
        Event::Quit => {}
    })?;

    if shot.is_some() && !captured {
        bail!("窗口提前退出，没有截到图");
    }
    println!("退出（共提交 {} 帧）", pet.drawn);
    Ok(())
}
