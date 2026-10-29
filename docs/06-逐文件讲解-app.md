# 06 逐文件讲解：feiyu-app（组装层）

这一层把 core 的纯逻辑和 platform 的窗口操作组装成一只真的桌宠。

```
crates/feiyu-app/src/
├── lib.rs       模块导出 + cli_main
├── cli.rs       命令行、演示模式、消息泵回调（两个 exe 共用）
├── main.rs      GUI 入口（双击，无终端）
├── bin/feiyu-cli.rs  console 入口（诊断）
├── pet/
│   ├── mod.rs       Pet 结构体、open、tick、事件、渲染、托盘、几何
│   ├── clips.rs     动画参数表 / 分类 / 随机池
│   ├── menu.rs      右键菜单与点播
│   ├── walk.rs      走路
│   ├── follow.rs    跟随鼠标转头
│   ├── typing.rs    打字联动
│   ├── sleep.rs     自然醒与睡眠区间
│   └── pixels.rs    镜像 / 接缝 / 混合
└── selftest.rs  94 项真机验收
```

---

# 一、`cli.rs`：命令行与模式

## 1.1 `Args` 与 `parse_args`

```rust
struct Args {
    dir: Option<PathBuf>,
    config: Option<PathBuf>,
    anim: Option<String>,
    shot: Option<PathBuf>,
    ...
    showcase: bool,
    showcase_only: Option<String>,
    ...
}
```

所有参数都先收进 `Args`，再决定走哪条路径。为什么不用第三方 CLI 库？
参数就二十来个，手写解析反而更好读，也少一个依赖。

手写解析的固定套路：

```rust
let mut it = argv.iter();
while let Some(arg) = it.next() {
    match arg.as_str() {
        "--dir" => out.dir = Some(PathBuf::from(it.next().context("--dir 缺少路径")?)),
        "--showcase" => {
            out.showcase = true;
            if let Some(next) = it.clone().next() {
                if !next.starts_with("--") {
                    out.showcase_only = Some(next.clone());
                    it.next();
                }
            }
        }
        ...
        other => bail!("未知参数 {other}（--help 查看用法）"),
    }
}
```

- `it.clone().next()` 是“偷看下一个参数”，用来实现 `--showcase [分类]` 这种可选参数；
- `context()` 把“没给值”变成一条人话错误；
- 未知参数**直接报错**，不静默忽略——否则打错一个字母，程序行为会悄悄不同。

## 1.2 模式分派

`run()` 的顺序：

```rust
let args = parse_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
let dir = args.dir.clone().unwrap_or_else(default_anim_dir);
let cfg = load_config(args.config.as_deref())?;

if args.selftest { ...; return }
if args.list { return list_anims(&dir, &cfg); }
if let Some((id, out, step)) = &args.dump { return dump_frames(...); }
run_pet(args, dir, cfg)
```

`load_config` 的优先级：`--config` > exe 旁边 `config.json` > 当前目录 `config.json` > 默认值。
显式给了路径却读不了就报错；自动找到但 JSON 写坏了只告警并回退——用户明确指定的东西
不能默默忽略，但坏配置不能害得桌宠起不来。

`run_pet` 里：

```rust
let forced = args.anim.clone()
    .or_else(|| args.shots.as_ref().map(|_| "idle".into()))
    .or_else(|| args.showcase.then(|| "idle".into()));
let mut pet = Pet::open(&dir, forced.as_deref(), cfg)?;
pet.show();
```

`forced` 的含义是“冻结状态机，只播指定动画”。截图、逐段演示都需要它，
否则桌宠会在演示途中自己睡着/自己换动作。

## 1.3 消息泵回调

```rust
pump(|ev| {
    if let Some(d) = deadline { if now >= d { quit(); return; } }
    if menu_event(ev, &mut pet, verbose) { return; }
    if matches!(ev, Event::TrayLeftClick | Event::Woken) { pet.unhide(); return; }
    if let Err(e) = pet.on_event(ev) { ...; quit(); return; }
    if verbose && pet.current() != last { println!("[{:?}s] → {last}", started.elapsed()); }
})
```

顺序很重要：

1. 先判截止时间（`--run` 用墙钟，不数拍子，因为 `SetTimer` 粒度 15.6ms）；
2. 菜单事件单独处理（它要弹模态菜单，不能进 `Pet::on_event`）；
3. 托盘左键/唤醒 = 唤出；
4. 其余交给 `Pet::on_event`；
5. `verbose` 只在姿态真的变化时打印，并带时间戳——否则日志会被 Tick 刷屏。

## 1.4 `showcase`

```rust
pet.demo_clip(&id)?;
let frames = pet.frame_count().max(1);
let secs = frames as f32 / pet.fps().max(1.0);
let started = Instant::now();
while started.elapsed().as_secs_f32() < secs {
    std::thread::sleep(Duration::from_millis(10));
    pet.tick()?;
}
```

为什么用“睡 10ms + tick”而不是 `pet.tick()` 忙等？因为 `tick` 用真实 `dt`
推进帧号，忙等会让 `dt` 极小、`acc` 永远攒不够一帧。睡 10ms 是让出 CPU，
同时保证 24fps 的节奏。

`--showcase 移动` 会按分类过滤；分类名写错时列出全部分类，避免用户猜。

## 1.5 `typing_probe`

```rust
pet.set_tick(200);
pump(|ev| {
    if ev != Event::Tick { return; }
    total += feiyu_platform::typing_keys();
    ...
})
```

只统计键盘按键次数，不跑 `pet.tick()`。它的存在是为了证明“原始输入真的收到了”，
而不是只证明“登记函数返回了 Ok”。

## 1.6 `list_anims`

按分类分组打印：分类名、**配置生效后的**随机权重、每段的尺寸/帧数/体积/中文名。
不建窗口，纯读清单，所以可以在任何终端里跑。这个输出是“100 段到底怎么被用到”的
权威说明。

---

# 二、`pet/`：一只桌宠的全部状态

拆之前是一个约 2700 行的 `pet.rs`。现在按“改什么的时候要一起改”拆成 8 个文件：

| 文件 | 管什么 | 什么时候动它 |
|---|---|---|
| `mod.rs` | `Pet` 字段、`open`、`tick`、事件、渲染、托盘、几何 | 改主循环或窗口行为 |
| `clips.rs` | 每段的帧区间/朝向/步频、分类、随机池权重 | 加素材、调权重、修前摇 |
| `menu.rs` | `Action`、菜单构建、命令派发、点播 | 加菜单项/改点播规则 |
| `walk.rs` | `Walk` 和位移推进 | 调步频、贴墙、走路打断 |
| `follow.rs` | 转头看鼠标的帧区间与冷却 | 调跟随手感 |
| `typing.rs` | 打字热度、触发、冷却、工作段权重 | 调打字联动 |
| `sleep.rs` | 自然醒掷骰、睡眠区间 | 调入睡/醒来 |
| `pixels.rs` | 镜像、接缝距离、两帧混合 | 改像素算法 |

`mod.rs` 仍然是最大的一个（约 1600 行），因为它装着 `Pet` 的字段和主循环；
再往下拆的前提是先把“播放器/事件/托盘”这三块也独立成模块，而不是按行数硬切。

## 2.1 字段分组

```rust
pub struct Pet {
    win: LayeredWindow,              // 窗口
    manifest: Manifest,              // 清单
    cfg: Config,                     // 用户配置（只读）
    anims: HashMap<String, Anim>,    // 已打开的动画（有上限）
    current: String, frame: u32, acc: f32,   // 当前动画与播放位置
    anchor_x: i32, feet_y: i32,      // 锚点（画布坐标）
    brain: Brain,                    // 心情状态机
    loop_span: Option<(u32, u32)>,   // 只在这一段里循环
    dissolve: Option<Dissolve>,      // 接缝淡入
    sleep_cycle: Option<SleepCycle>, // 睡眠编排
    walking: Option<Walk>,           // 正在走的一段路
    dragging: bool, motion: Option<Motion>, trail: VecDeque<_>, // 拖拽/物理
    follow/click_walk/typing_enabled: bool,     // 菜单开关
    typing_heat/typing_next/typing_chance: ..., // 打字联动
    hidden: bool, tray/icon: ...,    // 托盘
    ...
}
```

**一个字段只表达一件事**。`brain` 管“想干什么”，`loop_span` 管“播哪一段”，
`action_playing` 管“这段演完要不要收尾”。混在一起就会出现“睡着时被随机动作打断”
这类 bug。

## 2.2 `open`：启动顺序

```rust
let manifest = Manifest::load(...)?;
let entry = manifest.require(&first)?.clone();
let area = primary_work_area()?;
let anchor_x = (area.0 + area.2) / 2;
let feet_y = area.3 - BOTTOM_GAP;
let (x, y) = window_pos(manifest.canvas.width, &entry, anchor_x, feet_y);
let win = LayeredWindow::create(entry.width(), entry.height(), x, y, "飞鱼")?;
enable_typing(win.hwnd())?;   // 失败只丢一个功能
let pool = Pool::new(pool_items(&cfg)).retained(&available);
let mut pet = Self { cfg, ... };
pet.schedule_next_action();
pet.ensure_loaded(&pet.current)?;
pet.draw_frame()?;
```

顺序的讲究：

- 先算窗口位置再建窗口，避免先在 (0,0) 闪一下再跳过去；
- 池子要在 `manifest` 被 move 进 `Pet` 之前建（字段按声明顺序求值）；
- `cfg` 在 `open` 之前读好，`Pet` 只读不写；改配置要重启。
- 先抽入睡时间、排随机动作，再画第一帧。

## 2.3 `tick`：每 8ms 的决策顺序

```rust
let dt = now - last;
if brain.advance(dt) { play(...); after_switch(...); }   // ① 睡觉
if 待机 && 到点 && !action_playing { 抽随机动作 }          // ② 自娱自乐
if follow && 待机 { poll_look(mx) }                       // ③ 转向跟随
if click_walk && 待机 { 检测左键按下沿 }                    // ④ 点哪走哪
if 睡着 && 有持续输入 { 每秒掷 10% 自然醒 }                // ⑤ 自然醒
if !forced { note_typing(typing_keys(), dt, now) }        // ⑥ 打字联动
if let Some(motion) { motion.step(...); place_window(); } // ⑦ 物理
if landed_at 到 0.1 秒 { DragEnd }                        // ⑧ 落地收尾
acc += dt * fps; if acc < 1 { return }                    // ⑨ 帧节拍
step_frames(step); draw_frame()                            // ⑩ 推进并提交
```

为什么是这个顺序？

- ①②③④⑤⑥ 都可能改变“该播什么”，必须在推进帧之前；
- ⑦ 物理要先算位置，再摆窗口，本帧的落点才是最终结果；
- ⑧ 落地保持必须在物理之后；
- ⑩ 最后统一推进帧并提交，一帧只提交一次。

## 2.4 `on_event`：点击与拖拽

```rust
Event::MouseDown => {
    self.grab_origin = Some(win.rect().0, .1);
    self.dragging = false;
    self.landed_at = None;
    self.trail.clear();
    let _ = self.ensure_loaded("drag");   // 预热
}
Event::Moved { x, y } => self.on_moved(x, y),
Event::MouseUp => self.on_button_up(),
Event::MouseMove { .. } => self.stimulate(Stimulus::Attention),
```

按下时**预热** `drag`：真正开始拖时只剩解一帧，用户感觉“提起来就飘”。

`on_moved` 用两轴最大位移判断是否超过 `DRAG_THRESHOLD = 2`：
1 像素手抖仍算点击，2 像素以上才算拖拽。

`on_button_up` 分两条路：

- 没拖过 → `clicked()`；
- 拖过 → 用最近 120ms 的采样估速度 → `Motion::fling` → 等落地。

## 2.5 `clicked` / `wake_up`

```rust
fn clicked(&mut self) -> Result<()> {
    if self.brain.mood() == Mood::Sleep { return self.wake_up(); }
    self.stimulate(Stimulus::Click)?;
    if matches!(self.brain.mood(), Mood::Happy | Mood::Pout) {
        self.action_playing = true;
    }
    Ok(())
}
```

`action_playing = true` 是“点击反应只演一圈”的关键。不标的话，`click_happy` 是
10 秒循环片段，会一直重复——用户报过“被点击后的动画一直在重复”。

`wake_up` 从**当前帧**换成比例，传给 `SleepCycle::start_waking(from)`：
醒来段不回绕，从当前帧往后演，Z 自己飘散，不会“闪一下”。

## 2.6 `stimulate` 与 5 段点击回应

```rust
fn stimulate(&mut self, s: Stimulus) -> Result<()> {
    if self.forced.is_some() { return Ok(()); }
    if self.brain.stimulus(s) {
        let id = if self.brain.mood() == Mood::Happy {
            self.pick_click()
        } else {
            self.brain.anim_id()
        };
        self.play(id)?;
        self.after_switch(id)?;
    }
    Ok(())
}
```

状态机只回答“该开心了”，具体播哪一段由 `pick_click` 从 5 段里随机抽，
并且不连抽同一个。这是“语义”和“素材”分离的例子。

## 2.7 `play`：换段

```rust
pub fn play(&mut self, id: &str) -> Result<()> {
    if id == self.current && self.anims.contains_key(id) {
        return Ok(());        // ① 同一段：早退
    }
    self.ensure_loaded(id)?;  // ② 按需打开文件
    let entry = self.manifest.require(id)?.clone();
    self.current = id.to_owned();
    self.frame = 0; self.acc = 0.0;
    self.loop_span = None; self.dissolve = None;
    self.walk = None; self.flip = false;
    self.sleep_cycle = None;
    self.place_window(&entry);  // ③ 先摆位置再画
    self.draw_frame()
}
```

① 早退省掉了重复加载。但**它也是 bug 源**：小动作演完调 `play("idle")` 时，
如果当前已经是 idle，早退不会清掉 `loop_span`，结果那段小动作被无限循环
（用户报过“看向一边一直在循环”）。修复办法是 `resume_idle_at()`：

```rust
fn resume_idle_at(&mut self, frame: u32) -> Result<()> {
    self.loop_span = None;      // 先清区间
    self.play("idle")?;         // 再切
    self.frame = frame % count; // 落在段末
    self.draw_frame()
}
```

③ 先摆位置再画：`UpdateLayeredWindow` 带尺寸时保留左上角，新尺寸如果变大，
必须先落位，否则会从旧左上角长出去。

## 2.8 `after_switch`：进场收尾

```rust
fn after_switch(&mut self, id: &str) -> Result<()> {
    self.action_playing = false;
    let Some(spec) = clip(id) else { return Ok(()) };
    match spec.on_enter {
        OnEnter::Sleep => { self.sleep_cycle = Some(SleepCycle::new()); self.apply_sleep_span(); }
        OnEnter::Idle => { self.brain.set_sleep_after(self.next_sleep_delay()); self.schedule_next_action(); }
        OnEnter::Plain => {}
    }
    if let Some((lo, hi)) = spec.span { self.loop_span = Some((lo, hi)); }
    ...
}
```

所有具体 id 的判断都变成了数据（`ClipSpec`），这里不再写
`if id == "drag"`。加一段新动画不需要改这个函数。

## 2.9 `advance_frame`：整个播放器唯一容易写错的地方

```rust
fn advance_frame(&mut self, step: u32, count: u32) {
    self.tick_dissolve();                       // ① 淡入进度 +1（不拦帧号）
    let Some((lo, hi)) = self.loop_span else {  // ② 没有区间：整段循环
        let next = (self.frame + step) % count;
        if next < self.frame { self.start_seam_dissolve(self.frame, next); }
        self.frame = next;
        return;
    };
    if lo > hi || hi >= count {                 // ③ 区间坏了：退回整段
        self.loop_span = None;
        self.frame = (self.frame + step) % count;
        return;
    }
    let span = hi - lo + 1;
    if self.frame < lo || self.frame > hi {     // ④ 帧号在区间外：拉回段首
        self.frame = lo;
    }
    let next = self.frame.saturating_add(step);
    if next <= hi { self.frame = next; return; } // ⑤ 段内：正常前进
    if self.sleep_cycle.as_mut().is_some_and(|c| c.on_segment_end()) {
        // ⑥ 睡眠换段：把帧号夹进新区间，而不是拨回段首
        self.apply_sleep_span();
        if let Some((a, b)) = self.loop_span { self.frame = self.frame.clamp(a, b); }
        else { self.frame = lo; }
    } else {
        // ⑦ 普通回绕：段尾 → 段首，必要时淡入
        let next_frame = lo + (next - hi - 1) % span;
        self.start_seam_dissolve(self.frame, next_frame);
        self.frame = next_frame;
    }
}
```

逐行说明：

- ① 淡入只推进自己的进度，帧号照常走。曾经为了让混合两端是固定图而把帧停住 6 拍，
  用户看到的是“卡一下”；现在旧帧作为残影，在动的画面上逐拍淡掉。
- ② 没有区间（普通动画）就整段取模。`next < frame` 说明回绕了，触发淡入判断。
- ③ 区间坏了（换了素材、帧数变了）必须退回整段，不能卡在第 0 帧不动。
- ④ 换段后帧号可能落在新区间外（`play` 重置为 0，睡眠段从 104 起），拉回 `lo`。
- ⑤ 段内正常前进。
- ⑥ 睡眠换段：`on_segment_end` 返回 true 时（入睡→睡着），把帧号 **clamp**
  进新区间。入睡停在 156，156 本来就在打呼段 104~156 里，拨回 104 等于倒退 52 帧，
  Z 会缩一下。
- ⑦ 普通回绕：`next - hi - 1` 是超出段尾的帧数，`% span` 保证落回段内。
  回绕前调 `start_seam_dissolve`，如果接缝太硬就淡过去。

## 2.10 接缝淡入 `Dissolve`

```rust
struct Dissolve {
    from: Vec<u8>,   // 接缝前那一帧
    out: Vec<u8>,    // 混合结果（复用）
    done: u32,
    total: u32,
}
```

`SEAM_DISSOLVE_MAD = 3.0` 是阈值：两帧平均像素差超过 3 才淡。为什么是 3？
素材自身相邻帧的差在 1.5~4.1 之间，接缝只要比“正常一帧”大就会被看成跳。

淡入**不冻帧号**：`t = done/total`，旧帧占 `1-t`，新帧占 `t`。6 帧走完接缝的
1/6 每拍，落回素材自身的噪声里。

`blend_frame` 的混合公式：

```rust
((a * (den - num) + b * num) / den)
```

预乘 BGRA 下逐通道线性插值就是正确的 alpha 混合，不会出现半透明边缘发黑。

## 2.11 走路

```rust
struct Walk {
    left: f32,      // 还剩多少像素
    dir: f32,       // -1 左 / +1 右
    per_frame: f32, // 每帧走多少（素材自己定）
    faces_left: Option<bool>,
    x: f32,         // 浮点累加，只在摆窗口时取整
}
```

- **速度来自素材**（`px_per_frame`），不是“距离 ÷ 帧数”：侧身跳步和正面小碎步
  的步频不一样，统一速度会滑步；
- `x` 必须浮点：短距离时每帧 0.4 像素，逐帧 `round()` 会被舍成 0，变成原地踏步；
- 走到一半点新方向：`walk_to` 直接改 `dir/left`，不切动画（可打断）；
- 往素材反方向走时镜像这一帧（`mirror_bgra`）。

`MoveClip` 的 `span` 跳过前摇：`move_crab` 从第 40 帧起（前 8 帧几乎不动），
`move_float` 从第 18 帧起（前 17 帧站着）。不跳前摇，用户看到的是“先动、一秒后迈腿”。

## 2.12 菜单：数据驱动 + 序号对位

```rust
const CATEGORIES: &[Category] = &[
    Category { label: "转向", weight: 5, clips: &["turn"] },
    Category { label: "移动", weight: 5, clips: &["move_crab", "move_float", "move_run"] },
    ...
];
```

同一个表决定：

- 随机池：`category.weight × 1000 / clips.len()` 摊到每段；
- 菜单：`动作 → 分类 → 具体动画`；
- 命令 id：`CMD_CLIP_BASE + 序号`。

`menu_clips()` 与 `build_menu()` 分别过滤清单，必须顺序一致。只数总数发现不了
“点第 5 项播第 6 段”，所以自检用 `dispatch_menu_clip(0)` / `(len-1)`
真的点首末两项，验证播出来的就是对应素材。

`play_once` 处理三件事：睡着先醒、停掉正在走的路/惯性、移动类真的走一段。

## 2.13 打字联动

```rust
fn note_typing(&mut self, keys: u32, dt: f32, now: Instant) -> Result<bool> {
    self.typing_heat = (self.typing_heat - self.cfg.typing.decay_per_sec * dt).max(0.0);
    if keys == 0 { return Ok(false); }
    if self.forced.is_some() || self.hidden || self.waking
        || !self.typing_enabled || self.brain.mood() != Mood::Idle || self.action_playing {
        return Ok(false);
    }
    self.typing_heat = (self.typing_heat + keys as f32).min(self.cfg.typing.warmup_keys * 4.0);
    if self.typing_heat < self.cfg.typing.warmup_keys || now < self.typing_next {
        return Ok(false);
    }
    self.typing_heat = 0.0;
    self.typing_next = now + rng.range(self.cfg.typing.cooldown.0, self.cfg.typing.cooldown.1);
    if self.rng.unit() >= self.typing_chance { return Ok(false); }
    let id = self.pick_typing_work();
    self.play(id)?;
    self.action_playing = true;
    Ok(true)
}
```

- 热度每键 +1、每秒衰减 `cfg.typing.decay_per_sec`（默认 2）；阈值默认 12 热 ≈ 连敲 12 键；
- 到线后掷 `cfg.typing.chance`（默认 35%），中不中都重置热度并记冷却——不重置会每帧重掷，
  “有概率”变“必触发”；
- 睡着/演动作/藏着/关掉开关都不抢；
- 抽段权重仍在 `TYPING_WORK` 里（代码内的素材偏好），触发参数在 `config.json`。

## 2.14 动画缓存

```rust
const ANIM_CACHE: usize = 6;
if self.anims.len() >= ANIM_CACHE {
    let keep = [self.current.as_str(), id, "idle", "drag"];
    self.anims.retain(|k, _| keep.contains(&k.as_str()));
}
```

100 段素材每段有一个整帧缓冲（几十万字节），全留着是几十 MB。只留“当前 + 待切 +
idle/drag 预热”，其余丢掉，重播时重开一次十几毫秒。

## 2.15 自由函数

- `window_pos()`（`mod.rs`）：锚点 → 窗口左上角（唯一公式）；
- `mirror_bgra()`（`pixels.rs`）：逐行倒序，用于往反方向走；
- `seam_mad()`（`pixels.rs`）：每 16 字节采样算平均差，判断接缝硬不硬；
- `blend_frame()`（`pixels.rs`）：按 `num/den` 混合两帧；
- `ordered()`（`pixels.rs`）：区间反了取中点，绝不 panic。

## 2.16 配置与模块边界

**为什么参数放 `core/config.rs`**：入睡间隔、动作间隔、打字联动、走路距离、分类权重都是
“产品参数”，不是算法常量。它们在 `core` 里定义默认值、解析和范围收敛，所以不依赖
Windows、可以直接单测；`feiyu-app` 只负责找文件、读文件、把 `Config` 交给 `Pet`。

```rust
pub struct Config {
    pub action_gap: (f32, f32),
    pub sleep_after: (f32, f32),
    pub typing: Typing,
    pub walk: Walk,
    pub category_weights: HashMap<String, u32>,
}
```

三条规则：

1. **缺字段用默认值**：`#[serde(default)]` + 每个字段 `Option<T>`，用户只写想改的那几项；
2. **写错不拒绝整份配置**：数值不合法就夹回合理区间（概率 0~1、区间自动排序、负数夹到下限），
   只有 JSON 语法/类型错才回退整份默认值；
3. **单位写进字段名**：`_px`、`_sec`、`_keys`，不靠注释猜。

**模块拆分后的可见性**：Rust 的私有项对“定义模块及其后代”可见。`clips.rs` 和 `walk.rs`
是兄弟模块，所以 `clips` 的 `ClipSpec`/字段要 `pub(crate)` 才能被 `walk` 用；
`pixels.rs` 只给父模块用，所以是 `pub(super)`；`Walk` 只在 `pet` 内部出现，类型
`pub(super)`、字段保持私有。原则：**能不给就不给，先写私有，编译器报错再放一层**。

`impl Pet` 可以分散在多个文件里——Rust 不要求 `impl` 和结构体在同一个文件，
只要在同一个 crate。这让“按行为拆文件”不需要牺牲字段访问：子模块本来就能访问
父模块的私有字段。

---

# 三、`selftest.rs`：为什么需要“真机自检”

单元测试测不了：

- 窗口是否真的带 `WS_EX_LAYERED`；
- 透明像素是否真的让鼠标穿透；
- `UpdateLayeredWindow` 是否真的把像素画到屏幕上；
- 菜单在 `WS_EX_NOACTIVATE` 窗口上是否真的弹得出来；
- 换动画时脚底是否真的没跳。

这些只能开真窗口、真提交、真抓屏。`selftest` 用 `Report` 收集每一条：

```rust
struct Report { passed: u32, failed: u32, skipped: u32 }
impl Report {
    fn check(&mut self, name: &str, ok: bool) { ... println!("✓/✗ {name}") }
    fn skip(&mut self, name: &str) { ... }
    fn summary(&self) -> String { ... }
}
```

几条典型检查：

- **窗口层**：建一个 200×240 的窗口，画一个 100×100 方块，断言方块内 `hit_test` 为真、
  方块外为假、截图像素是蓝色。
- **动画切换**：`idle → sleep → stretch`，断言窗口尺寸变化、脚底线（窗口顶 + baseline）
  始终等于桌面地面线；再抽查 `hobby_horse` / `leaves` / `whale` / `rubik`
  （有地面道具的段）仍然对位。
- **菜单**：不真的弹（模态没法自动关），而是构造菜单后用 `GetMenuItemCount` /
  `GetSubMenu` 问 Win32：顶层 10 项、「动作」下 10 类 97 段。
- **睡眠**：睡着后推 7000 帧，断言帧号始终在 104~156、回绕只发生在 156→104、
  每次回绕都走了淡入、淡入期间帧号没有停住。
- **打字联动**：把概率钉成 1.0/0.0，验证“到线才触发 / 冷却期不重复 / 关掉不触发 /
  睡着不触发”。

自检的哲学：**能自动验证的绝不靠肉眼**；实在要人眼的（菜单长什么样、某段动画好不好看），
在 `_notes/PROGRESS.md` 的“已知未验证”里列出来。

> **两个 exe**：`feiyu.exe` 是 GUI 子系统（双击不弹终端），`feiyu-cli.exe` 是 console
> 子系统（`--selftest` / `--list` 用它，输出和退出码同步）。两者都调 `cli_main`，
> 行为一模一样；拆开是因为一个 PE 只能有一个子系统。

---

# 四、本章验收

- [ ] 能说出 `tick` 里十个步骤为什么是这个顺序；
- [ ] 能解释 `play` 早退为什么会造成“无限循环”，以及 `resume_idle_at` 怎么修；
- [ ] 能逐行讲 `advance_frame` 的七个分支；
- [ ] 能解释淡入为什么不冻帧号；
- [ ] 能说出 `Walk.x` 为什么必须是浮点；
- [ ] 能解释菜单序号为什么要“点首末两项”验证。
