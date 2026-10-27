//! 桌宠运行时：窗口 + 素材清单 + 状态机。
//!
//! 与 P1 的关键差别：动画不再写死 `idle`，而是由 [`Brain`] 按事件切换。
//! 12 段动画的裁切尺寸各不相同（193×242 到 384×266），切换时靠清单里的几何信息把角色钉在原地：
//!
//! * 纵向：`window_y = 屏幕脚底线 - baseline`。`baseline` 是**帧内**落脚行，
//!   用它而不是 `origin_y + baseline`——后者是画布坐标，把两者搞混会让角色错位几百像素。
//! * 横向：`window_x = 锚点x - 画布宽/2 + origin_x`。以原画布中心为基准，
//!   这样 193 宽的 `stretch` 和 384 宽的 `sleep` 不会各自左右游移。

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use feiyu_core::anim::{AnimReader, Manifest};
use feiyu_core::brain::{Brain, Mood, Stimulus};
use feiyu_platform::{Event, LayeredWindow};

/// 脚底距工作区底边的空隙（像素）。
const BOTTOM_GAP: i32 = 4;

/// 单帧推进的时间上限：卡顿后不要一次跳过几十帧。
const MAX_ANIM_DT: f32 = 0.1;

type Anim = AnimReader<BufReader<File>>;

/// 一只桌宠的全部运行时状态。
pub struct Pet {
    win: LayeredWindow,
    /// 素材目录（`manifest.json` 所在处）。
    dir: PathBuf,
    manifest: Manifest,
    /// 已打开的动画。按需加载，没播过的段不占内存。
    anims: HashMap<String, Anim>,
    current: String,
    /// 当前帧号（对当前动画取模）。
    frame: u32,
    /// 不足一帧的时间余量。
    acc: f32,
    /// 画布坐标锚点：角色中心 x。
    anchor_x: i32,
    /// 画布坐标锚点：脚底 y。
    feet_y: i32,
    brain: Brain,
    last: Instant,
    /// 强制播放某段动画时置位，此时状态机不参与决策（截图/调试用）。
    forced: Option<String>,
    /// 正在被拖拽（收到过 Moved、还没松手）。
    dragging: bool,
    /// 累计提交帧数。
    drawn: u64,
}

impl Pet {
    /// 打开素材目录并建窗口。`forced` 指定则只播这一段、忽略状态机。
    pub fn open(dir: impl AsRef<Path>, forced: Option<&str>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let manifest = Manifest::load(dir.join("manifest.json"))
            .with_context(|| format!("读不了素材清单 {}", dir.join("manifest.json").display()))?;

        let first = match forced {
            Some(id) => manifest.require(id)?.id.clone(),
            None => "idle".to_owned(),
        };
        let entry = manifest.require(&first)?.clone();

        // 锚点：主屏工作区底部居中（工作区已排除任务栏）。
        let (left, _, right, bottom) =
            feiyu_platform::primary_work_area().context("取屏幕工作区失败，无法定位窗口")?;
        let anchor_x = (left + right) / 2;
        let feet_y = bottom - BOTTOM_GAP;

        // 先按清单算好位置再建窗口，避免窗口先出现在 (0,0) 再跳过去。
        let x = anchor_x - manifest.canvas.width as i32 / 2 + entry.origin[0] as i32;
        let y = feet_y - entry.baseline as i32;
        let win = LayeredWindow::create(entry.width() as u32, entry.height() as u32, x, y, "飞鱼")
            .context("创建分层窗口失败")?;

        let mut pet = Self {
            win,
            dir,
            manifest,
            anims: HashMap::new(),
            current: first,
            frame: 0,
            acc: 0.0,
            anchor_x,
            feet_y,
            brain: Brain::default(),
            last: Instant::now(),
            forced: forced.map(str::to_owned),
            dragging: false,
            drawn: 0,
        };
        pet.ensure_loaded(&pet.current.clone())?;
        pet.draw_frame()?;
        Ok(pet)
    }

    /// 素材目录里全部动画 id（按清单顺序）。
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.manifest.ids()
    }

    /// 当前正在播的动画 id。
    pub fn current(&self) -> &str {
        &self.current
    }

    /// 当前姿态；强制模式下为 `None`（状态机不参与）。
    pub fn mood(&self) -> Option<Mood> {
        self.forced.is_none().then(|| self.brain.mood())
    }

    /// 累计提交帧数。
    pub fn drawn(&self) -> u64 {
        self.drawn
    }

    /// 显示窗口（不抢焦点）。
    pub fn show(&self) {
        self.win.show();
    }

    /// 启动/停止周期定时器。
    pub fn set_tick(&self, ms: u32) {
        self.win.set_tick(ms);
    }

    /// 清单原始 fps。
    pub fn fps(&self) -> f32 {
        self.manifest.fps
    }

    /// 切到 `id` 并立刻提交第一帧。尺寸变化时窗口会跟着变。
    pub fn play(&mut self, id: &str) -> Result<()> {
        if id == self.current && self.anims.contains_key(id) {
            return Ok(());
        }
        self.ensure_loaded(id)?;
        let entry = self.manifest.require(id)?.clone();
        let x = self.anchor_x - self.manifest.canvas.width as i32 / 2 + entry.origin[0] as i32;
        let y = self.feet_y - entry.baseline as i32;

        self.current = id.to_owned();
        self.frame = 0;
        self.acc = 0.0;
        // 先摆位置再画：`UpdateLayeredWindow` 带尺寸时**保留左上角**，
        // 所以新尺寸的帧若是宽高变大的，必须先落位，否则会从旧左上角长出去。
        self.win.move_to(x, y);
        self.draw_frame()
    }

    /// 时间前进：驱动状态机与帧号。
    pub fn tick(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;

        // 状态机吃真实 dt（休眠两小时醒来要能直接睡着）；
        // 帧推进吃截断后的 dt，免得卡顿一次跳过几十帧。
        if self.forced.is_none() && self.brain.advance(dt) {
            let id = self.brain.anim_id();
            self.play(id)?;
        }

        let anim_dt = dt.min(MAX_ANIM_DT);
        self.acc += anim_dt * self.manifest.fps;
        if self.acc < 1.0 {
            return Ok(());
        }
        let step = self.acc.floor();
        self.acc -= step;
        let count = self.current_count();
        self.frame = (self.frame + step as u32) % count;
        self.draw_frame()
    }

    /// 处理一个窗口事件。右键菜单与退出由调用方决定，这里不处理。
    pub fn on_event(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::Tick => self.tick(),
            // 记下“还没开始拖”，松手时据此区分点击与拖拽。
            Event::MouseDown { .. } => {
                self.dragging = false;
                Ok(())
            }
            Event::Moved { .. } => {
                if !self.dragging {
                    self.dragging = true;
                    self.stimulate(Stimulus::DragStart)?;
                }
                Ok(())
            }
            Event::MouseUp { .. } => {
                if self.dragging {
                    self.dragging = false;
                    // 拖到哪就停在哪：把锚点重新对齐到实际窗口位置。
                    self.capture_anchor()?;
                    self.stimulate(Stimulus::DragEnd)
                } else {
                    self.stimulate(Stimulus::Click)
                }
            }
            Event::MouseMove { .. } | Event::RightClick { .. } | Event::Quit => Ok(()),
        }
    }

    /// 连续提交 `n` 帧，返回 `(平均解码 ms, 平均提交 ms)`。
    ///
    /// 拆开量是因为优化必须有靶子：P1 时就曾误判“解压慢”，
    /// 实际是读取器在重复重解差分链。
    pub fn bench(&mut self, n: u32) -> Result<(f64, f64)> {
        let entry = self.manifest.require(&self.current)?.clone();
        let (w, h) = (entry.width() as u32, entry.height() as u32);
        let Self { win, anims, current, frame, drawn, .. } = self;
        let anim = anims.get_mut(current).with_context(|| format!("动画 {current} 尚未加载"))?;
        let count = anim.frame_count().max(1);
        let (mut dec, mut drw) = (0u128, 0u128);
        for _ in 0..n {
            *frame = (*frame + 1) % count;
            let t0 = Instant::now();
            let pixels = anim.frame(*frame)?;
            dec += t0.elapsed().as_nanos();
            let t1 = Instant::now();
            win.draw(w, h, pixels)?;
            drw += t1.elapsed().as_nanos();
            *drawn += 1;
        }
        let n = n.max(1) as f64;
        Ok((dec as f64 / n / 1e6, drw as f64 / n / 1e6))
    }

    /// 按当前动画的尺寸/坐标重设 DIB 并提交一帧。
    pub fn draw_frame(&mut self) -> Result<()> {
        let entry = self.manifest.require(&self.current)?.clone();
        // 解构借用：同一次调用里要同时碰 win（可变）与 anims（可变）。
        let Self { win, anims, current, frame, drawn, .. } = self;
        let anim = anims
            .get_mut(current)
            .with_context(|| format!("动画 {current} 尚未加载"))?;
        let idx = *frame % anim.frame_count().max(1);
        let pixels = anim.frame(idx).with_context(|| format!("解 {current} 第 {idx} 帧失败"))?;
        win.draw(entry.width() as u32, entry.height() as u32, pixels)
            .context("提交画面失败")?;
        *drawn += 1;
        Ok(())
    }

    /// 截当前窗口到 BMP。
    pub fn capture(&self, path: impl AsRef<Path>) -> Result<(u32, u32)> {
        feiyu_platform::shot::capture_to_bmp(self.win.hwnd(), path.as_ref())
            .context("截屏失败")
    }

    /// 窗口矩形（屏幕坐标）。自检用。
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        self.win.rect()
    }

    fn current_count(&self) -> u32 {
        self.anims.get(&self.current).map(|a| a.frame_count()).unwrap_or(1).max(1)
    }

    /// 把刺激交给状态机；姿态真的变了才换动画。
    fn stimulate(&mut self, s: Stimulus) -> Result<()> {
        if self.forced.is_some() {
            return Ok(());
        }
        if self.brain.stimulus(s) {
            let id = self.brain.anim_id();
            self.play(id)?;
        }
        Ok(())
    }

    /// 拖拽结束后，按当前动画的 origin/baseline 把锚点对回实际窗口位置，
    /// 这样后续切动画时角色不会瞬间跳回屏幕中间。
    fn capture_anchor(&mut self) -> Result<()> {
        let entry = self.manifest.require(&self.current)?.clone();
        let (left, top, _, _) = self.win.rect();
        self.anchor_x = left + self.manifest.canvas.width as i32 / 2 - entry.origin[0] as i32;
        self.feet_y = top + entry.baseline as i32;
        Ok(())
    }

    /// 按需打开动画，并核对“清单说的”与“文件里写的”是否一致。
    ///
    /// 这条核对针对的是真实事故场景：改过打包参数却忘了重新生成清单，
    /// 会导致按清单尺寸提交位图、实际帧却是另一个尺寸，画面直接错位。
    fn ensure_loaded(&mut self, id: &str) -> Result<()> {
        if self.anims.contains_key(id) {
            return Ok(());
        }
        let entry = self.manifest.require(id)?.clone();
        let path = entry.path_in(&self.dir);
        let anim = AnimReader::open(&path)
            .with_context(|| format!("打不开动画 {}（id={id}）", path.display()))?;
        let header = *anim.header();
        ensure!(
            header.width == entry.width() && header.height == entry.height(),
            "清单与文件尺寸不符（{id}）：清单 {}×{}，文件 {}×{}；请重新生成 manifest",
            entry.width(),
            entry.height(),
            header.width,
            header.height
        );
        ensure!(
            header.frame_count == entry.frames,
            "清单与文件帧数不符（{id}）：清单 {}，文件 {}；请重新生成 manifest",
            entry.frames,
            header.frame_count
        );
        self.anims.insert(id.to_owned(), anim);
        Ok(())
    }
}
