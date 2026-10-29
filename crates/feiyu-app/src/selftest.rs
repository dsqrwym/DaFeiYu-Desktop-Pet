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

use crate::pet::{Pet, LANDING_HOLD};

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
        items.as_ref().is_ok_and(|n| *n == 7),
    );

    // ---- 托盘图标用「此刻那一帧」：必须真的跟着动画走 ----
    //
    // 这条不是走形式：如果 `enable_tray` 仍写死 `frame(0)`，下面两个条件
    // （帧号推进了、像素变了）至少有一个会失。
    let px_first = pet.current_frame_pixels()?;
    // 帧号由真实 dt 累积而来，得真的等一会儿才能推进（24fps 下 1 帧 = 42ms）。
    std::thread::sleep(std::time::Duration::from_millis(220));
    pet.on_event(Event::Tick)?;
    let frame_now = pet.frame();
    let px_now = pet.current_frame_pixels()?;
    r.check(
        &format!(
            "托盘图标取当前帧而非首帧（第 0 帧 vs 第 {frame_now} 帧，像素{}）",
            if px_first == px_now {
                "相同（错）"
            } else {
                "不同"
            }
        ),
        frame_now > 0 && px_first != px_now,
    );

    // ---- 隐藏 / 唤出：窗口真的会不可见，菜单文案跟着换 ----
    pet.show();
    let shown = pet.is_visible();
    // 藏的那一刻帧号必须已经离开首帧，否则「定格」和「写死首帧」看不出区别。
    let frame_at_hide = pet.frame();
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
    r.check(
        &format!("隐藏时定格在第 {frame_at_hide} 帧（非首帧才算真定格）"),
        frame_at_hide > 0,
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

    // 点击反应是**一次性**的：演完一圈就回待机。
    // 不收回的话 `click_happy` 会一直循环（用户：「被点击后的动画一直在重复」）。
    let click_anim = pet.current().to_owned();
    let went_back = pet.advance_frames(245)?;
    r.check(
        &format!(
            "{click_anim} 只演一圈就回待机（{}，心情 {:?}，回待机 = {went_back}）",
            pet.current(),
            pet.mood()
        ),
        went_back && pet.current() == "idle" && pet.mood() == Some(Mood::Idle),
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
    // 进入拖拽姿态那一刻就必须是「悬空」的样子：`drag` 素材前 20 帧人还站在地上，
    // 用户报的「拖一两秒才漂浮」就是它演出来的。
    let drag_frame = pet.frame();
    let drag_span = pet.loop_span();
    r.check(
        &format!("拖拽从悬空段第 {drag_frame} 帧起播（区间 {drag_span:?}）"),
        drag_span == Some((35, 110)) && (35..=110).contains(&drag_frame),
    );
    // 拿着不放 3 秒：不能漂回「站在地上」的开头，也不能演到落地那一段。
    pet.advance_frames(72)?;
    let held_frame = pet.frame();
    r.check(
        &format!("按住 3 秒仍在悬空段内（第 {held_frame} 帧）"),
        (35..=110).contains(&held_frame),
    );
    pet.on_event(Event::MouseUp { x: 10, y: 10 })?;
    r.check(
        &format!("松手后仍在空中保持拖拽姿态（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Dragged),
    );

    // 已贴地且零速度：一拍内必须停稳，窗口不能再溜。
    let before = pet.rect();
    pet.tick()?;
    r.check(
        &format!("贴地松手后一拍内停稳（{:?} → {:?}）", before, pet.rect()),
        pet.rect() == before,
    );

    // 停稳不等于立刻站起来：要再保持 `LANDING_HOLD` 秒才回待机。
    // 这一等必须真的等：`tick` 吃的是墙钟 dt，不能造假。
    std::thread::sleep(std::time::Duration::from_millis(120));
    pet.tick()?;
    r.check(
        &format!("落地 {LANDING_HOLD} 秒后才回到待机（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Idle),
    );

    // ---- 提起来跟不跟手：量「首次切到 drag」到底花多久 ----
    // 这一步包含打开文件 + 解一帧 + 换窗口尺寸（drag 是 221×264，idle 是 384×266），
    // 是用户感知里「提起了却还没开始飘」的全部成本。再切一次则只剩解一帧，
    // 两条数字放在一起才能分出「IO 慢」还是「解码慢」。
    let t0 = std::time::Instant::now();
    pet.play("drag")?;
    let first_ms = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = std::time::Instant::now();
    pet.play("idle")?;
    pet.play("drag")?;
    let again_ms = t1.elapsed().as_secs_f64() * 1e3;
    r.check(
        &format!("首次切到拖拽姿态 {first_ms:.1}ms（已加载后 {again_ms:.1}ms）"),
        first_ms < 50.0,
    );
    pet.play("idle")?;

    drop(pet);

    // ---- 睡觉：睡着后不许自己演到「醒来」----
    // 素材 `sleep.fishanim` 是 10 秒闭环，末尾会把眼睛睁开。
    // 直接整段循环就会「睡十秒、醒一下、又睡」，像个睡不安稳的人。
    let mut pet = Pet::open(dir, None).context("打开桌宠失败（睡眠段）")?;
    // 入睡时间本来是 18~38 秒随机的，测试里缩到瞬间。
    pet.set_sleep_after(0.01);
    std::thread::sleep(std::time::Duration::from_millis(30));
    pet.tick()?;
    r.check(
        &format!("安静 30ms 就睡着（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Sleep),
    );

    let span_falling = pet.loop_span();
    // 入睡段 0~156：一步步推到段尾，看它换段时会不会把帧号拨回去。
    pet.advance_frames(156)?;
    r.check(
        &format!("入睡段演到最后一帧（第 {} 帧）", pet.frame()),
        pet.frame() == 156,
    );
    pet.advance_frames(1)?;
    let span_asleep = pet.loop_span();
    r.check(
        &format!(
            "入睡段 {span_falling:?} 演完转入打呼段 {span_asleep:?}（接上第 {} 帧）",
            pet.frame()
        ),
        // 入睡段就停在打呼段的上界：换段时帧号一步都不用回退。
        span_falling == Some((0, 156)) && span_asleep == Some((104, 156)) && pet.frame() == 156,
    );
    // 打呼段的两端都必须有气泡，而且必须止于身体起身之前。
    // 逐帧数过：气泡在 78~173 之间存在，174 之后一个都没有；身体 176 起开始起身。
    // 下界取 104（已经有 2~3 个）而不是 76（还没气泡），
    // 就是为了不让用户看到「气泡整体消失 → 卡一下 → 从零重新长出来」。
    r.check(
        &format!(
            "打呼段 {}~{} 两端都有气泡、且止于起身之前（气泡 78~173，176 起起身）",
            span_asleep.map_or(0, |(lo, _)| lo),
            span_asleep.map_or(0, |(_, hi)| hi)
        ),
        span_asleep.is_some_and(|(lo, hi)| lo >= 100 && hi <= 173),
    );

    // 关键一条：睡着后无论过多久，都不能演到帧 210+ 那段「睁眼醒来」。
    // 顺带钉住「段内不跳帧」：帧号只能一帧一帧往前走，唯一允许的大跳是
    // 段尾 → 段首这一下回绕（那是排好的接缝，不是 bug）。
    let (lo, hi) = span_asleep.unwrap_or((0, 0));
    let mut escaped = 0u32;
    let mut bad_jump = 0u32;
    let mut wraps = 0u32;
    let mut faded = 0u32;
    let mut hard_cut = 0u32;
    let mut faded_frames = 0u32;
    let mut stalled = 0u32;
    let mut prev = pet.frame();
    for _ in 0..7000 {
        pet.advance_frames(1)?;
        let f = pet.frame();
        if f < lo || f > hi {
            escaped += 1;
        }
        if f < prev {
            // 只可能是回绕：必须从段尾直接回到段首。
            wraps += 1;
            if !(prev == hi && f == lo) {
                bad_jump += 1;
            }
            // 回绕这一下必须已经在淡化：否则就是硬切，接缝又会被看见。
            if pet.seam_dissolving() {
                faded += 1;
            } else {
                hard_cut += 1;
            }
        }
        if pet.seam_dissolving() {
            faded_frames += 1;
            // 淡入不许冻住帧号——那正是用户看到的「卡一下」。
            if f == prev {
                stalled += 1;
            }
        }
        prev = f;
    }
    r.check(
        &format!("睡着后 7000 帧都留在打呼段 {lo}~{hi}（越界 {escaped} 次）"),
        escaped == 0,
    );
    r.check(
        &format!(
            "千帧推进没出现意外跳帧（回绕 {wraps} 次，全部是 {hi} → {lo}，异常 {bad_jump} 次）"
        ),
        bad_jump == 0 && wraps > 0,
    );
    r.check(
        &format!("每次回绕都走了接缝淡入（淡 {faded} 次，硬切 {hard_cut} 次）"),
        hard_cut == 0 && faded == wraps && wraps > 0,
    );
    r.check(
        &format!("淡入期间帧号照常前进（{faded_frames} 拍在淡，冻帧 {stalled} 拍）",),
        stalled == 0 && faded_frames > 0,
    );

    // 鼠标划过只推迟入睡，不许叫醒。
    pet.on_event(Event::MouseMove { x: 10, y: 10 })?;
    r.check(
        &format!("鼠标划过不叫醒（{:?}）", pet.mood()),
        pet.mood() == Some(Mood::Sleep),
    );

    // 但点一下必须醒——而且要先播「睁眼起身」，不能一下子站到开心。
    // 而且必须从**当前那一帧**接着播：跳到固定的 170 帧会让 Z 凭空消失
    // （画面差实测 11.7），用户看到的就是「闪一下」。
    let sleep_frame = pet.frame();
    pet.on_event(Event::MouseDown { x: 10, y: 10 })?;
    pet.on_event(Event::MouseUp { x: 10, y: 10 })?;
    let wake_span = pet.loop_span();
    r.check(
        &format!(
            "点一下就从第 {sleep_frame} 帧接着播醒来段（{wake_span:?}，心情 {:?}）",
            pet.mood()
        ),
        wake_span == Some((sleep_frame, 240))
            && pet.frame() == sleep_frame
            && pet.mood() == Some(Mood::Sleep),
    );

    // 醒来段的长度随「点了多晚」变：睡在 104 帧醒来要演 137 帧，停在 156 则只需 85 帧。
    // 演完就回待机，不留在睡眠区间里，也不走到 click_happy。
    let wake_len = 240 - wake_span.map_or(0, |(lo, _)| lo) + 1;
    let woke_up = pet.advance_frames(wake_len)?;
    r.check(
        &format!(
            "醒来段演 {wake_len} 帧后回待机（{}，心情 {:?}，区间 {:?}）",
            pet.current(),
            pet.mood(),
            pet.loop_span()
        ),
        woke_up
            && pet.current() == "idle"
            && pet.mood() == Some(Mood::Idle)
            && pet.loop_span().is_none(),
    );

    drop(pet);

    // ---- 待机自娱自乐：抽得到、播得完、播完回 idle ----
    let mut pet = Pet::open(dir, None).context("打开桌宠失败（小动作段）")?;
    // 入睡时间拉到一小时：这一段只关心“会不会玩”，不关心“会不会睡”。
    pet.set_sleep_after(3600.0);
    let pool: Vec<&str> = pet.action_pool().iter().map(|(id, _)| *id).collect();
    r.check(
        &format!("小动作池按素材过滤后非空（{pool:?}）"),
        !pool.is_empty(),
    );

    pet.trigger_next_action();
    pet.tick()?;
    let picked = pet.current().to_owned();
    r.check(
        &format!("够无聊了会抽个小动作（{picked}）"),
        pool.contains(&picked.as_str()),
    );

    // 推完整一圈：必须自己回 idle，不能卡在小动作上永远演。
    let went_idle = pet.advance_frames(300)?;
    r.check(
        &format!(
            "小动作播完一圈就回待机（{}，回待机 = {went_idle}）",
            pet.current()
        ),
        went_idle && pet.current() == "idle",
    );

    // ---- 走路段**逐段**验：跳过前摇 + 方向跟素材一致（反方向靠镜像）----
    // 用户报的两个毛病都在这里：
    // * 人先动、一秒后才开始迈腿 → 循环起点必须在真正迈步的帧；
    // * 素材朝左人却向右走 → 往反方向走时必须水平镜像（`flipped()`）。
    for id in ["move_float", "move_crab"] {
        pet.back_to_home()?;
        let started = pet.wander_with(id);
        let span = pet.loop_span();
        let flip = pet.flipped();
        let x0 = pet.anchor_x();
        let _ = pet.advance_frames(200)?;
        let dx = pet.anchor_x() - x0;
        // `move_crab` 素材朝左（faces_left = Some(true)）：往右走的时候必须镜像。
        let flip_ok = if id == "move_crab" {
            flip == (dx > 0)
        } else {
            true
        };
        r.check(
            &format!(
                "{id}：从第 {} 帧起走（区间 {span:?}），锚点 {dx:+}px，镜像 = {flip}",
                span.map_or(0, |(lo, _)| lo)
            ),
            started
                && span.is_some_and(|(lo, hi)| lo >= 18 && hi >= 200)
                && dx.abs() >= 40
                && flip_ok,
        );
    }

    // ---- 参数表自身：带区间的段都得「跳过前摇」且装得下整个动作 ----
    // 这条就是用户要求的「每段动画都验一遍」：以后加新素材，表里写错区间会当场报。
    let mut bad: Vec<String> = Vec::new();
    for id in ["drag", "move_float", "move_crab"] {
        match Pet::clip_span(id) {
            Some((lo, hi)) if lo >= 1 && hi > lo + 60 => {}
            other => bad.push(format!("{id}={other:?}")),
        }
    }
    r.check(
        &format!(
            "参数表里的区间都跳过了前摇（drag / move_float / move_crab{}{}）",
            if bad.is_empty() { "" } else { "，异常：" },
            bad.join(", ")
        ),
        bad.is_empty(),
    );

    // ---- 镜像：同一段素材往两个方向走，朝反方向时必须翻转 ----
    // 用户报的「素材朝左、人却向右走」就是这里没翻转。
    for (id, right, want_flip) in [("move_crab", true, true), ("move_crab", false, false)] {
        pet.back_to_home()?;
        let started = pet.wander_dir(id, right);
        let flip = pet.flipped();
        let x0 = pet.anchor_x();
        let _ = pet.advance_frames(60)?;
        let dx = pet.anchor_x() - x0;
        r.check(
            &format!(
                "{id} 往{}走：镜像 = {flip}（期望 {want_flip}），锚点 {dx:+}px",
                if right { "右" } else { "左" }
            ),
            started && flip == want_flip && (dx > 0) == right,
        );
    }

    // ---- 参数表里每段都要有说明（`--showcase` 靠它告诉人「该看到什么」）----
    let mut no_desc = Vec::new();
    for id in [
        "idle",
        "sleep",
        "drag",
        "click_happy",
        "click_pout",
        "stretch",
        "mirror",
        "turn",
        "move_float",
        "move_crab",
    ] {
        if Pet::clip_desc(id).is_empty() {
            no_desc.push(id);
        }
    }
    r.check(
        &format!(
            "参数表里每段都有说明（缺 {}）",
            if no_desc.is_empty() {
                "无".to_owned()
            } else {
                no_desc.join(",")
            }
        ),
        no_desc.is_empty(),
    );

    // ---- 转向跟随：鼠标从「鱼身上」移到「一个鱼身宽以外」时转一次头 ----
    // 用户的两条要求：①鼠标在鱼旁边不转；②只有「离开了」才转，且只转一次。
    // 先把上一段走路演完（`poll_look` 在 `action_playing` 期间不抢戏）。
    // 螃蟹步只有 1.2px/帧，800px 要走 600 多帧，所以这里推到 900。
    let _ = pet.advance_frames(900)?;
    pet.back_to_home()?;
    pet.set_follow(true);
    pet.reset_look_cooldown();
    let near_x = pet.anchor_x();
    let far_x = near_x + 900;
    let not_turned = pet.poll_look(near_x);
    let turned = pet.poll_look(far_x);
    let again = pet.poll_look(far_x + 50); // 已经在外面了，不该再转
    r.check(
        &format!(
            "鼠标移出鱼身宽后转头一次（{}，区间 {:?}）",
            pet.current(),
            pet.loop_span()
        ),
        !not_turned && turned && !again && pet.current() == "idle",
    );
    pet.set_follow(false);

    // ---- 点哪走哪：距差不到「一个鱼身宽」不动，超了就开走 ----
    // `turn` 是 241 帧的整段，得推完才能测（`walk_to` 在演动作时不抢）。
    let _ = pet.advance_frames(260)?;
    pet.back_to_home()?;
    let x0 = pet.anchor_x();
    let too_near = pet.walk_to(x0 + 40);
    let far = pet.walk_to(x0 + 900);
    let walked = pet.walk_to(x0 + 900);
    r.check(
        &format!(
            "点哪走哪：近距离不动（{too_near}），远处开走（{far}，{}）",
            pet.current()
        ),
        !too_near && far,
    );
    // 已经在走的时候点新方向：应该**就地改向**（可打断），而不是被拒绝。
    r.check(
        &format!("走路期间点新方向能立即改向（{walked}，{}）", pet.current()),
        walked,
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
