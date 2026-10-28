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
use feiyu_core::pool::Pool;
use feiyu_core::rng::Rng;
use feiyu_core::sleep::{span_to_frames, SleepCycle, SleepStage};
use feiyu_platform::{CommandId, Event, Icon, LayeredWindow, Menu, TrayIcon};

/// 脚底距工作区底边的空隙（像素）。
const BOTTOM_GAP: i32 = 4;

/// 藏进托盘后的慢拍子（毫秒）。
///
/// **不能停成 0**：托盘回调是 shell 用同步投递发给窗口的，事件进了队列要等
/// 下一条被取出的消息才会被排空；定时器一停，那条 `TrayLeftClick` 就永远躺在
/// 队列里（用户：「菜单隐藏后点托盘图标没反应」）。1 秒一拍既几乎不耗 CPU
/// （实测隐藏 10 秒增量 0.0000s），又不至于让点击看起来没反应。
const HIDDEN_TICK_MS: u32 = 1000;

/// 单帧推进的时间上限：卡顿后不要一次跳过几十帧。
const MAX_ANIM_DT: f32 = 0.1;

/// 位移超过这个距离才算「拖拽」，否则松手仍当一次点击。
///
/// 没有这个阈值时，点击鼠标时手抖一两个像素就会把「点一下」吞成拖拽，
/// 表现是「点它没反应」。
///
/// 取 2 而不是 4：自检实测切换 drag 姿态只要 0.4ms，用户感知到的
/// 「提起了却还没飘」全部来自这段死区。阈值只该挡住真实的手抖
/// （1~2 像素），不该挡住「提起」这个动作本身。
const DRAG_THRESHOLD: i32 = 2;

/// `drag` 素材里「已经离地」的那一段（闭区间，帧号）。
///
/// 用户报「拖一两秒才漂浮」，罪不在切换逻辑（自检实测切 drag 0.4ms），
/// 而在素材本身：这段 10 秒的循环是从**站着**开始演的。用 `bbox` 逐帧量
/// 包围盒（见 `_notes/PROGRESS.md`）得到的实测曲线：
///
/// * 0~20：底边贴着画布底部（y2=262），人还站在地上；
/// * 25~30：正在升起（y2=248 → 239）；
/// * 35~205：悬空（y2 在 233~251 之间起伏，头顶快贴画布上沿）；
/// * 210 之后：落地。
///
/// 所以从第 35 帧进、在这个区间里循环，「提起来」就是按下即生效的。
/// 段尾取 110 而不是 205：`110 → 35` 的接缝平均差 14.8，和素材自己
/// 0.2 秒的变化量（7~13）同级——回绕看不出来；从 205 回 35 会一下跳 12 像素。
const DRAG_LO: u32 = 35;
const DRAG_HI: u32 = 110;

/// 松手落地后，先保持被拖拽姿态这么久再回待机。
///
/// 直接切回 idle 会显得「一到手就站好」，像被剪掉了一帧；
/// 0.1 秒刚够眼睛确认「落地了」，再长就开始拖沓。
pub(crate) const LANDING_HOLD: f32 = 0.1;

/// 拖拽采样只保留最近这么久（秒）。
///
/// 窗口开太长会把「拖到位、停住两秒、再松手」算成一次快甩；
/// 太短又会在正常拖动时凑不够两个采样点。
const TRAIL_WINDOW: f32 = 0.12;

/// 右键菜单的命令 id（含义见 [`Pet::menu`]）。
const CMD_HOME: CommandId = 1;
const CMD_TOPMOST: CommandId = 2;
const CMD_QUIT: CommandId = 3;
const CMD_HIDE: CommandId = 4;

/// 托盘图标的悬停提示。
const TRAY_TIP: &str = "飞鱼";

/// 入睡前要安静的时长范围（秒）。
///
/// 给一个区间而不是固定值：固定 25 秒会让「它什么时候睡」变成可预测的定时器。
/// 每次回到待机都重新抽一次，于是每次入睡的等待都不一样。
const SLEEP_AFTER_MIN: f32 = 18.0;
const SLEEP_AFTER_MAX: f32 = 38.0;

/// 两次自娱自乐之间的间隔（秒）。
///
/// 比入睡阈值短，清醒期间才看得到一会儿；又不至于太频繁——
/// 一直在扭的桌宠会显得焦躁。
const ACTION_GAP_MIN: f32 = 8.0;
const ACTION_GAP_MAX: f32 = 15.0;

/// 待机时会随机播的小动作。
///
/// 只列**确实打包进来了**的段：原版 `categories.小动作` 有 18 段，
/// 我们只打包了 3 段，其中 `sleep` 已归睡觉姿态，剩 `stretch` 和 `mirror`。
/// 权重照原版的 categories=20；二期补 `turn`/`move` 时按原比例追加即可。
const ACTION_POOL: &[(&str, u32)] = &[("stretch", 20), ("mirror", 20)];

/// 睡着时，系统有键鼠输入就按这个间隔掷一次「自然醒」的骰子（秒）。
///
/// 不按输入事件逐次掷：鼠标一动就是几十条消息，逐次掷会让它一秒不到就醒。
/// 按秒掷才符合「你在电脑前坐了一阵，它被你弄醒了」的观感。
const WAKE_ROLL_EVERY: f32 = 1.0;

/// 每次掷骰醒来的概率。
///
/// 10% → 持续操作时平均十秒醒一次：既有「有几率」的随机感，
/// 又不会让人敲一下键盘它就从床上掉下来。
const WAKE_ROLL_CHANCE: f32 = 0.10;

/// 最后一次键鼠操作在这个时间内（毫秒）算「人正在电脑前」。
///
/// 比 `WAKE_ROLL_EVERY` 略短：要的是「持续在动」，偶尔碰一下鼠标不算。
const INPUT_FRESH_MS: u32 = 700;

/// 接缝两侧的平均像素差超过这个值，就不硬切，改成几帧淡过去。
///
/// 这个度量是预乘 BGRA、每 16 字节采样（`FEIYU_SEAM_TRACE=1` 直接打出读数）。
/// 标尺是素材自己：自然一帧的差在 **1.5 ~ 4.1** 之间（看哪一段），
/// 而三段循环的接缝实测是：
///
/// | 接缝 | 读数 | 自然一帧 |
/// |---|---|---|
/// | sleep 156 → 104 | 5.11 | ~1.5 |
/// | idle 240 → 0 | 13.54 | ~4.1 |
/// | drag 110 → 35 | ~15（整帧灰度口径 14.86） | ~4.5 |
///
/// 所以 3.0 意味着**现在这三段每次回绕都会淡**——这是有意的：素材里根本
/// 没有无缝的循环点（见 `core/sleep.rs` 的位移曲线）。阈值留着是为了
/// 将来碰到真正首尾一致的新素材时不至于白忙一场。
///
/// 上一版阈值取 6.0，直接把 sleep 的 5.11 漏掉了（自检：淡 0 次 / 硬切 133 次）。
const SEAM_DISSOLVE_MAD: f32 = 3.0;

/// 接缝淡入用几帧。6 帧 ≈ 250ms：每步只走接缝的 1/6，
/// sleep 的 5.11 变成 0.85/帧，落回素材自身的噪声里（睡眠段自然一帧 ~1.5）。
///
/// **淡入期间帧号照常往前跑**：以前为了让混合两端都是固定图而把帧停住 6 拍，
/// 用户看到的正是「卡一下」；现在旧帧的残影在动的画面上一边推进一边淡掉
/// （电影里的交叉叠化就是这么做的），循环周期也就刚好是区间长度，不多走时间。
const SEAM_DISSOLVE_FRAMES: u32 = 6;

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
    /// 隐藏状态切换后的新状态（`true` = 已藏进托盘）。
    Hidden(bool),
    /// 用户要退出。
    Quit,
}

/// 循环接缝的淡入。
///
/// 素材里**找不到**「回绕时一模一样」的两帧：整段是画家一口气演完的 10 秒，
/// 身体尤其在一路单调地慢慢起身（见 `core/sleep.rs` 的位移曲线），
/// 任何时间段都会差着一两像素。所以接缝不硬切，用几帧把旧画面淡成新画面
/// ——每一步只走接缝的 1/6，眼睛归成「正常在动」。
///
/// `from` 是接缝前那一帧的快照，新帧则**照常逐帧推进**：
/// 画面上是「旧帧残影渐隐 + 新动画渐显」，不是停下来做一次 morph。
struct Dissolve {
    /// 接缝前一帧的像素（预乘 BGRA，和 `LayeredWindow::draw` 吃的一样）。
    from: Vec<u8>,
    /// 混合结果。复用，免得到接缝时又申请四百 KB。
    out: Vec<u8>,
    /// 已淡到第几帧 / 一共几帧。
    done: u32,
    total: u32,
}

impl Dissolve {
    /// 混合进度：`from` 占 `1 - t`，新帧占 `t`。
    fn t(&self) -> (u32, u32) {
        (self.done.min(self.total), self.total.max(1))
    }
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
    /// 左键按下的时刻。只在 `FEIYU_DRAG_TRACE` 置位时用来报「按到进拖拽」的实际耗时。
    down_at: Option<Instant>,
    /// 落地时刻。置位表示「已落地但还没回待机」，0.1 秒后清。
    landed_at: Option<Instant>,
    /// 当前动画只在这个帧区间内循环（闭区间）。`None` = 整段循环。
    /// 睡觉用它（睡着后只在打呼那一段里转，不会自己演到「醒来」），
    /// 拖拽也用它（跳过素材开头那段还站在地上的画面）。
    loop_span: Option<(u32, u32)>,
    /// 正在淡化循环接缝。`Some` 时帧号停住，画面上是两帧的混合。
    dissolve: Option<Dissolve>,
    /// 固定入睡等待（秒）。`None` = 每次回待机都随机抽。
    /// 只有 `--sleep-after` 会设它：观察入睡/自然醒这种「要等十几秒才会发生」的行为时
    /// 不能每次都靠运气。
    fixed_sleep: Option<f32>,
    /// 睡眠编排。`Some` 表示当前正在按「入睡 → 睡着」分两段播 `sleep`。
    sleep_cycle: Option<SleepCycle>,
    /// 随机源：入睡抖动、待机小动作都从这里取。
    rng: Rng,
    /// 待机小动作池。素材缺段时会被过滤瘦身，空了就不播。
    pool: Pool,
    /// 下一个小动作最早在「待机计时」走到多少秒时播（绝对值，不是间隔）。
    next_action_at: f32,
    /// 当前播的是不是待机小动作：是的话播完一圈就得回 idle。
    action_playing: bool,
    /// 正在播「醒来」那段演出。期间状态机不许把动画切走，
    /// 否则用户点一下看到的还是「突然站起来」。
    waking: bool,
    /// 上次掷「自然醒」骰子的时刻。
    wake_roll: Instant,
    /// 是否被藏进托盘。
    hidden: bool,
    /// 调用方要求的定时器周期，隐藏时用来原样恢复。
    tick_ms: u32,
    /// 托盘图标。**必须声明在 `icon` 之前**：字段按声明顺序析构，
    /// 这样 `NIM_DELETE` 一定先于 `DestroyIcon`，托盘上不会留幽灵图标。
    tray: Option<TrayIcon>,
    /// 托盘图标本体（`TrayIcon` 只引用不持有）。
    icon: Option<Icon>,
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

        // 池子要在 `manifest` 被 move 进 `pet` 之前建：字段按声明顺序求值，
        // 写到 `manifest` 之后就拿不到它了。
        let available: Vec<&str> = manifest.ids().collect();
        let pool = Pool::new(ACTION_POOL.iter().copied()).retained(&available);

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
            down_at: None,
            landed_at: None,
            loop_span: None,
            dissolve: None,
            sleep_cycle: None,
            rng: Rng::from_time(),
            pool,
            next_action_at: 0.0,
            action_playing: false,
            waking: false,
            wake_roll: Instant::now(),
            fixed_sleep: None,
            hidden: false,
            tick_ms: 0,
            tray: None,
            icon: None,
        };
        // 开局就抽一个入睡时间，别让第一天也是「满 25 秒就睡」。
        let delay = pet.next_sleep_delay();
        pet.brain.set_sleep_after(delay);
        pet.schedule_next_action();
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

    /// 启动/停止周期定时器。周期记在 `tick_ms` 里，隐藏时用它原样恢复。
    pub fn set_tick(&mut self, ms: u32) {
        self.tick_ms = ms;
        self.win.set_tick(ms);
    }

    /// 藏进托盘：窗口不可见，定时器停掉，同时把托盘图标定格成「这一刻」。
    ///
    /// 定时器一定要停——不动的桌宠也要烧 1.48% 单核，
    /// 而「藏起来」对用户的意思就是「别再占我资源」。
    pub fn hide(&mut self) {
        if self.hidden {
            return;
        }
        self.hidden = true;
        // 先把图标换成屏幕上正在显示的那一帧，再收窗口：
        // 用户看到的效果是「鱼被收进兜里，兜里露出来的就是它被收走时的样子」。
        // 失败不阻断隐藏——用户要的是「收起来」，图标长什么样是锦上添花。
        if self.tray.is_some() {
            if let Err(e) = self.refresh_tray_icon() {
                eprintln!("换托盘图标失败（照原样隐藏了）：{e:#}");
            }
        }
        self.win.hide();
        // 定时器不能完全停成 0：托盘回调这类事件进了队列要有人把泵叫醒才会被排空。
        // 1 秒一拍，CPU 量不出来（实测 10 秒增量 0.0000s）。
        self.win.set_tick(HIDDEN_TICK_MS);
    }

    /// 从托盘唤出。
    ///
    /// 没藏过也照样显一次：托盘单击的含义就是「我要看到它」。
    pub fn unhide(&mut self) {
        self.win.reveal();
        if !self.hidden {
            return;
        }
        self.hidden = false;
        if self.tick_ms > 0 {
            self.win.set_tick(self.tick_ms);
        }
        // 停定时器这段时间里 `last` 停在很久以前：不重置的话
        // 状态机会吃到几分钟的 dt，刚一睁眼就直接睡着。
        self.last = Instant::now();
    }

    /// 当前是否藏着（菜单文案与自检用）。
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// 窗口当前是否可见（自检用）。
    pub fn is_visible(&self) -> bool {
        self.win.is_visible()
    }

    /// 挂上托盘图标：用**当前正在播的那一帧**现场画一张。
    ///
    /// 只在真要常驻时才调；`--shot` 那种一闪就退的模式加了又删，
    /// 通知区会闪一下，没意义。
    pub fn enable_tray(&mut self) -> Result<()> {
        if self.tray.is_some() {
            return Ok(());
        }
        let icon = self.icon_of_current_frame()?;
        let tray =
            TrayIcon::new(self.win.hwnd(), icon.handle(), TRAY_TIP).context("添加托盘图标失败")?;
        self.icon = Some(icon);
        self.tray = Some(tray);
        Ok(())
    }

    /// 把托盘图标换成当前帧的样子。
    ///
    /// 顺序不能颠倒：**先让系统指向新图标，再让旧 `Icon` 析构**。
    /// 反过来会有一瞬系统指着一个已经 `DestroyIcon` 的句柄，通知区闪空白。
    fn refresh_tray_icon(&mut self) -> Result<()> {
        if self.tray.is_none() {
            return Ok(());
        }
        let icon = self.icon_of_current_frame()?;
        if let Some(tray) = self.tray.as_ref() {
            tray.set_icon(icon.handle())?;
        }
        // 赋值这一刻旧图标才析构，而此时系统已经指到新的上面了。
        self.icon = Some(icon);
        Ok(())
    }

    /// 当前帧的预乘 BGRA 像素（副本）。
    ///
    /// 给自检和取证用：确认「托盘图标取的是此刻」而不是写死的首帧。
    /// 返回副本而不是切片，是因为调用方接下来往往要再可变借用 `self`。
    pub fn current_frame_pixels(&mut self) -> Result<Vec<u8>> {
        let cur = self.current.clone();
        let idx = self.frame;
        Ok(self
            .anims
            .get_mut(&cur)
            .with_context(|| format!("动画 {cur} 尚未加载"))?
            .frame(idx)?
            .to_vec())
    }

    /// 用「屏幕上正在显示的那一帧」画一张图标。
    ///
    /// 取 `self.frame` 而不是第 0 帧：托盘里那个小东西才是「被暂存的大肥鱼」。
    ///
    /// 成本几乎为零：`draw_frame()` 刚刚解过这一帧，[`AnimReader`] 的 `pos` 缓存会
    /// 直接命中、返回已有缓冲。**但如果 `frame` 和 `pos` 不同步，这里会退化成
    /// 「从最近的关键帧重解整条差分链」（最坏 16 帧、~2.5ms）**，所以调用点必须
    /// 在 [`Self::draw_frame`] 之后，不能先改 `frame` 再来取。
    fn icon_of_current_frame(&mut self) -> Result<Icon> {
        let cur = self.current.clone();
        let entry = self.manifest.require(&cur)?.clone();
        let pixels = self.current_frame_pixels()?;
        Icon::from_bgra(&pixels, entry.width() as u32, entry.height() as u32)
            .context("从帧像素生成托盘图标失败")
    }

    /// 当前帧号（对当前动画取模）。自检与取证用。
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// 当前动画被限定的循环区间（闭区间）。`None` = 整段循环。
    pub fn loop_span(&self) -> Option<(u32, u32)> {
        self.loop_span
    }

    /// 是否正在淡化循环接缝。自检用：它不生效的话回绕又会变成硬切。
    ///
    /// 淡入**不冻结帧号**，所以这个标志为真的那些拍里帧号仍在前进。
    pub fn seam_dissolving(&self) -> bool {
        self.dissolve.is_some()
    }

    /// 重设「多久不互动就睡」。给自检和将来的设置项用。
    pub fn set_sleep_after(&mut self, secs: f32) {
        self.brain.set_sleep_after(secs);
    }

    /// 把入睡等待钉成固定值（秒）。`--sleep-after` 与取证用。
    ///
    /// 不钉住不行：每次回到待机都会重抽 18~38 秒，
    /// 要观察「入睡了之后被键鼠弄醒」，不能每次都靠运气等到它自己困。
    pub fn set_fixed_sleep_after(&mut self, secs: f32) {
        self.fixed_sleep = Some(secs);
        self.brain.set_sleep_after(secs);
    }

    /// 直接推进 `n` 帧，不碰墙钟。**只给自检用**：
    /// 真按 24fps 演完入睡段要等 7 秒，自检等不起。
    ///
    /// 返回是否因为「小动作播完一圈」而切回了 idle。
    pub(crate) fn advance_frames(&mut self, n: u32) -> Result<bool> {
        let mut went_idle = false;
        for _ in 0..n {
            if self.step_frames(1)? {
                went_idle = true;
            }
        }
        Ok(went_idle)
    }

    /// 待机小动作池当前的 `(id, 权重)`。自检用。
    pub fn action_pool(&self) -> &[(&'static str, u32)] {
        self.pool.items()
    }

    /// 把下一个小动作提前到现在。**只给自检用**（真实间隔 8~15 秒，测不起）。
    pub(crate) fn trigger_next_action(&mut self) {
        self.next_action_at = 0.0;
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
        // 换段就丢掉上一段的区间限制：否则从「睡着」切到「开心」时，
        // 新动画会莫名其妙只播前 70%。淡入也是上一段的事，一并丢掉——
        // 不然新段的头几帧会跟旧段的画面混在一起。
        self.loop_span = None;
        self.dissolve = None;
        self.sleep_cycle = None;
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
            self.after_switch(id)?;
        }

        // 待机自娱自乐：够无聊了就抽个小动作。
        // 条件里特意看 `elapsed`（而不是另起一个计时器）：小动作不重置它，
        // 所以自己能玩几下，但该睡的时候照样会困。
        if self.forced.is_none()
            && self.brain.mood() == Mood::Idle
            && !self.action_playing
            && self.brain.elapsed() >= self.next_action_at
        {
            if let Some(id) = self.pool.pick(&mut self.rng) {
                self.action_playing = true;
                self.play(id)?;
            } else {
                // 池子空（素材缺段）：往后顺延，别每帧都重试。
                self.schedule_next_action();
            }
        }

        // 睡着时：电脑前的人一直在动，就有概率把它吵醒。
        // 只看**持续输入**——偶尔划一下鼠标只会推迟入睡（`Attention`），不会弄醒它。
        if self.forced.is_none() && self.brain.mood() == Mood::Sleep && !self.waking {
            let idle_ms = feiyu_platform::last_input_ms();
            let fresh = idle_ms.is_some_and(|ms| ms < INPUT_FRESH_MS);
            if fresh && now.duration_since(self.wake_roll).as_secs_f32() >= WAKE_ROLL_EVERY {
                self.wake_roll = now;
                let roll = self.rng.unit();
                // 取证开关：`FEIYU_WAKE_TRACE=1` 时把每次掷骰的输入间隔与点数打出来。
                // 「有几率自然醒」肉眼根本判不了：没有这行，一次没醒就分不清
                // 是「运气不济（10% 没中）」还是「输入根本没被读到」。
                if std::env::var_os("FEIYU_WAKE_TRACE").is_some() {
                    eprintln!(
                        "自然醒判定：输入在 {}ms 前，掷 {roll:.3}（<{WAKE_ROLL_CHANCE:.2} 则醒）",
                        idle_ms.unwrap_or(u32::MAX)
                    );
                }
                if roll < WAKE_ROLL_CHANCE {
                    self.wake_up()?;
                }
            }
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
            } else if self.brain.mood() == Mood::Dragged {
                // 落地了，但先别急着站起来：记下时刻，由下面那段等 0.1 秒。
                self.landed_at = Some(now);
            }
        }

        // 落地后再保持被拖拽姿态 `LANDING_HOLD` 秒才回待机。
        // 从高处松手时鱼一直在掉，`motion` 要落地才 `rest`，
        // 所以这段等待不会把「还在空中」误判成「已经停好」。
        if let Some(t) = self.landed_at {
            if now.duration_since(t).as_secs_f32() >= LANDING_HOLD {
                self.landed_at = None;
                self.stimulate(Stimulus::DragEnd)?;
            }
        }

        let anim_dt = dt.min(MAX_ANIM_DT);
        self.acc += anim_dt * self.manifest.fps;
        if self.acc < 1.0 {
            return Ok(());
        }
        let step = self.acc.floor();
        self.acc -= step;
        // 推进帧 + 处理「小动作播完」。返回 `true` 时 `play` 已经重画过画面。
        if self.step_frames(step as u32)? {
            return Ok(());
        }
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
                self.landed_at = None;
                self.trail.clear();
                self.down_at = Some(Instant::now());
                // 预热：按住就是想拖。把文件先打开，等阈值一到就只剩解一帧。
                // 失败不管——真拖起来时 `play()` 会再报一次，没必要在按下时就报错。
                let _ = self.ensure_loaded("drag");
                Ok(())
            }
            Event::Moved { x, y } => self.on_moved(x, y),
            Event::MouseUp { .. } => self.on_button_up(),
            // 鼠标在它身上划过：不叫醒，只推迟下一次入睡。
            Event::MouseMove { .. } => self.stimulate(Stimulus::Attention),
            Event::RightClick { .. } | Event::Quit => Ok(()),
            // 托盘点击与唤醒由 app 层处理（它们要改可见性、开菜单），
            // 这里只保证穷举匹配不被绕过去。
            Event::TrayLeftClick | Event::TrayRightClick { .. } | Event::Woken => Ok(()),
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
            // 取证开关：`FEIYU_DRAG_TRACE=1` 时把「按下 → 进入拖拽姿态」的真实耗时
            // 打到 stderr。体感延迟是用户报的，软件拿不出数字就只能猜。
            if std::env::var_os("FEIYU_DRAG_TRACE").is_some() {
                if let Some(t) = self.down_at {
                    eprintln!(
                        "拖拽：按下 → 进入 drag 姿态 {:.1}ms",
                        t.elapsed().as_secs_f64() * 1e3
                    );
                }
            }
            self.stimulate(Stimulus::DragStart)?;
        }
        Ok(())
    }

    /// 左键抬起：真拖过就「甩出去 + 落地」，没拖过就算点了一下。
    fn on_button_up(&mut self) -> Result<()> {
        let was_dragging = std::mem::replace(&mut self.dragging, false);
        self.grab_origin = None;
        self.down_at = None;
        if !was_dragging {
            self.trail.clear();
            return self.clicked();
        }
        // 拖到哪就从哪开始：先把锚点对回实际窗口位置，再据此起飞。
        self.capture_anchor()?;
        let (vx, vy) = self.take_fling();
        self.motion = Some(Motion::fling(vx, vy));
        // 这里**不**切回待机：鱼还在空中，保持被拖拽的姿态直到落地
        // （见 `tick` 里的 `landed_at`）。松手瞬间就站好会显得像被剪了一帧。
        Ok(())
    }

    /// 点了一下。睡着时要先播「慢慢醒来」，其余情况交给状态机。
    fn clicked(&mut self) -> Result<()> {
        if self.brain.mood() == Mood::Sleep {
            return self.wake_up();
        }
        self.stimulate(Stimulus::Click)?;
        // 开心 / 生气是**一次性反应**，不是一种常态：标记成「演一圈」的模式，
        // 播完一圈由 `step_frames` 收回待机。不标的话 `click_happy` 会一直循环
        // ——用户看到的「点一下之后动画一直在重复」就是它。
        if matches!(self.brain.mood(), Mood::Happy | Mood::Pout) {
            self.action_playing = true;
        }
        Ok(())
    }

    /// 开始一次「醒来」：从**当前帧**接着演到最后一帧，播完回待机。
    ///
    /// 不直接 `stimulate(Click)`：那会切到 `click_happy`，观众看到的是
    /// 鱼从躺着一下跳到站着笑，像被剪了一帧。素材里本来就有睁眼起身的过程，
    /// 不花额外成本就能把它演完。
    ///
    /// 也不能从固定的第 170 帧起播（旧实现）：睡着时 Z 正挂在中间，一下跳到
    /// Z 已经消失的 170，画面差实测 11.7——比循环接缝还大一倍，用户说的「闪一下」
    /// 就是它。从当前帧往后演，Z 会在 149~173 自己飘散掉，一帧都没跳过。
    ///
    /// 睡到一半（`Falling`，还闭着眼往下沉）被点则直接醒：那时跳到起身段反而突兀。
    fn wake_up(&mut self) -> Result<()> {
        if self.waking || self.brain.mood() != Mood::Sleep {
            return Ok(());
        }
        // 当前帧换成比例：`span_to_frames` 的 round 是精确的，传进去多少换回来还是多少。
        let count = self.current_count();
        let from = match count.checked_sub(1) {
            Some(last) if last > 0 => self.frame.min(last) as f32 / last as f32,
            _ => 0.0,
        };
        match self.sleep_cycle.as_mut() {
            Some(cycle) if cycle.stage() == SleepStage::Asleep => cycle.start_waking(from),
            _ => return self.stimulate(Stimulus::Awaken),
        }
        self.apply_sleep_span();
        // 正在淡接缝时被点醒：淡入的两端已经不相关了，直接收掉。
        self.dissolve = None;
        self.frame = self.loop_span.map_or(0, |(lo, _)| lo);
        self.waking = true;
        // 复用「播完一圈就收尾」那条路：醒来段不回绕，一落到段尾就交回 `step_frames`。
        self.action_playing = true;
        self.draw_frame()
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
            dissolve,
            ..
        } = self;
        let anim = anims
            .get_mut(current)
            .with_context(|| format!("动画 {current} 尚未加载"))?;
        let idx = *frame % anim.frame_count().max(1);
        let pixels = anim
            .frame(idx)
            .with_context(|| format!("解 {current} 第 {idx} 帧失败"))?;
        match dissolve {
            // 接缝淡入：把接缝前那一帧和**当前正在推进的新帧**按进度混合。
            //
            // 旧帧只在这里出现（作为残影），画面上每一步都在动：
            // 停住帧号做过一版，用户看到的就是「卡一下」。
            Some(d) => {
                let (num, den) = d.t();
                blend_frame(&d.from, pixels, num, den, &mut d.out);
                win.draw(entry.width() as u32, entry.height() as u32, &d.out)?;
            }
            _ => {
                win.draw(entry.width() as u32, entry.height() as u32, pixels)?;
            }
        }
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
    /// 桌宠自身的右键和托盘图标的右键**共用同一张菜单**：两边的动作完全一样，
    /// 只有第一项随「当前藏没藏」换文案。分开维护两张表，只会让两边慢慢长歪。
    pub fn menu(&mut self, x: i32, y: i32) -> Result<Action> {
        let menu = self.build_menu()?;
        let Some(cmd) = menu.show_at(self.win.hwnd(), x, y)? else {
            return Ok(Action::None);
        };
        self.dispatch(cmd)
    }

    /// 菜单项数（自检用：只核对内容没被写坏，不弹出来）。
    pub fn menu_items(&self) -> Result<usize> {
        Ok(self.build_menu()?.items())
    }

    /// 菜单首项文案：藏着时是「唤出」，显示着时是「隐藏」。
    pub fn hide_label(&self) -> &'static str {
        if self.hidden {
            "唤出桌宠"
        } else {
            "隐藏到托盘"
        }
    }

    fn build_menu(&self) -> Result<Menu> {
        let topmost = self.win.is_topmost();
        let mut menu = Menu::new()?;
        menu.command(CMD_HIDE, self.hide_label())
            .command(CMD_HOME, "回到屏幕底部")
            .check(CMD_TOPMOST, "总在最前", topmost)
            .separator()
            .command(CMD_QUIT, "退出");
        Ok(menu)
    }

    fn dispatch(&mut self, cmd: CommandId) -> Result<Action> {
        match cmd {
            CMD_HIDE => {
                let next = !self.hidden;
                if next {
                    self.hide();
                } else {
                    self.unhide();
                }
                Ok(Action::Hidden(next))
            }
            CMD_HOME => {
                self.back_to_home()?;
                Ok(Action::Home)
            }
            CMD_TOPMOST => {
                // 重读一次而不是拿弹菜单前的快照：菜单是模态的，
                // 中间可能已经改过了。
                let next = !self.win.is_topmost();
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

    /// 下一次入睡前要安静多久（秒）。
    ///
    /// 每次回到待机都重新抽：固定值会让「它什么时候睡」变成可预测的定时器。
    fn next_sleep_delay(&mut self) -> f32 {
        // 钉过就照钉的来：取证时不能每轮都抽到不同的等待时间。
        if let Some(secs) = self.fixed_sleep {
            return secs;
        }
        self.rng.range(SLEEP_AFTER_MIN, SLEEP_AFTER_MAX)
    }

    /// 安排下一次自娱自乐。
    ///
    /// 存的是**绝对时刻**而不是间隔：`elapsed` 会被鼠标划过清零，
    /// 存绝对值就自然跟着顺延，不用在清零处到处补一行。
    fn schedule_next_action(&mut self) {
        let now = self.brain.elapsed();
        self.next_action_at = now + self.rng.range(ACTION_GAP_MIN, ACTION_GAP_MAX);
    }

    /// 换段之后的收尾。
    ///
    /// 三件事只有这里知道：
    /// * 落到 `sleep` 上要起睡眠编排（它从帧 0 演，但只演到冒出 Z）；
    /// * 落到 `drag` 上要跳掉素材开头那段「还站在地上」的画面；
    /// * 回到 `idle` 要换一个新的入睡时间。
    fn after_switch(&mut self, id: &str) -> Result<()> {
        // 任何真正的姿态切换都结束「待机小动作」这场戏。
        self.action_playing = false;
        if id == "drag" {
            // 只播悬空段：素材前 35 帧人还站在地上，用户报的「拖一两秒才漂浮」就是它。
            self.loop_span = Some((DRAG_LO, DRAG_HI));
            self.frame = DRAG_LO;
            return self.draw_frame();
        }
        if id == "sleep" {
            self.sleep_cycle = Some(SleepCycle::new());
            self.apply_sleep_span();
            self.frame = self.loop_span.map_or(0, |(lo, _)| lo);
            return self.draw_frame();
        }
        if id == "idle" {
            let delay = self.next_sleep_delay();
            self.brain.set_sleep_after(delay);
            self.schedule_next_action();
        }
        Ok(())
    }

    /// 按睡眠编排的当前阶段重算循环区间。
    fn apply_sleep_span(&mut self) {
        let Some(cycle) = self.sleep_cycle.as_ref() else {
            return;
        };
        let count = self.current_count();
        self.loop_span = Some(span_to_frames(cycle.span(), count));
    }

    /// 推进 `step` 帧，并处理「待机小动作播完一圈」。
    ///
    /// `tick` 与自检共用这一段：回绕判断只写一份，两边不会慢慢长歪。
    /// 返回 `true` 表示刚因为小动作演完而切回了 idle（此时画面已经重画过）。
    fn step_frames(&mut self, step: u32) -> Result<bool> {
        let count = self.current_count();
        let before = self.frame;
        self.advance_frame(step, count);
        // `step` 最多两三帧（`MAX_ANIM_DT` 卡着），所以帧号变小只可能是回绕，
        // 不会漏跳一整圈。
        if !self.action_playing || self.frame >= before {
            return Ok(false);
        }
        self.action_playing = false;
        self.schedule_next_action();
        if self.waking {
            // 醒来那段播完：由状态机回到待机，不走 `click_happy`。
            self.waking = false;
            self.stimulate(Stimulus::Awaken)?;
            return Ok(true);
        }
        // 点击反馈（开心 / 生气）只演一遍：一圈播完就把心情交回待机。
        // 素材里这两段都是 10 秒的循环片段，不收回就会一直重复播。
        if matches!(self.brain.mood(), Mood::Happy | Mood::Pout) {
            self.stimulate(Stimulus::Awaken)?;
            return Ok(true);
        }
        self.play("idle")?;
        Ok(true)
    }

    /// 推进 `step` 帧。
    ///
    /// 没有被限定区间时就是普通的整段循环；被限定（睡觉）时只在段内转，
    /// 段播完再问一次睡眠编排要不要换段——这是「睡着后不再自己醒来」的实现点。
    fn advance_frame(&mut self, step: u32, count: u32) {
        // 淡入只推进进度，**不拦帧号**：帧照常往前走，旧帧的残影逐拍淡掉。
        // 把帧停住做过一版，用户直接看到了「卡一下」。
        self.tick_dissolve();
        let Some((lo, hi)) = self.loop_span else {
            let next = (self.frame + step) % count;
            if next < self.frame {
                self.start_seam_dissolve(self.frame, next);
            }
            self.frame = next;
            return;
        };
        if lo > hi || hi >= count {
            // 区间坏了（换了素材、帧数变了）：退回整段循环，
            // 不要卡在第 0 帧不动——那比多播几帧难查得多。
            self.loop_span = None;
            self.frame = (self.frame + step) % count;
            return;
        }
        let span = hi - lo + 1;
        if self.frame < lo || self.frame > hi {
            self.frame = lo;
        }
        let next = self.frame.saturating_add(step);
        if next <= hi {
            self.frame = next;
            return;
        }
        // 越过段尾：入睡段演完就进打呼段，打呼段就一直原地循环。
        if self
            .sleep_cycle
            .as_mut()
            .is_some_and(|c| c.on_segment_end())
        {
            // 换段时把帧号**夹进**新区间，而不是一律拨回段首：
            // 入睡段停在 133，而 133 本来就在打呼段的 76~174 里，
            // 拨回 76 等于倒退 57 帧（Z 会缩一下）。
            self.apply_sleep_span();
            if let Some((a, b)) = self.loop_span {
                self.frame = self.frame.clamp(a, b);
            } else {
                self.frame = lo;
            }
        } else {
            let next_frame = lo + (next - hi - 1) % span;
            // 段尾接段首：整个播放器里唯一一次大跳。跳得狠就淡过去。
            self.start_seam_dissolve(self.frame, next_frame);
            self.frame = next_frame;
        }
    }

    /// 推进一拍接缝淡入。帧号归帧号，这里只管进度。
    fn tick_dissolve(&mut self) {
        let Some(d) = self.dissolve.as_mut() else {
            return;
        };
        d.done += 1;
        if d.done >= d.total {
            // 淡完了：之后画的就是新帧本身（`draw_frame` 会原样画出来）。
            self.dissolve = None;
        }
    }

    /// 段尾要接回段首时调一次：接缝够硬就准备淡过去。
    ///
    /// `from` 是刚放完的那一帧，`to` 是下一拍要显示的那一帧。
    fn start_seam_dissolve(&mut self, from: u32, to: u32) {
        // 醒来段播完也要回绕，但那是「醒完了」，接下来就回待机：
        // 淡化只会把「站起来」的最后一步糊掉。
        if self.waking {
            return;
        }
        let Some(prev) = self.snapshot_frame(from) else {
            return;
        };
        let Some(next) = self.snapshot_frame(to) else {
            return;
        };
        let mad = seam_mad(&prev, &next);
        // 取证开关：不管淡不淡都先把接缝读数打出来。只打「淡了」的话，
        // 没淡（阈值没进）情况下日志里什么都看不到，就会误以为接缝是好的。
        if std::env::var_os("FEIYU_SEAM_TRACE").is_some() {
            eprintln!(
                "接缝 {} → {}（{}）：mad {:.2}，{}",
                from,
                to,
                self.current,
                mad,
                if mad >= SEAM_DISSOLVE_MAD {
                    format!("{} 帧淡过去", SEAM_DISSOLVE_FRAMES)
                } else {
                    "硬切".to_owned()
                }
            );
        }
        if mad < SEAM_DISSOLVE_MAD {
            return;
        }
        self.dissolve = Some(Dissolve {
            from: prev,
            out: next,
            // 从 1/6 起淡，不从 0/6：`t = 0` 等于再把旧帧画一遍，
            // 白占一拍（旧帧刚画过），而且新段的第一帧会被整拍跳过。
            done: 1,
            total: SEAM_DISSOLVE_FRAMES,
        });
    }

    /// 取某一帧的像素副本。接缝要用它把「刚过去的那个画面」留在手里。
    fn snapshot_frame(&mut self, idx: u32) -> Option<Vec<u8>> {
        let anim = self.anims.get_mut(&self.current)?;
        let idx = idx % anim.frame_count().max(1);
        anim.frame(idx).ok().map(<[u8]>::to_vec)
    }

    /// 把刺激交给状态机；姿态真的变了才换动画。
    fn stimulate(&mut self, s: Stimulus) -> Result<()> {
        if self.forced.is_some() {
            return Ok(());
        }
        if self.brain.stimulus(s) {
            let id = self.brain.anim_id();
            self.play(id)?;
            self.after_switch(id)?;
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
        let started = Instant::now();
        let anim = AnimReader::open(&path)
            .with_context(|| format!("打不开动画 {}（id={id}）", path.display()))?;
        // 取证开关：首次加载要读文件 + 解 zstd + 建差分索引，
        // 如果它贵到几百毫秒，“提起来不跟手”就不在拖拽逻辑而在这一行。
        if std::env::var_os("FEIYU_DRAG_TRACE").is_some() {
            eprintln!("加载 {id}：{:.1}ms", started.elapsed().as_secs_f64() * 1e3);
        }
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

/// 两帧的平均像素差。用来判断一个循环接缝硬不硬。
///
/// 每 16 个字节采一个样（一行 384 像素 = 1536 字节，即一行采 96 点），
/// 二十四万多个采样点，四百 KB 的帧算一遍不到 0.1ms——只在接缝处跑一次。
fn seam_mad(a: &[u8], b: &[u8]) -> f32 {
    let (mut sum, mut n) = (0u64, 0u64);
    for (x, y) in a.iter().zip(b).step_by(16) {
        sum += u64::from((*x).abs_diff(*y));
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum as f32 / n as f32
    }
}

/// 把 `from` 和 `to` 按 `num/den` 线性混合，写进 `out`。
///
/// 帧像素是**预乘** BGRA，逐通道线性插值就是正确的那种混合（预乘之后
/// 透明度已经乘进颜色里，插值不会出现半透明边缘发黑）。
fn blend_frame(from: &[u8], to: &[u8], num: u32, den: u32, out: &mut Vec<u8>) {
    let den = den.max(1);
    let num = num.min(den);
    out.clear();
    out.extend(to.iter().enumerate().map(|(i, b)| {
        // 长度不一致时（换了素材还没重画）就以新帧为准，不越界。
        let a = u32::from(from.get(i).copied().unwrap_or(*b));
        let b = u32::from(*b);
        ((a * (den - num) + b * num) / den) as u8
    }));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 混合的两端必须精确落在原帧上：`t=0` 是接缝前那帧，`t=1` 是新帧。
    /// 任一端偏一点，接缝处就还是能看到一下跳。
    #[test]
    fn blend_hits_both_ends_exactly() {
        let from = [0u8, 10, 200, 255];
        let to = [200u8, 10, 0, 255];
        let mut out = Vec::new();

        blend_frame(&from, &to, 0, 6, &mut out);
        assert_eq!(out, from.to_vec(), "t=0 必须原样画出旧帧");

        blend_frame(&from, &to, 6, 6, &mut out);
        assert_eq!(out, to.to_vec(), "t=1 必须原样画出新帧");

        blend_frame(&from, &to, 3, 6, &mut out);
        assert_eq!(out, vec![100, 10, 100, 255], "中间取中点");
    }

    /// 长度不一致时以新帧为准，不许越界、不许 panic。
    #[test]
    fn blend_survives_size_mismatch() {
        let from = [1u8, 2];
        let to = [7u8, 8, 9];
        let mut out = Vec::new();
        blend_frame(&from, &to, 0, 4, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2], 9, "缺的那一端直接取新帧");
    }

    /// 同一帧的差必须是 0；差得越多读数越大——阈值 6.0 才靠得住。
    #[test]
    fn seam_mad_measures_distance() {
        let a: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        assert_eq!(seam_mad(&a, &a), 0.0);

        let mut b = a.clone();
        for (i, v) in b.iter_mut().enumerate() {
            if i % 2 == 0 {
                *v = v.saturating_sub(32);
            }
        }
        assert!(seam_mad(&a, &b) > SEAM_DISSOLVE_MAD, "差 32 已经远超阈值");
    }
}
