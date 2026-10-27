//! 桌宠运行时：窗口 + 素材清单 + 状态机。
//!
//! 与 P1 的关键差别：动画不再写死 `idle`，而是由 [`Brain`] 按事件切换。
//! 12 段动画的裁切尺寸各不相同（193×242 到 384×266），切换时靠清单里的几何信息把角色钉在原地：
//!
//! * 纵向：`window_y = 屏幕脚底线 - baseline`。`baseline` 是**帧内**落脚行，
//!   用它而不是 `origin_y + baseline`——后者是画布坐标，把两者搞混会让角色错位几百像素。
//! * 横向：`window_x = 锚点x - 画布宽/2 + origin_x`。以原画布中心为基准，
//!   这样 193 宽的 `stretch` 和 384 宽的 `sleep` 不会各自左右游移。
//!
//! 拖动不再「松手就停」：松手时用最近 120ms 的采样估一个速度，交给
//! [`feiyu_core::motion`] 做滑行 + 重力。位置的真源是锚点 `(anchor_x, feet_y)`，
//! 窗口每帧跟着锚点走，所以滑行途中换动画也不会跳位。

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use feiyu_core::anim::{AnimEntry, AnimReader, Manifest};
use feiyu_core::brain::{Brain, Mood, Stimulus};
use feiyu_core::motion::{fling_velocity, Limits, Motion};
use feiyu_platform::{CommandId, Event, LayeredWindow, Menu};

/// 脚底距工作区底边的空隙（像素）。
const BOTTOM_GAP: i32 = 4;

/// 单帧推进的时间上限：卡顿后不要一次跳过几十帧。
const MAX_ANIM_DT: f32 = 0.1;

/// 位移超过这个距离才算「拖拽」，否则松手仍当一次点击。
///
/// 没有这个阈值时，点击鼠标时手抖一两个像素就会把「点一下」吞成拖拽，
/// 表现是「点它没反应」。
const DRAG_THRESHOLD: i32 = 4;

/// 拖拽采样只保留最近这么久（秒）。
///
/// 窗口开太长会把「拖到位、停住两秒、再松手」算成一次快甩；
/// 太短又会在正常拖动时凑不够两个采样点。
const TRAIL_WINDOW: f32 = 0.12;

/// 右键菜单的命令 id（含义见 [`Pet::menu`]）。
const CMD_HOME: CommandId = 1;
const CMD_TOPMOST: CommandId = 2;
const CMD_QUIT: CommandId = 3;

type Anim = AnimReader<BufReader<File>>;

/// 右键菜单选完之后要做什么。
///
/// 退出这动作留给 app 层执行：`feiyu-platform` 不该知道应用语义，
/// `pet` 也不该自己去关消息循环。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 什么都没选（点了别处或按了 Esc）。
    None,
    /// 角色已搬回屏幕底部中央。
    Home,
    /// 「总在最前」切换后的新状态。
    Topmost(bool),
    /// 用户要退出。
    Quit,
}

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
    /// 正在被拖拽（位移已超过阈值、还没松手）。
    dragging: bool,
    /// 累计提交帧数。
    drawn: u64,
    /// 主显示器工作区 `(left, top, right, bottom)`。反弹边界与「回到底部」都靠它。
    area: (i32, i32, i32, i32),
    /// 松手后的滑行/下落；`None` 表示已停稳。
    motion: Option<Motion>,
    /// 最近的拖拽采样 `(时刻, 窗口左上角)`，松手时用来估甩动速度。
    trail: VecDeque<(Instant, i32, i32)>,
    /// 左键按下时的窗口左上角，用来区分「点击」与「拖拽」。
    grab_origin: Option<(i32, i32)>,
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
        let area = feiyu_platform::primary_work_area().context("取屏幕工作区失败，无法定位窗口")?;
        let anchor_x = (area.0 + area.2) / 2;
        let feet_y = area.3 - BOTTOM_GAP;

        // 先按清单算好位置再建窗口，避免窗口先出现在 (0,0) 再跳过去。
        let (x, y) = window_pos(manifest.canvas.width, &entry, anchor_x, feet_y);
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
            area,
            motion: None,
            trail: VecDeque::new(),
            grab_origin: None,
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

        self.current = id.to_owned();
        self.frame = 0;
        self.acc = 0.0;
        // 先摆位置再画：`UpdateLayeredWindow` 带尺寸时**保留左上角**，
        // 所以新尺寸的帧若是宽高变大的，必须先落位，否则会从旧左上角长出去。
        self.place_window(&entry);
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

        // 松手后的滑行/下落。先算运动再推进帧：位置是这一帧的最终结果，
        // 顺序反了会让迟一帧的落点闪到边界外。
        if let Some(mut motion) = self.motion.take() {
            let entry = self.manifest.require(&self.current)?.clone();
            let limits = self.limits_for(&entry);
            let anchor = (self.anchor_x as f32, self.feet_y as f32);
            let ((x, y), rest) = motion.step(anchor, limits, dt);
            self.anchor_x = x.round() as i32;
            self.feet_y = y.round() as i32;
            self.place_window(&entry);
            if !rest {
                self.motion = Some(motion);
            }
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
            // 记下按下时的位置：位移不足阈值就仍按「点击」算。
            Event::MouseDown { .. } => {
                let (left, top, _, _) = self.win.rect();
                self.grab_origin = Some((left, top));
                self.dragging = false;
                self.trail.clear();
                Ok(())
            }
            Event::Moved { x, y } => self.on_moved(x, y),
            Event::MouseUp { .. } => self.on_button_up(),
            Event::MouseMove { .. } | Event::RightClick { .. } | Event::Quit => Ok(()),
        }
    }

    /// 窗口被拖到了 `(x, y)`（屏幕坐标，左上角）。
    fn on_moved(&mut self, x: i32, y: i32) -> Result<()> {
        let now = Instant::now();
        self.trail.push_back((now, x, y));
        self.prune_trail(now);

        if !self.dragging {
            let Some((ox, oy)) = self.grab_origin else {
                return Ok(());
            };
            // 取两轴最大位移：斜着抖也算抖。
            if (x - ox).abs().max((y - oy).abs()) < DRAG_THRESHOLD {
                return Ok(());
            }
            self.dragging = true;
            self.stimulate(Stimulus::DragStart)?;
        }
        Ok(())
    }

    /// 左键抬起：真拖过就「甩出去 + 落地」，没拖过就算点了一下。
    fn on_button_up(&mut self) -> Result<()> {
        let was_dragging = std::mem::replace(&mut self.dragging, false);
        self.grab_origin = None;
        if !was_dragging {
            self.trail.clear();
            return self.stimulate(Stimulus::Click);
        }
        // 拖到哪就从哪开始：先把锚点对回实际窗口位置，再据此起飞。
        self.capture_anchor()?;
        let (vx, vy) = self.take_fling();
        self.motion = Some(Motion::fling(vx, vy));
        self.stimulate(Stimulus::DragEnd)
    }

    /// 用最近的拖拽采样估一个松手速度，并清空采样。
    fn take_fling(&mut self) -> (f32, f32) {
        let now = Instant::now();
        // 先在**松手这一刻**剪一遍：否则「拖到位后停住一秒再松手」
        // 会拿一秒前的数据算出一次根本不存在的快甩。
        self.prune_trail(now);
        let base = self.trail.front().map(|s| s.0).unwrap_or(now);
        let samples: Vec<(f32, i32, i32)> = self
            .trail
            .iter()
            .map(|(t, x, y)| (t.duration_since(base).as_secs_f32(), *x, *y))
            .collect();
        self.trail.clear();
        fling_velocity(&samples)
    }

    /// 丢掉过老的拖拽采样。
    fn prune_trail(&mut self, now: Instant) {
        while self
            .trail
            .front()
            .is_some_and(|s| now.duration_since(s.0).as_secs_f32() > TRAIL_WINDOW)
        {
            self.trail.pop_front();
        }
    }

    /// 连续提交 `n` 帧，返回 `(平均解码 ms, 平均提交 ms)`。
    ///
    /// 拆开量是因为优化必须有靶子：P1 时就曾误判“解压慢”，
    /// 实际是读取器在重复重解差分链。
    pub fn bench(&mut self, n: u32) -> Result<(f64, f64)> {
        let entry = self.manifest.require(&self.current)?.clone();
        let (w, h) = (entry.width() as u32, entry.height() as u32);
        let Self {
            win,
            anims,
            current,
            frame,
            drawn,
            ..
        } = self;
        let anim = anims
            .get_mut(current)
            .with_context(|| format!("动画 {current} 尚未加载"))?;
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
        let Self {
            win,
            anims,
            current,
            frame,
            drawn,
            ..
        } = self;
        let anim = anims
            .get_mut(current)
            .with_context(|| format!("动画 {current} 尚未加载"))?;
        let idx = *frame % anim.frame_count().max(1);
        let pixels = anim
            .frame(idx)
            .with_context(|| format!("解 {current} 第 {idx} 帧失败"))?;
        win.draw(entry.width() as u32, entry.height() as u32, pixels)
            .context("提交画面失败")?;
        *drawn += 1;
        Ok(())
    }

    /// 截当前窗口到 BMP。
    pub fn capture(&self, path: impl AsRef<Path>) -> Result<(u32, u32)> {
        feiyu_platform::shot::capture_to_bmp(self.win.hwnd(), path.as_ref()).context("截屏失败")
    }

    /// 窗口矩形（屏幕坐标）。自检用。
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        self.win.rect()
    }

    /// 主显示器工作区 `(left, top, right, bottom)`。自检用。
    pub fn area(&self) -> (i32, i32, i32, i32) {
        self.area
    }

    /// 当前是否「总在最前」。自检用。
    pub fn topmost(&self) -> bool {
        self.win.is_topmost()
    }

    /// 当前动画的锚点允许范围。自检用。
    pub fn limits(&self) -> Limits {
        let Ok(entry) = self.manifest.require(&self.current) else {
            return Limits {
                x: (0.0, 0.0),
                y: (0.0, 0.0),
            };
        };
        self.limits_for(entry)
    }

    /// 在屏幕坐标 `(x, y)` 弹出右键菜单，返回应该做什么。
    ///
    /// 菜单内容就是功能边界的一次声明：这里**故意不放「隐藏」**——
    /// 还没有托盘图标，藏起来就再也点不到了。
    pub fn menu(&mut self, x: i32, y: i32) -> Result<Action> {
        let topmost = self.win.is_topmost();
        let mut menu = Menu::new()?;
        menu.command(CMD_HOME, "回到屏幕底部")
            .check(CMD_TOPMOST, "总在最前", topmost)
            .separator()
            .command(CMD_QUIT, "退出");
        let Some(cmd) = menu.show_at(self.win.hwnd(), x, y)? else {
            return Ok(Action::None);
        };
        match cmd {
            CMD_HOME => {
                self.back_to_home()?;
                Ok(Action::Home)
            }
            CMD_TOPMOST => {
                let next = !topmost;
                self.win.set_topmost(next);
                Ok(Action::Topmost(next))
            }
            CMD_QUIT => Ok(Action::Quit),
            other => {
                // 菜单里没有这一项：不静默吞掉，留条线索。
                eprintln!("未知菜单命令 {other}，已忽略");
                Ok(Action::None)
            }
        }
    }

    /// 给一个初速度让它自己滑/落。
    ///
    /// 只给调试模式（`--fling`）用：真机拖拽走 [`Self::on_button_up`]，
    /// 路径一模一样，这个入口只是把「需要鼠标才能触发」这一步拆掉，
    /// 让「物理真的在推窗口」可以被截屏取证。
    pub fn start_fling(&mut self, vx: f32, vy: f32) {
        self.motion = Some(Motion::fling(vx, vy));
    }

    /// 搬回工作区底部中央，并终止正在进行的滑行/下落。
    pub fn back_to_home(&mut self) -> Result<()> {
        self.anchor_x = (self.area.0 + self.area.2) / 2;
        self.feet_y = self.area.3 - BOTTOM_GAP;
        self.motion = None;
        let entry = self.manifest.require(&self.current)?.clone();
        self.place_window(&entry);
        Ok(())
    }

    /// 按锚点摆放窗口。
    fn place_window(&self, entry: &AnimEntry) {
        let (x, y) = window_pos(
            self.manifest.canvas.width,
            entry,
            self.anchor_x,
            self.feet_y,
        );
        self.win.move_to(x, y);
    }

    /// 当前动画的锚点允许范围。
    ///
    /// 由「裁切几何 + 工作区」算出来：窗口左右边缘不能越过工作区，
    /// 脚底的下界就是地面。区间可能退化（窗口比工作区还宽），
    /// 这里直接压成中点——[`Motion::step`] 只拿它做比较，自己不会兜底。
    fn limits_for(&self, entry: &AnimEntry) -> Limits {
        // 锚点 → 窗口左边缘的距离。
        let dx = self.manifest.canvas.width as f32 / 2.0 - entry.origin[0] as f32;
        // 锚点 → 窗口上边缘的距离。
        let dy = entry.baseline as f32;
        let w = entry.width() as f32;
        let ground = (self.area.3 - BOTTOM_GAP) as f32;
        Limits {
            x: ordered(self.area.0 as f32 + dx, self.area.2 as f32 - w + dx),
            y: ordered(self.area.1 as f32 + dy, ground),
        }
    }

    fn current_count(&self) -> u32 {
        self.anims
            .get(&self.current)
            .map(|a| a.frame_count())
            .unwrap_or(1)
            .max(1)
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

/// 由锚点算出窗口左上角。
///
/// 所有「摆窗口」的地方必须走这里：同一个公式散落三份时，改动漏掉一处
/// 就会变成 P1 那种「纵向错 46 像素」的 bug。
fn window_pos(canvas_width: u16, entry: &AnimEntry, anchor_x: i32, feet_y: i32) -> (i32, i32) {
    let x = anchor_x - canvas_width as i32 / 2 + entry.origin[0] as i32;
    let y = feet_y - entry.baseline as i32;
    (x, y)
}

/// 保证区间左小右大；退化（窗口比工作区还大）时压成中点。
fn ordered(lo: f32, hi: f32) -> (f32, f32) {
    if lo <= hi {
        (lo, hi)
    } else {
        let mid = (lo + hi) / 2.0;
        (mid, mid)
    }
}
