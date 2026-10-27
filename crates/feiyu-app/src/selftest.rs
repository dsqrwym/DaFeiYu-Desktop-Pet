//! 自检：把「窗口真的能逐像素透明、真的能点到、真的画出来了」变成可执行断言。
//!
//! 全部走真实 Win32 路径（真建窗口、真提交位图、真从屏幕上抓像素），
//! 不做 mock——mock 过的窗口测不出 DWM 合成问题。

use std::fs;
use std::time::Instant;

use anyhow::{Context, Result};
use feiyu_platform::{pump, quit, Event, LayeredWindow, SingleInstance};

const W: u32 = 200;
const H: u32 = 240;
const WIN_X: i32 = 400;
const WIN_Y: i32 = 300;

/// 逐个执行断言，返回是否全部通过。
pub fn run() -> Result<bool> {
    let mut r = Report::default();
    println!("飞鱼自检\n");

    r.check("DPI 每显示器 v2 感知", feiyu_platform::enable_per_monitor_v2());

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

    drop(win);
    println!("\n{}", r.summary());
    Ok(r.failed == 0)
}

#[derive(Default)]
struct Report {
    passed: u32,
    failed: u32,
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

    fn summary(&self) -> String {
        format!(
            "自检结果：{} 通过 / {} 失败{}",
            self.passed,
            self.failed,
            if self.failed == 0 { "，全部通过" } else { "，存在失败项" }
        )
    }
}
