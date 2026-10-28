//! 桌宠主程序。
//!
//! P2 起动画由状态机决定，不再写死 `idle`。运行模式见 `--help`：
//!
//! * 无参数 —— 正常当桌宠跑；
//! * `--selftest` —— 无人值守自检，逐项打印通过/失败，退出码即结论；
//! * `--list` —— 打印素材清单（不建窗口）；
//! * `--shot <文件>` —— 跑 0.5 秒后截一张；
//! * `--shots <目录>` —— 每段动画各截一张，用于肉眼验收全部素材；
//! * `--menu` —— 弹一次右键菜单后退出，用来确认菜单在真机上长什么样；
//! * `--tray` —— 挂上托盘图标跑 5 秒后退出，用来确认通知区里真有一只看得到；
//! * `--bench <n>` / `--static` —— 性能与静止底噪测量。

mod pet;
mod selftest;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use feiyu_core::anim::Manifest;
use feiyu_platform::{pump, quit, Event, SingleInstance};

use crate::pet::{Action, Pet};

/// 素材目录默认值（相对当前工作目录）。
const DEFAULT_DIR: &str = "assets/anim";

/// 计时器周期。8ms 足够驱动 24fps，空闲时不至于空转太凶。
const TICK_MS: u32 = 8;

/// `--shot` 截屏前跑多少个 tick（8ms × 62 ≈ 0.5 秒）。
const SHOT_TICKS: u32 = 62;

/// `--shots` 每段动画跑多少个 tick 再截（够推进 2~3 帧）。
const SHOT_PER_ANIM: u32 = 10;

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
    let args = parse_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let dir = args
        .dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR));

    if args.selftest {
        let ok = selftest::run()?;
        return if ok {
            Ok(())
        } else {
            bail!("自检未全部通过")
        };
    }
    if args.list {
        return list_anims(&dir);
    }
    if let Some((id, out_dir, step)) = &args.dump {
        return dump_frames(&dir, id, out_dir, *step);
    }
    run_pet(args, dir)
}

struct Args {
    dir: Option<PathBuf>,
    /// 强制只播这一段（忽略状态机）。
    anim: Option<String>,
    shot: Option<PathBuf>,
    shots: Option<PathBuf>,
    /// 只画一帧后挂着不动，用于测量静止时的 CPU 底噪。
    r#static: bool,
    /// 基准模式：连续提交 n 帧并报告单帧耗时。
    bench: Option<u32>,
    /// 弹一次右键菜单就退出（验证菜单用）。
    menu: bool,
    /// 挂上托盘图标跑 5 秒就退出（验证通知区用）。
    tray: bool,
    /// 一开始就藏进托盘，挂 n 秒后退出（量隐藏时的 CPU）。
    hide: Option<u32>,
    /// 一启动就藏进托盘，但**消息泵照常跑**：验证「隐藏后托盘能不能点开」。
    /// `--hide` 不跑泵（那是量 CPU 用的），验不了点击。
    hide_now: bool,
    /// 跑起来 n 秒之后再藏进托盘，**且不补定时器**——这才是菜单里点
    /// 「隐藏到托盘」走的那条路径（`--hide-now` 会补一个 500ms 拍子，
    /// 两者行为不完全一样，托盘问题必须用这条验）。
    hide_after: Option<u32>,
    /// 逐段演示：每段播几秒，并把「该看到什么」打出来给人对照。
    showcase: bool,
    /// 帧率覆写（素材是 24fps；调低显得更像手绘，调高更跟手）。
    fps: Option<f32>,
    /// 正常跑 n 秒就优雅退出。观察随机动作、入睡这类「要等才会发生」的行为时用。
    run: Option<u32>,
    /// 把入睡等待钉成固定秒数（默认随机 18~38 秒）。
    /// 观察「睡着后被键鼠弄醒」必须能控住这个数。
    sleep_after: Option<f32>,
    /// 启动时给一个初速度（像素/秒），用来验证滑行与下落。
    fling: Option<(f32, f32)>,
    /// 把某段动画按步长导出成 BMP（判读素材内容用）。
    dump: Option<(String, PathBuf, u32)>,
    list: bool,
    selftest: bool,
    /// 打印动画切换日志。
    verbose: bool,
}

fn parse_args(argv: &[String]) -> Result<Args> {
    let mut out = Args {
        dir: None,
        anim: None,
        shot: None,
        shots: None,
        r#static: false,
        bench: None,
        menu: false,
        tray: false,
        hide: None,
        hide_now: false,
        hide_after: None,
        showcase: false,
        fps: None,
        run: None,
        sleep_after: None,
        fling: None,
        dump: None,
        list: false,
        selftest: false,
        verbose: false,
    };
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dir" => out.dir = Some(PathBuf::from(it.next().context("--dir 缺少路径")?)),
            "--anim" => out.anim = Some(it.next().context("--anim 缺少 id")?.clone()),
            "--shot" => out.shot = Some(PathBuf::from(it.next().context("--shot 缺少路径")?)),
            "--shots" => out.shots = Some(PathBuf::from(it.next().context("--shots 缺少目录")?)),
            "--static" => out.r#static = true,
            "--bench" => {
                out.bench = Some(
                    it.next()
                        .context("--bench 缺少帧数")?
                        .parse()
                        .context("--bench 需要整数")?,
                )
            }
            "--menu" => out.menu = true,
            "--tray" => out.tray = true,
            "--hide-now" => out.hide_now = true,
            "--hide-after" => {
                out.hide_after = Some(
                    it.next()
                        .context("--hide-after 缺少数秒")?
                        .parse()
                        .context("--hide-after 需要整数秒")?,
                )
            }
            "--showcase" => out.showcase = true,
            "--fps" => {
                out.fps = Some(
                    it.next()
                        .context("--fps 缺少数值")?
                        .parse()
                        .context("--fps 需要数字")?,
                )
            }
            "--hide" => {
                out.hide = Some(
                    it.next()
                        .context("--hide 缺少数秒")?
                        .parse()
                        .context("--hide 需要整数秒")?,
                )
            }
            "--fling" => {
                let vx = it
                    .next()
                    .context("--fling 缺少 vx")?
                    .parse()
                    .context("--fling 的 vx 需要数字")?;
                let vy = it
                    .next()
                    .context("--fling 缺少 vy")?
                    .parse()
                    .context("--fling 的 vy 需要数字")?;
                out.fling = Some((vx, vy));
            }
            "--dump" => {
                let id = it.next().context("--dump 缺少动画 id")?.clone();
                let out_dir = PathBuf::from(it.next().context("--dump 缺少输出目录")?);
                let step = it
                    .next()
                    .context("--dump 缺少帧步长")?
                    .parse()
                    .context("--dump 步长需要整数")?;
                out.dump = Some((id, out_dir, step));
            }
            "--sleep-after" => {
                out.sleep_after = Some(
                    it.next()
                        .context("--sleep-after 缺少数秒")?
                        .parse()
                        .context("--sleep-after 需要数字秒")?,
                )
            }
            "--list" => out.list = true,
            "--run" => {
                out.run = Some(
                    it.next()
                        .context("--run 缺少数秒")?
                        .parse()
                        .context("--run 需要整数秒")?,
                )
            }
            "--selftest" => out.selftest = true,
            "--verbose" | "-v" => out.verbose = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("未知参数 {other}（--help 查看用法）"),
        }
    }
    Ok(out)
}

fn print_help() {
    println!(
        "feiyu —— 桌宠\n\n\
         \x20 --dir <目录>      素材目录（含 manifest.json，默认 {DEFAULT_DIR}）\n\
         \x20 --anim <id>       强制只播这一段，忽略状态机\n\
         \x20 --list           打印素材清单\n\
         \x20 --dump <id> <目录> <步长>  把该段动画每步长帧导一张白底 BMP（判读素材内容用）\n\
         \x20 --shot <文件>     跑 0.5 秒后截屏存 BMP 并退出\n\
         \x20 --shots <目录>    每段动画各截一张 BMP 后退出\n\
         \x20 --menu           弹一次右键菜单后退出（验证菜单用）\n\
         \x20 --tray           挂上托盘图标跑 5 秒后退出（验证通知区用）\n\
         \x20 --hide <秒>      先跑 2 秒让动画推进，再藏进托盘挂 n 秒（量隐藏时的 CPU，不跑泵）\n\
         \x20 --hide-now       一启动就藏进托盘但照常跑泵（验证隐藏后托盘能不能点开）\n\
         \x20 --hide-after <秒> 跑 n 秒后从泵内部藏进托盘且不补定时器（等价于菜单里点隐藏）\n\
         \x20 --showcase       逐段演示：每段播几秒，并把「该看到什么」打出来\n\
         \x20 --fps <数值>     覆写帧率（默认用清单里的 24）\n\
         \x20 --run <秒>       正常跑 n 秒后优雅退出（观察随机动作、入睡这类慢行为）\n\
         \x20 --sleep-after <秒> 把入睡等待钉成固定值（默认随机 18~38 秒）\n\
         \x20 --fling <vx> <vy> 启动时给一个初速度（像素/秒，验证滑行与下落）\n\
         \x20 --static         画一帧后不再推进（测静止底噪）\n\
         \x20 --bench <n>      连续提交 n 帧，报告单帧耗时\n\
         \x20 --verbose        打印动画切换\n\
         \x20 --selftest       跑自检并打印逐项结果"
    );
}

/// 打印素材清单（不需要窗口，可在任何终端里跑）。
fn list_anims(dir: &Path) -> Result<()> {
    let manifest = Manifest::load(dir.join("manifest.json"))
        .with_context(|| format!("读不了素材清单 {}", dir.join("manifest.json").display()))?;
    println!(
        "画布 {}×{}，帧率 {}，共 {} 段动画\n",
        manifest.canvas.width,
        manifest.canvas.height,
        manifest.fps,
        manifest.animations.len()
    );
    println!(
        "{:<12} {:<9} {:>10} {:>7} {:>8}  {}",
        "id", "帧尺寸", "落脚行", "帧数", "体积", "名称"
    );
    for a in &manifest.animations {
        println!(
            "{:<12} {:<9} {:>10} {:>7} {:>7.1}M  {}",
            a.id,
            format!("{}×{}", a.width(), a.height()),
            a.feet_row(),
            a.frames,
            a.bytes as f64 / 1e6,
            a.name
        );
    }
    Ok(())
}

/// `--dump`：把某段动画按步长导出成 BMP。
///
/// 存在的理由很具体：**素材里到底有什么，只能看帧才知道**。
/// 判断「睡觉动画哪一段是 Z」「拖拽开头是不是静止」，靠读代码或读时长
/// 都只能猜；导成图以后一眼就能定。
fn dump_frames(dir: &Path, id: &str, out_dir: &Path, step: u32) -> Result<()> {
    let manifest = Manifest::load(dir.join("manifest.json"))
        .with_context(|| format!("读不了素材清单 {}", dir.join("manifest.json").display()))?;
    let entry = manifest.require(id)?.clone();
    let (w, h) = (entry.width() as u32, entry.height() as u32);
    let src = dir.join(&entry.file);
    let mut anim = feiyu_core::anim::AnimReader::open(&src)
        .with_context(|| format!("打不开 {}", src.display()))?;
    let count = anim.frame_count();
    std::fs::create_dir_all(out_dir)?;
    let step = step.max(1);
    let started = std::time::Instant::now();
    let mut written = 0u32;
    let mut idx = 0u32;
    while idx < count {
        let pixels = anim.frame(idx)?;
        let flat = flatten_on_white(pixels);
        // 文件名用**连续序号**而不是帧号：ffmpeg 的 `%04d` 序列号模式靠连续编号找图，
        // 而步长导出出来的帧号是 0/6/12…，不连续就拼不了图。
        let path = out_dir.join(format!("{id}_{written:04}.bmp"));
        feiyu_platform::shot::write_bmp(&path, w, h, &flat)?;
        written += 1;
        idx += step;
    }
    let ms = started.elapsed().as_secs_f64() * 1e3;
    println!(
        "{id}：{w}×{h}，共 {count} 帧（{:.2} 秒 @{}fps），按步长 {step} 导出 {written} 张到 {}（第 n 张 = 第 n×{step} 帧），耗时 {ms:.0}ms",
        f64::from(count) / f64::from(manifest.fps),
        manifest.fps,
        out_dir.display()
    );
    Ok(())
}

/// 预乘 BGRA → 不透明 BGRA（合成到白底）。
///
/// 直接写预乘像素的话，透明处的值是 0，看图软件又忽略 alpha，
/// 得到的就是「一张全黑、只有轮廓略亮」的图——正好把要看的东西藏了。
fn flatten_on_white(src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = (*c as u32 + 255 - a).min(255) as u8;
        }
        px[3] = 255;
    }
    out
}

fn run_pet(args: Args, dir: PathBuf) -> Result<()> {
    // 第二次双击时安静退出：用户看不到任何提示，桌宠也不会变成两只。
    let Some(_instance) =
        SingleInstance::acquire(SingleInstance::DEFAULT_NAME).context("单实例锁创建失败")?
    else {
        // 但也不能真的什么都不做：双击了就说明用户想看到它。
        // 给它发一个自定义消息，让它自己从托盘里出来。
        match feiyu_platform::find_window() {
            Some(hwnd) => match feiyu_platform::wake(hwnd) {
                Ok(()) => println!("已经有一只飞鱼在桌面上了，已把它唤出来。"),
                Err(e) => println!("已经有一只飞鱼在桌面上了（唤醒失败：{e}）。"),
            },
            None => println!("已经有一只飞鱼在桌面上了。"),
        }
        return Ok(());
    };

    if !feiyu_platform::enable_per_monitor_v2() {
        eprintln!("提示：未能开启 DPI v2 感知，高缩放下画面可能发糊");
    }

    // `--shots` 也要冻结状态机，否则截图过程里桌宠自己睡着，截到一半换段。
    // `--showcase` 同理：逐段演示时不能被随机动作/入睡抢走。
    let forced = args
        .anim
        .clone()
        .or_else(|| args.shots.as_ref().map(|_| "idle".to_owned()))
        .or_else(|| args.showcase.then(|| "idle".to_owned()));
    let mut pet = Pet::open(&dir, forced.as_deref())?;
    pet.show();
    if let Some(fps) = args.fps {
        pet.set_fps(fps);
    }
    if args.showcase {
        return showcase(&mut pet);
    }
    if let Some(secs) = args.sleep_after {
        pet.set_fixed_sleep_after(secs);
    }

    // 调试入口：把「要鼠标才能触发」那一步拆掉，让物理推窗口可以被截屏取证。
    if let Some((vx, vy)) = args.fling {
        pet.start_fling(vx, vy);
    }

    if let Some(n) = args.bench {
        let (dec, drw) = pet.bench(n)?;
        println!(
            "基准：{n} 帧，单帧 {:.3}ms（解码 {dec:.3} + 提交 {drw:.3}）→ {}fps 约 {:.1}% 单核",
            dec + drw,
            pet.fps(),
            (dec + drw) * pet.fps() as f64 / 10.0
        );
        return Ok(());
    }

    if args.r#static {
        return still(&mut pet);
    }

    if args.menu {
        return menu_once(&mut pet);
    }

    if args.tray {
        return tray_demo(&mut pet);
    }

    if let Some(secs) = args.hide {
        return hide_demo(&mut pet, secs);
    }

    if let Some(out) = &args.shots {
        let out = out.clone();
        return shoot_all(&mut pet, &out);
    }

    // 托盘只在真要常驻时才挂：`--shot` 一闪就退，加了又删会让通知区闪一下。
    if args.shot.is_none() {
        pet.enable_tray().context("挂托盘图标失败")?;
        if args.hide_now {
            pet.hide();
            // 真正的隐藏要停定时器（省 CPU），但这个模式的用途恰恰是
            // 「隐藏后还能不能点开」——没有消息就没有 pump 回调，`--run` 的
            // 截止也就永远等不到。给一个 500ms 的慢速定时器：够粗到不耗电，
            // 又够细到能被截止检查看到。
            pet.set_tick(500);
            println!(
                "已藏进托盘（--hide-now）：窗口可见 = {}，托盘图标照常可点。",
                pet.is_visible()
            );
        }
    }

    // 隐藏模式已经自己设了 500ms 的慢速定时器，别拿 8ms 盖掉它。
    if !args.hide_now {
        pet.set_tick(TICK_MS);
    }
    let shot = args.shot.clone();
    let verbose = args.verbose;
    // `--run` 用墙钟截止，只能在泵里判：`SetTimer` 粒度 15.6ms，数拍子不准。
    let deadline = args
        .run
        .map(|s| std::time::Instant::now() + std::time::Duration::from_secs(u64::from(s)));
    let mut ticks = 0u32;
    let mut captured = false;
    let mut last = pet.current().to_owned();
    let mut failure: Option<anyhow::Error> = None;
    // `--hide-after`：跑一会儿之后从泵内部隐藏，行为等同于菜单里点「隐藏到托盘」
    // （`hide()` 会停定时器，而且不会有人再把它开回来）。
    let hide_after = args.hide_after;
    let started = std::time::Instant::now();
    let mut hid = false;

    pump(|ev| {
        // 取证开关：把每个事件原样打出来。「托盘点了没反应」这类问题，
        // 光看姿态日志分不出「消息没到」还是「到了但没反应」。
        if std::env::var_os("FEIYU_EVENT_TRACE").is_some() {
            println!("事件：{ev:?}");
        }
        if !hid && hide_after.is_some_and(|s| started.elapsed().as_secs() >= u64::from(s)) {
            hid = true;
            pet.hide();
            println!(
                "已藏进托盘（--hide-after，不补定时器）：窗口可见 = {}，藏着 = {}",
                pet.is_visible(),
                pet.hidden()
            );
        }
        if let Some(d) = deadline {
            if std::time::Instant::now() >= d {
                println!("到时间了，退出（共提交 {} 帧）", pet.drawn());
                quit();
                return;
            }
        }
        // 右键（桌宠身上或托盘图标上）弹同一张原生菜单。
        // 菜单是模态的：它内部会跑自己的消息循环，期间窗口定时器照常触发，
        // 所以动画不会卡在选中前那一帧。
        if menu_event(ev, &mut pet, verbose) {
            return;
        }
        // 托盘左键单击 / 又启动了一次：两种意思都是「把这一只叫出来」。
        if matches!(ev, Event::TrayLeftClick | Event::Woken) {
            pet.unhide();
            if verbose {
                println!("唤出（可见 = {}）", pet.is_visible());
            }
            return;
        }
        if let Err(e) = pet.on_event(ev) {
            eprintln!("错误：{e:#}");
            failure = Some(e);
            quit();
            return;
        }
        if verbose && pet.current() != last {
            last = pet.current().to_owned();
            println!("→ {}", last);
        }
        if let (Some(path), true) = (&shot, ev == Event::Tick) {
            ticks += 1;
            if ticks >= SHOT_TICKS {
                match pet.capture(path) {
                    Ok((w, h)) => {
                        println!("已截屏 {w}×{h}，窗口 {:?} → {}", pet.rect(), path.display())
                    }
                    Err(e) => eprintln!("截屏失败：{e:#}"),
                }
                captured = true;
                quit();
            }
        }
    })?;

    if let Some(e) = failure {
        return Err(e);
    }
    if shot.is_some() && !captured {
        bail!("窗口提前退出，没有截到图");
    }
    println!("退出（共提交 {} 帧）", pet.drawn());
    Ok(())
}

/// `--showcase`：逐段播一遍，并把「该看到什么」打出来。
///
/// 存在的理由很具体：素材合不合适、区间切得对不对、朝向对不对，
/// 只有人眼能定。每段播几秒（够看到一轮循环），然后换下一段。
fn showcase(pet: &mut Pet) -> Result<()> {
    const SHOWCASE_SECS: f32 = 4.0;
    let available: Vec<String> = pet.ids().map(str::to_owned).collect();
    println!(
        "逐段演示，每段 {SHOWCASE_SECS:.0} 秒（共 {} 段）",
        Pet::clip_table().len()
    );
    for (id, desc, span) in Pet::clip_table() {
        if !available.iter().any(|a| a == id) {
            continue;
        }
        println!("\n=== {id} ===");
        println!("  应该看到：{desc}");
        if let Some((lo, hi)) = span {
            println!("  播放区间：第 {lo}~{hi} 帧（跳过前摇/收势）");
        }
        pet.play(id)?;
        let started = std::time::Instant::now();
        while started.elapsed().as_secs_f32() < SHOWCASE_SECS {
            std::thread::sleep(std::time::Duration::from_millis(10));
            pet.tick()?;
        }
    }
    println!("\n演示完毕。");
    Ok(())
}

/// 弹一次右键菜单然后退出：用来确认菜单在真机上长什么样。
fn menu_once(pet: &mut Pet) -> Result<()> {
    let (left, top, _, _) = pet.rect();
    // 从窗口左上角往内偏一点弹，免得菜单正好盖住角色。
    let action = pet.menu(left + 12, top + 12)?;
    println!(
        "菜单结果：{action:?}；当前总在最前 = {}，藏着 = {}",
        if pet.topmost() { "开" } else { "关" },
        pet.hidden()
    );
    Ok(())
}

/// 处理「弹菜单」类事件；返回 `true` 表示这个事件已被消费。
///
/// 桌宠身上的右键和托盘图标上的右键走同一条路径：两者弹的是同一张菜单，
/// 分成两份代码只会让两边慢慢长歪。
fn menu_event(ev: Event, pet: &mut Pet, verbose: bool) -> bool {
    let (Event::RightClick { x, y } | Event::TrayRightClick { x, y }) = ev else {
        return false;
    };
    let started = std::time::Instant::now();
    let outcome = pet.menu(x, y);
    let waited = started.elapsed().as_millis();
    // 取证开关：菜单到底返回了什么、在里面待了多久。
    // 耗时≈ 0ms 且返回 None = 菜单根本没显示（被前台锁拒了）；
    // 耗时到几百 ms 才返回 None = 菜单显示了，是外面那一下点击把它关掉的。
    if std::env::var_os("FEIYU_EVENT_TRACE").is_some() {
        println!("菜单返回：{outcome:?}（耗时 {waited}ms）");
    }
    match outcome {
        Ok(Action::Quit) => quit(),
        Ok(Action::Topmost(on)) => {
            if verbose {
                println!("总在最前：{}", if on { "开" } else { "关" });
            }
        }
        Ok(Action::Hidden(on)) => {
            if verbose {
                println!(
                    "{}",
                    if on {
                        "已藏进托盘"
                    } else {
                        "已从托盘唤出"
                    }
                );
            }
        }
        Ok(Action::None | Action::Home) => {}
        Err(e) => eprintln!("菜单出错：{e:#}"),
    }
    true
}

/// 藏进托盘挂 n 秒：用来量「藏起来之后到底还烧不烧 CPU」。
///
/// 隐藏**之前先正常跑 2 秒**：一是让动画推进到非首帧（否则托盘图标和启动时那张
/// 一模一样，光看输出分不出「定格」有没有生效），二是更接近真实使用——没人会在
/// 第 0 帧就去点隐藏。
///
/// 隐藏之后刻意**不跑消息泵**：定时器已经停了，泵只是在 `GetMessageW` 上阻塞，
/// CPU 贡献可忽略，直接用 `sleep` 把时间耗掉更干净。代价是这段时间里的托盘
/// 点击不会响应——它只用于测量，不是给人用的日常模式。
fn hide_demo(pet: &mut Pet, secs: u32) -> Result<()> {
    // 按**墙钟**计时，不数拍子：`SetTimer` 粒度 15.6ms，数 48 拍以为是 2 秒，实际跑成 4 秒。
    const WARMUP_SECS: f32 = 2.0;
    pet.enable_tray().context("挂托盘图标失败")?;
    pet.set_tick(TICK_MS);
    let started = std::time::Instant::now();
    let mut hid = false;
    pump(|ev| {
        if !hid && ev == Event::Tick && started.elapsed().as_secs_f32() >= WARMUP_SECS {
            hid = true;
            // 先定格再收窗口：托盘里那个小东西就是它被收走时的样子。
            pet.hide();
            println!(
                "已藏进托盘：定格在第 {} 帧，窗口可见 = {}，藏着 = {}，定时器已停。挂 {secs} 秒后退出。",
                pet.frame(),
                pet.is_visible(),
                pet.hidden()
            );
            quit();
            return;
        }
        if let Err(e) = pet.on_event(ev) {
            eprintln!("错误：{e:#}");
            quit();
        }
    })?;
    if !hid {
        bail!("窗口提前退出，没来得及隐藏");
    }
    std::thread::sleep(std::time::Duration::from_secs(u64::from(secs)));
    println!("退出（共提交 {} 帧）", pet.drawn());
    Ok(())
}

/// 挂上托盘图标跑 5 秒：确认图标真的出现在通知区。
///
/// 自检能验「`HICON` 造得对」，但验不了「任务栏右下角到底长什么样」——
/// 那一步只能靠人的眼睛。
fn tray_demo(pet: &mut Pet) -> Result<()> {
    pet.enable_tray().context("挂托盘图标失败")?;
    pet.set_tick(TICK_MS);
    println!(
        "托盘图标已添加（悬停提示「飞鱼」）。\n\
         左键单击 → 把桌宠叫到同层最前；右键 → 菜单；5 秒后自动退出。"
    );
    // 按**墙钟**计时，不数拍子：`SetTimer` 的粒度是 ~15.6ms，
    // 数 625 拍以为是 5 秒，实际会跑成 10 秒。
    const DEMO_SECS: f32 = 5.0;
    let started = std::time::Instant::now();
    pump(|ev| {
        if menu_event(ev, pet, true) {
            return;
        }
        if matches!(ev, Event::TrayLeftClick | Event::Woken) {
            pet.unhide();
            println!("唤出（可见 = {}）", pet.is_visible());
            return;
        }
        if let Err(e) = pet.on_event(ev) {
            eprintln!("错误：{e:#}");
            quit();
            return;
        }
        if ev == Event::Tick && started.elapsed().as_secs_f32() >= DEMO_SECS {
            println!("窗口可见 = {}，藏着 = {}", pet.is_visible(), pet.hidden());
            quit();
        }
    })?;
    println!("退出（共提交 {} 帧）", pet.drawn());
    Ok(())
}

/// 静止模式：只画一帧，然后用一个长周期定时器挂着，测「不动的底噪」。
fn still(pet: &mut Pet) -> Result<()> {
    pet.set_tick(1000);
    let mut ticks = 0u32;
    pump(|ev| {
        if ev == Event::Tick {
            ticks += 1;
            if ticks >= 3 {
                quit();
            }
        }
    })?;
    println!("静止 3 秒后退出（共提交 {} 帧）", pet.drawn());
    Ok(())
}

/// 每段动画各截一张，用于肉眼验收全部素材。
fn shoot_all(pet: &mut Pet, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("建目录 {} 失败", dir.display()))?;
    let ids: Vec<String> = pet.ids().map(str::to_owned).collect();
    if ids.is_empty() {
        bail!("清单里没有动画");
    }
    pet.play(&ids[0])?;
    pet.set_tick(TICK_MS);

    let mut i = 0usize;
    let mut ticks = 0u32;
    pump(|ev| {
        if !matches!(ev, Event::Tick) {
            return;
        }
        if let Err(e) = pet.tick() {
            eprintln!("推送帧失败：{e:#}");
            quit();
            return;
        }
        ticks += 1;
        if ticks < SHOT_PER_ANIM {
            return;
        }
        ticks = 0;
        let path = dir.join(format!("{}.bmp", ids[i]));
        match pet.capture(&path) {
            Ok((w, h)) => println!("{:<12} {w}×{h} → {}", ids[i], path.display()),
            Err(e) => eprintln!("{:<12} 截屏失败：{e:#}", ids[i]),
        }
        i += 1;
        if i >= ids.len() {
            quit();
            return;
        }
        if let Err(e) = pet.play(&ids[i]) {
            eprintln!("切到 {} 失败：{e:#}", ids[i]);
            quit();
        }
    })?;
    println!(
        "共截 {} 段动画，用时约 {:.1} 秒",
        ids.len(),
        (ids.len() as f32) * (SHOT_PER_ANIM as f32) * 0.008
    );
    Ok(())
}
