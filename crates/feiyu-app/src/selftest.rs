//! 自检：把「窗口真的能逐像素透明、真的能点到、真的画出来了、真的会换姿势、
//! 真的能被拖起来又落回地面」变成可执行断言。
//!
//! 全部走真实 Win32 路径（真建窗口、真提交位图、真从屏幕上抓像素），
//! 不做 mock——mock 过的窗口测不出 DWM 合成问题。
//!
//! 素材（`assets/anim/`）不入库，缺失时相关项标记为「跳过」而不是失败，
//! 否则新克隆的仓库跑自检会永远红。

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use feiyu_core::{Limits, Mood, Motion};
use feiyu_platform::{pump, quit, Event, LayeredWindow, SingleInstance};

use crate::pet::Pet;

const W: u32 = 200;
const H: u32 = 240;
const WIN_X: i32 = 400;
const WIN_Y: i32 = 300;

/// 素材目录。
const ANIM_DIR: &str = "assets/anim";

/// 脚底距工作区底边的空隙，必须与 `pet.rs` 的 `BOTTOM_GAP` 一致。
const BOTTOM_GAP: i32 = 4;

/// 逐个执行断言，返回是否全部通过。
pub fn run() -> Result<bool> {
    let mut r = Report::default();
    println!("飞鱼自检\n");

    check_window(&mut r)?;
    check_assets(&mut r)?;
    check_animation_switch(&mut r)?;
    check_interaction(&mut r)?;

    println!("\n{}", r.summary());
    Ok(r.failed == 0)
}

/// 窗口层：透明、命中、定位、定时器、截屏。
fn check_window(r: &mut Report) -> Result<()> {
    r.check(
        "DPI 每显示器 v2 感知",
        feiyu_platform::enable_per_monitor_v2(),
    );

    // 单实例：同名的第二次获取必须被拒。
    let first = SingleInstance::acquire("Local\\FeiYuPet.Selftest").context("创建互斥体失败")?;
    r.check("首次获取单实例锁", first.is_some());
    let second = SingleInstance::acquire("Local\\FeiYuPet.Selftest").context("创建互斥体失败")?;
    r.check("第二次获取被拒（防多开）", second.is_none());
    drop(second);
    drop(first);

    // 画一张测试图：左上 100×100 实心方块，其余全透明。
    let mut win = LayeredWindow::create(W, H, WIN_X, WIN_Y, "飞鱼自检").context("建窗口失败")?;
    r.check("窗口带 WS_EX_LAYERED", win.is_layered());

    let mut px = vec![0u8; (W * H * 4) as usize];
    for y in 20..120usize {
        for x in 20..120usize {
            let o = (y * W as usize + x) * 4;
            // 预乘 BGRA：不透明像素各通道值即颜色本身。这里给一个高饱和蓝。
            px[o] = 220; // B
            px[o + 1] = 40; // G
            px[o + 2] = 40; // R
            px[o + 3] = 255; // A
        }
    }
    win.draw(W, H, &px).context("提交测试位图失败")?;

    r.check("不透明像素会抢鼠标", win.hit_test(60, 60));
    r.check("透明像素穿透（不抢鼠标）", !win.hit_test(5, 5));
    r.check("窗口外不抢鼠标", !win.hit_test(-10, 60));
    r.check(
        &format!("窗口尺寸 = 位图尺寸（{:?}）", win.rect()),
        win.rect() == (WIN_X, WIN_Y, WIN_X + W as i32, WIN_Y + H as i32),
    );

    win.move_to(500, 350);
    r.check(
        &format!("移动后矩形跟随（{:?}）", win.rect()),
        win.rect() == (500, 350, 500 + W as i32, 350 + H as i32),
    );

    // 定时器精度：20ms × 10 次。
    win.show();
    win.set_tick(20);
    let started = Instant::now();
    let mut ticks = 0u32;
    pump(|ev| {
        if ev == Event::Tick {
            ticks += 1;
            if ticks >= 10 {
                quit();
            }
        }
    })?;
    let elapsed = started.elapsed().as_millis();
    win.set_tick(0);
    // 注意：Win32 定时器精度只有 ~15.6ms（未调 timeBeginPeriod 时），
    // 所以 20ms 的定时器实际每拍 ~31ms。这里只验证「定时器在跑且不偏得离谱」，
    // 真正的帧节拍靠主循环累加壁钟时间，不依赖定时器精度。
    r.check(
        &format!("定时器在跑：20ms×10 实测 {elapsed}ms（系统粒度 ~15.6ms，倍数为正常）"),
        (150..=800).contains(&elapsed),
    );

    // 截图取证：内容必须真的上屏。
    fs::create_dir_all("target").context("建 target 目录失败")?;
    let shot = "target/selftest_win.bmp";
    let (sw, sh) = feiyu_platform::shot::capture_to_bmp(win.hwnd(), shot).context("截屏失败")?;
    r.check(&format!("截图尺寸 {sw}×{sh}"), sw == W && sh == H);

    let data = fs::read(shot).context("读截图失败")?;
    let off = 54 + ((60 * W as usize + 60) * 4);
    let (b, g, red) = (data[off], data[off + 1], data[off + 2]);
    r.check(
        &format!("截图取样点（60,60）为方块蓝色 BGR={b},{g},{red}"),
        b > 150 && red < 100 && g < 100,
    );
    let off_t = 54 + ((10 * W as usize + 10) * 4);
    let (tb, tg, tr) = (data[off_t], data[off_t + 1], data[off_t + 2]);
    r.check(
        &format!("截图取样点（10,10）未画出方块 BGR={tb},{tg},{tr}"),
        !(tb > 150 && tr < 100 && tg < 100),
    );

    // 托盘图标：从一帧像素真的造出一个 HICON。
    // 「像素拼得对」由 `icon.rs` 的纯函数单测钉住，这里只验 Win32 这层能通。
    let mut icon_px = vec![0u8; (32 * 32 * 4) as usize];
    for y in 8..24usize {
        for x in 8..24usize {
            let o = (y * 32 + x) * 4;
            icon_px[o] = 200; // B
            icon_px[o + 1] = 80; // G
            icon_px[o + 2] = 40; // R
            icon_px[o + 3] = 255; // A
        }
    }
    let icon = feiyu_platform::Icon::from_bgra_sized(&icon_px, 32, 32, 16);
    let icon_desc = match &icon {
        Ok(i) => format!("句柄空 = {}", i.handle().0.is_null()),
        Err(e) => format!("失败：{e:#}"),
    };
    r.check(
        &format!("从帧像素能造出托盘图标（{icon_desc}）"),
        icon.as_ref().is_ok_and(|i| !i.handle().0.is_null()),
    );
    drop(icon);

    drop(win);
    Ok(())
}

/// 素材层：清单能解析、字段自洽、文件都在。
fn check_assets(r: &mut Report) -> Result<()> {
    let dir = Path::new(ANIM_DIR);
    let path = dir.join("manifest.json");
    if !path.exists() {
        r.skip(&format!(
            "素材清单 {} 不存在（先跑 pack-anim）",
            path.display()
        ));
        return Ok(());
    }
    let manifest = feiyu_core::anim::Manifest::load(&path)?;
    r.check(
        &format!("清单版本 {} 受支持", manifest.format),
        manifest.format == 1,
    );
    r.check(
        &format!("清单有 {} 段动画", manifest.animations.len()),
        manifest.animations.len() >= 5,
    );

    let missing: Vec<&str> = manifest
        .animations
        .iter()
        .filter(|a| !a.path_in(dir).exists())
        .map(|a| a.id.as_str())
        .collect();
    r.check(
        &format!("清单里每个文件都存在（缺 {:?}）", missing),
        missing.is_empty(),
    );

    // 脚底行必须落在画布内，且各段的脚底行应大致一致（同一角色同一画布）。
    let feet: Vec<i32> = manifest
        .animations
        .iter()
        .map(|a| a.feet_row() as i32)
        .collect();
    let spread = feet.iter().max().unwrap_or(&0) - feet.iter().min().unwrap_or(&0);
    r.check(
        &format!("各段落脚行差异 {spread} 像素（≤16 视为同一条地面）"),
        spread <= 16,
    );
    Ok(())
}

/// 运行时层：真的能换动画，且换的时候尺寸变、脚底不跳。
fn check_animation_switch(r: &mut Report) -> Result<()> {
    let dir = Path::new(ANIM_DIR);
    if !dir.join("manifest.json").exists() {
        r.skip("素材缺失，跳过运行时动画切换检查");
        return Ok(());
    }
    let Ok((left, _, right, bottom)) = feiyu_platform::primary_work_area() else {
        r.skip("取屏幕工作区失败，跳过动画切换检查");
        return Ok(());
    };
    let feet_line = bottom - BOTTOM_GAP;

    // 强制模式：状态机不参与，这样才能稳定断言尺寸。
    let mut pet = Pet::open(dir, Some("idle")).context("打开桌宠失败")?;
    r.check("强制模式下状态机不参与", pet.mood().is_none());

    let idle = pet.rect();
    let (w0, h0) = (idle.2 - idle.0, idle.3 - idle.1);
    r.check(
        &format!("idle 窗口 {w0}×{h0} 在屏幕内"),
        w0 > 0 && h0 > 0 && idle.0 >= left && idle.2 <= right,
    );
    r.check(
        &format!("idle 脚底贴地面：窗口底 {} / 目标 {}", idle.3, feet_line),
        idle.3 == feet_line,
    );

    // 切到最宽的一段，尺寸必然变化。
    pet.play("sleep").context("切到 sleep 失败")?;
    let sleep = pet.rect();
    let (w1, h1) = (sleep.2 - sleep.0, sleep.3 - sleep.1);
    r.check(
        &format!("切到 sleep 尺寸跟着变：{w0}×{h0} → {w1}×{h1}"),
        (w1, h1) != (w0, h0),
    );
    r.check(
        &format!(
            "换尺寸后脚底没跳（窗口底 {} / 目标 {}）",
            sleep.3, feet_line
        ),
        sleep.3 == feet_line,
    );

    // 再切到最窄的一段，横向锚定也不能飘。
    pet.play("stretch").context("切到 stretch 失败")?;
    let stretch = pet.rect();
    r.check(
        &format!("再切 stretch 脚底仍贴地（窗口底 {}）", stretch.3),
        stretch.3 == feet_line,
    );
    let (cw0, cw1) = ((idle.0 + idle.2) / 2, (stretch.0 + stretch.2) / 2);
    r.check(
        &format!("水平中心漂移 {} 像素（≤48 视为稳定）", (cw0 - cw1).abs()),
        (cw0 - cw1).abs() <= 48,
    );

    // 换段之后还得能正常解帧提交。
    let before = pet.drawn();
    pet.draw_frame().context("换段后提交失败")?;
    r.check(
        &format!("换段后仍能提交（{} → {} 帧）", before, pet.drawn()),
        pet.drawn() == before + 1,
    );

    // 清单与文件不一致时必须报错，而不是错位着播下去。
    let bad = Pet::open(dir, Some("definitely_not_an_anim")).err();
    r.check(
        &format!(
            "清单里不存在的 id 会被拒（{:?}）",
            bad.as_ref().map(|e| format!("{e:#}"))
        ),
        bad.is_some(),
    );

    drop(pet);
    Ok(())
}

/// 交互层：菜单能建、锚点边界与工作区自洽、能落回地面、点击与拖拽能区分。
fn check_interaction(r: &mut Report) -> Result<()> {
    let dir = Path::new(ANIM_DIR);
    let manifest_path = dir.join("manifest.json");
    if !manifest_path.exists() {
        r.skip("素材缺失，跳过交互检查");
        return Ok(());
    }
    let manifest = feiyu_core::anim::Manifest::load(&manifest_path)?;

    // ---- 几何：锚点范围必须与工作区、窗口尺寸自洽 ----
    let mut pet = Pet::open(dir, Some("idle")).context("打开桌宠失败")?;

    // 菜单：只核对内容没被写坏，不真的弹出来（模态菜单没法自动关），
    // 外观请用 `--menu` 看。
    let items = pet.menu_items();
    let items_desc = match &items {
        Ok(n) => format!("{n} 项"),
        Err(e) => format!("失败：{e:#}"),
    };
    r.check(
        &format!(
            "右键菜单能构造（{items_desc}，首项「{}」）",
            pet.hide_label()
        ),
        items.as_ref().is_ok_and(|n| *n == 5),
    );

    // ---- 隐藏 / 唤出：窗口真的会不可见，菜单文案跟着换 ----
    pet.show();
    let shown = pet.is_visible();
    pet.hide();
    let after_hide = pet.is_visible();
    let label_hidden = pet.hide_label().to_owned();
    let hidden_flag = pet.hidden();
    pet.unhide();
    let after_unhide = pet.is_visible();
    r.check(
        &format!("隐藏 → 不可见（{shown} → {after_hide}），唤出 → 又可见（{after_unhide}）"),
        shown && !after_hide && after_unhide && hidden_flag,
    );
    r.check(
        &format!("藏着时菜单首项变「{label_hidden}」"),
        label_hidden == "唤出桌宠",
    );
    r.check(
        &format!("唤出后首项变回「{}」", pet.hide_label()),
        pet.hide_label() == "隐藏到托盘",
    );

    let area = pet.area();
    let ground = (area.3 - BOTTOM_GAP) as f32;
    let limits = pet.limits();
    let (l, _, rr, b) = pet.rect();
    r.check(
        &format!("初始脚底贴地（窗口底 {b} / 地面 {}）", ground as i32),
        b == ground as i32,
    );
    r.check(
        &format!("锚点上界=地面（{:.1} / {ground:.1}）", limits.y.1),
        (limits.y.1 - ground).abs() < 0.5,
    );
    let span = limits.x.1 - limits.x.0;
    let expect = (area.2 - area.0 - (rr - l)) as f32;
    r.check(
        &format!("横向可动范围 {span:.1} = 工作区宽-窗口宽 {expect:.1}"),
        (span - expect).abs() <= 1.0,
    );

    // ---- 运动学：用真实边界跑一遍，确保落得下去也停得下来 ----
    let (landed, frames) = settle(Motion::fling(0.0, 0.0), (limits.x.0, limits.y.0), limits);
    let landed_desc = match landed {
        Some(p) => format!("{frames} 帧后 y={:.1}", p.1),
        None => "900 帧仍未落地".to_owned(),
    };
    r.check(
        &format!("从顶上自由落体：{landed_desc}"),
        landed.is_some_and(|p| (p.1 - limits.y.1).abs() < 0.5),
    );

    let (stopped, frames) = settle(
        Motion::fling(2000.0, 0.0),
        (limits.x.0 + 50.0, limits.y.1),
        limits,
    );
    r.check(
        &format!("横向快甩 {} 秒内停稳", frames as f32 / 60.0),
        stopped.is_some_and(|p| p.0 >= limits.x.0 - 0.5 && p.0 <= limits.x.1 + 0.5),
    );

    // ---- 回到底部：窗口左边缘必须落在「画布中心 - 半画布 + origin_x」上 ----
    pet.play("stretch").context("切到 stretch 失败")?;
    pet.back_to_home()?;
    let entry = manifest.require("stretch")?.clone();
    let expect_x =
        (area.0 + area.2) / 2 - manifest.canvas.width as i32 / 2 + entry.origin[0] as i32;
    let (hl, _, hr, hb) = pet.rect();
    r.check(
        &format!(
            "回到底部：底 {hb} / 地面 {}，左 {hl} / 期望 {expect_x}",
            ground as i32
        ),
        hb == ground as i32 && (hl - expect_x).abs() <= 1 && (hr - hl) == entry.width() as i32,
    );
    r.check("窗口默认为总在最前", pet.topmost());
    drop(pet);

    // ---- 点击 / 拖拽的区分：直接喂事件，走的是运行时真路径 ----
    let mut pet = Pet::open(dir, None).context("打开桌宠失败（自由模式）")?;

    // 1 像素抖动：必须仍然算「点击」，否则手的微小位移会把点击吞掉。
    pet.on_event(Event::MouseDown { x: 10, y: 10 })?;
    let (jl, jt, _, _) = pet.rect();
    pet.on_event(Event::Moved {
        x: jl + 1,
        y: jt - 1,
    })?;
    r.check(
        &format!("1 像素抖动不算拖拽（{:?}）", pet.mood()),
        pet.mood() != Some(Mood::Dragged),
    );
    pet.on_event(Event::MouseUp { x: 10, y: 10 })?;
    r.check(
        &format!("抖动后松手仍算点击（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Happy),
    );

    // 移出阈值：进入被拖拽姿态；松手回到待机。
    pet.on_event(Event::MouseDown { x: 10, y: 10 })?;
    let (ml, mt, _, _) = pet.rect();
    pet.on_event(Event::Moved {
        x: ml + 40,
        y: mt + 24,
    })?;
    r.check(
        &format!("位移超过阈值进入拖拽姿态（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Dragged),
    );
    pet.on_event(Event::MouseUp { x: 10, y: 10 })?;
    r.check(
        &format!("松手回到待机（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Idle),
    );

    // 已贴地且零速度：一拍内必须停稳，窗口不能再溜。
    let before = pet.rect();
    pet.tick()?;
    r.check(
        &format!("贴地松手后一拍内停稳（{:?} → {:?}）", before, pet.rect()),
        pet.rect() == before,
    );

    drop(pet);
    Ok(())
}

/// 一直推进到停稳，返回 `(最终锚点, 帧数)`；没停则第一个元素为 `None`。
fn settle(mut m: Motion, mut anchor: (f32, f32), limits: Limits) -> (Option<(f32, f32)>, usize) {
    for i in 0..900 {
        let (next, rest) = m.step(anchor, limits, 1.0 / 60.0);
        anchor = next;
        if rest {
            return (Some(anchor), i + 1);
        }
    }
    (None, 900)
}

#[derive(Default)]
struct Report {
    passed: u32,
    failed: u32,
    skipped: u32,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool) {
        if ok {
            self.passed += 1;
            println!("  ✓ {name}");
        } else {
            self.failed += 1;
            println!("  ✗ {name}");
        }
    }

    fn skip(&mut self, name: &str) {
        self.skipped += 1;
        println!("  - {name}（跳过）");
    }

    fn summary(&self) -> String {
        let tail = if self.failed == 0 {
            "，全部通过"
        } else {
            "，存在失败项"
        };
        let skipped = if self.skipped == 0 {
            String::new()
        } else {
            format!("，跳过 {}", self.skipped)
        };
        format!(
            "自检结果：{} 通过 / {} 失败{skipped}{tail}",
            self.passed, self.failed
        )
    }
}
