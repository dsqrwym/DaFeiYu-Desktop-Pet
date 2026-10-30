# 04 逐文件讲解：feiyu-core（纯逻辑层）

> [English](en/04-core-files.md) | 简体中文

`feiyu-core` 没有任何 `windows::` 依赖，所以它的每一条逻辑都能在 `cargo test` 里钉死。
这一章按文件讲，重点在“为什么这么写”，而不是逐字翻译语法。

```
crates/feiyu-core/src/
├── anim/
│   ├── format.rs     .fishanim 容器：帧头 + 帧表 + 压缩帧
│   ├── reader.rs     运行时随机取帧
│   └── manifest.rs   manifest.json：id → 文件 + 几何
├── brain.rs          心情状态机
├── config.rs         用户参数（默认值 / JSON / 范围收敛）
├── mask.rs           `.fishmask` 1-bit alpha 掩码
├── motion.rs         甩出后的滑行/下落/反弹
├── pool.rs           带权重的随机池
├── rng.rs            xorshift64 随机数（不引第三方）
├── sleep.rs          睡觉的三段编排
└── lib.rs            模块导出
```

---

## 1. `anim/format.rs`：自定义动画容器

### 1.1 为什么不用 gif/webm

- gif：只有 256 色 + 1 位透明，边缘会锯齿；
- webm：VP9 解码要带 `ffmpeg`/`libvpx` 动态库，启动慢、发版大；
- `.fishanim`：**预乘 BGRA 原始帧 + zstd**，解出来就能直接交给
  `UpdateLayeredWindow`，零转换。

代价是体积大（100 段约 582MB），换来的是“打开就能画”。这是有意识的取舍。

### 1.2 文件布局

```
0   8  magic        "FISHANM1"
8   2  version
10  2  flags        bit0 = 已预乘 alpha；bit1 = 帧间差分
12  2  width
14  2  height
16  4  frame_count
20  4  fps          f32 的位模式
24  2  baseline     脚底线（帧内）
26  2  key_every    差分的 key 间隔
28  4  table_off    帧表偏移（恒为 48）
32  4  data_off     负载起始
36  4  payload_len
40  2  origin_x
42  2  origin_y
44  4  reserved
48  ×N 帧表项：offset u32 | len u32 | codec u8 | pad[3]
```

帧表项的 `offset` 是**文件内绝对偏移**，所以取任意一帧只要一次 `seek`。

### 1.3 编码：`encode_with` 的核心循环

```rust
for (idx, frame) in frames.into_iter().enumerate() {
    let idx = idx as u32;
    if frame.len() != expected {
        return Err(AnimError::FrameSize { expected, got: frame.len(), idx });
    }
    // 链首（idx % key == 0）存原图，其余存差值。
    let source: Vec<u8> = if key > 1 && idx % key != 0 {
        let base = prev.as_ref().expect("差分帧必然有前一帧");
        frame.iter().zip(base.iter())
             .map(|(a, b)| a.wrapping_sub(*b))
             .collect()
    } else {
        frame.clone()
    };
    prev = Some(frame);

    let (bytes, codec) = if level > 0 {
        match zstd::bulk::compress(&source, level) {
            Ok(packed) if packed.len() < source.len() => (packed, Codec::Zstd),
            _ => (source, Codec::Raw),
        }
    } else {
        (source, Codec::Raw)
    };
    ...
}
```

逐行看：

- `frames.into_iter()`：拿到每一帧的所有权；
- `frame.len() != expected`：尺寸不对立刻报错，绝不“凑合解”；
- 差分：不是 key 帧时，逐字节 `a - b`（`wrapping_sub` 允许下溢，解码时 `wrapping_add` 还原）；
- `prev = Some(frame)`：保存当前帧给下一帧做差；
- 压缩：**压不小就不压**（`packed.len() < source.len()`），否则解压反而更慢；
- 最后回填 `offset`：负载紧跟在“头 + 帧表”后面，所以 `cursor` 从
  `HEADER_SIZE + count * ENTRY_SIZE` 开始累加。

`wrapping_sub` / `wrapping_add` 是差分的关键：普通减法在 `u8` 下溢会 panic
（debug）或回绕（release），行为不一致；`wrapping_*` 明确表示“按 256 取模”。

### 1.4 为什么差分能把压缩比从 7× 提到 20×

动画相邻帧的透明区、静止背景占很大比例，差分后大量字节是 0，zstd 能连续匹配。
代价是随机取帧要先从链首累加，最多 `key_every` 步；顺序播放只累加一步。

### 1.5 测试

- `roundtrip_header_and_table`：写进去再读出来，逐字节一致；
- `level_zero_stores_raw`：`level=0` 时必须走 `Raw`；
- `rejects_bad_magic` / `rejects_zero_frames` / `rejects_bad_fps`：坏文件必须报错，不能“带着坏数据继续跑”。

---

## 2. `anim/reader.rs`：运行时取帧

### 2.1 目标

- 不把整段动画读进内存；
- 顺序播放时每帧只解压一次；
- 随机跳帧不重解整条差分链。

### 2.2 `frame(idx)` 的三条路径

```rust
pub fn frame(&mut self, idx: u32) -> Result<&[u8], AnimError> {
    if self.pos == Some(idx) {
        return Ok(&self.scratch);          // ① 缓存命中
    }
    let key = if self.header.uses_delta() { self.header.key_every as u32 } else { 0 };
    if key > 1 {
        let resume = matches!(self.pos, Some(p)
            if p.checked_add(1) == Some(idx) && p / key == idx / key);
        if resume {
            self.decode_one(idx, false)?;  // ② 同一链的下一帧：只累加一步
        } else {
            let chain = idx / key * key;   // ③ 跳链：从链首重解
            self.decode_one(chain, true)?;
            for step in (chain + 1)..=idx {
                self.decode_one(step, false)?;
            }
        }
    } else {
        self.decode_one(idx, true)?;
    }
    self.pos = Some(idx);
    Ok(&self.scratch)
}
```

关键在 `resume` 的条件：

```rust
p + 1 == idx && p / key == idx / key
```

- 必须是“上一帧的下一帧”（顺序播放）；
- 必须还在同一条差分链里。

早期版本把“链号”和“链首帧号”直接比，只有第 0 条链碰巧相等，其余每帧都退化成
“从链首重解整条链”——顺序播放慢 8 倍。现在这个条件有专门的回归测试
（`sequential_playback_decodes_one_per_frame` 断言 20 帧只解压 20 次）。

### 2.3 `decode_one`

```rust
self.staged.resize(entry.len as usize, 0);
self.r.seek(SeekFrom::Start(entry.offset as u64))?;
self.r.read_exact(&mut self.staged)?;
```

- `staged` / `delta` / `scratch` 都是复用缓冲：一帧 200KB，每帧新申请会让 GC 压力很大；
- `dec` 是复用的 zstd 解压上下文：每帧新建解压表会慢 6 倍；
- 解出差值后，`scratch[i] = scratch[i].wrapping_add(delta[i])` 还原。

---

## 3. `anim/manifest.rs`：清单与几何

### 3.1 一条动画记录

```json
{
  "id": "sleep",
  "name": "原地小憩沉眠",
  "file": "sleep.fishanim",
  "size": [384, 266],
  "origin": [84, 24],
  "baseline": 259,
  "frames": 241,
  "bytes": 5501487
}
```

- `size`：裁切后的帧尺寸；
- `origin`：裁切帧左上角在 544×306 画布中的位置；
- `baseline`：**帧内**脚底线；
- `feet_row()` = `origin_y + baseline`：换算到画布坐标的脚底线。

H.264 条目另外有 `codec: "h264"`、`file: "*.fishvid"`、`mask: "*.fishmask"`；
旧 `.fishanim` 条目没有 `codec` 字段，反序列化时默认 `FishAnim`，所以旧清单不用改。
`Manifest::validate` 会检查 `h264` 必须有 mask、文件名不能带 `../` 或绝对路径。

### 3.2 读入即校验

`validate()` 检查：版本、非空、id 唯一、尺寸非 0、帧数非 0、几何不越界、
`baseline <= height`。为什么在读入时查？因为启动时报“清单坏了”比播到第 7 秒
才发现某个 id 不存在好得多。

### 3.3 测试

- `parses_and_looks_up`：正常清单能查；
- `rejects_duplicate_id`：id 重复必须拒绝（菜单/池子都靠 id 定位）；
- `rejects_bad_version_and_geometry`：越界必须拒绝；
- `requires_known_id`：`require("fly")` 报错信息里要有 `fly`。

---

## 4. `brain.rs`：心情状态机

### 4.1 五个心情

```rust
pub enum Mood { Idle, Sleep, Happy, Pout, Dragged }
```

每个心情对应一个素材 id（`anim_id`）。为什么不在 `Pet` 里直接存字符串？
因为枚举能保证“所有心情都有素材”，漏一个编译不过。

### 4.2 三个时间量

- `elapsed`：当前心情持续了多久（也是“多久没被搭理”）。**一数两用**：
  - 待机够久 → 睡觉；
  - 够无聊 → 随机动作。
  播随机动作**不重置**它，所以自娱自乐几次之后照样会困，不会“永远不睡”。
- `poke_window` / `pokes`：2 秒内点 3 次 → 生气。
- `sleep_after`：入睡阈值，每次回待机重新抽 18~38 秒。

### 4.3 刺激与转移

| 刺激 | 效果 |
|---|---|
| `Click` | 连点计数 +1；≥3 → `Pout`，否则 `Happy` |
| `DragStart` | `Dragged`，清空连点 |
| `DragEnd` | `Idle` |
| `Attention`（鼠标划过） | **只清 elapsed，不改心情**；睡着也不叫醒 |
| `Awaken` | 回 `Idle`，不经过 `Happy` |

`Attention` 的设计很关键：鼠标在鱼身上划来划去，只推迟入睡，不把它弄醒；
真想叫醒得点击或拖拽。

### 4.4 测试

- `falls_asleep_after_quiet_time`：24.9 秒不睡，25.1 秒睡；
- `click_wakes_up_and_resets_timer`；
- `rapid_pokes_annoy` / `slow_pokes_do_not_annoy`；
- `attention_postpones_sleep` / `attention_never_wakes_a_sleeper`；
- `long_gap_puts_it_to_sleep`：休眠两小时后唤醒，空档必须算数；
- `ignores_bogus_dt`：NaN/负数/无穷大不能把状态机搞坏；
- `sleep_after_can_be_retuned`：负数夹到 0，非法值忽略。

这些测试的意义：状态机的所有转移都能在没有窗口的情况下验证，不用“等 25 秒看它睡不睡”。

---

## 5. `motion.rs`：松手后的物理

### 5.1 参数

| 常量 | 值 | 含义 |
|---|---|---|
| `FRICTION` | 3.2 | 速度按 `e^(-3.2·dt)` 衰减，1 秒后剩 4% |
| `GRAVITY` | 2400 | 像素/秒²，约 0.5 秒从 300px 落下 |
| `BOUNCE` | 0.35 | 撞墙/落地保留的速度比例 |
| `BOUNCE_MIN` | 240 | 低于它直接停，免得边缘抖动 |
| `MAX_SPEED` | 2600 | 防止一次甩动飞出屏幕 |
| `REST_SPEED` | 12 | 判定停稳 |
| `MIN_FLING` | 130 | 低于它忽略，轻轻一放不滑 |
| `MAX_STEP` | 0.032 | 单步积分上限，卡顿不瞬移 |

### 5.2 估速度：为什么用首尾两点

```rust
let vx = (last.1 - first.1) as f32 / span;
```

鼠标最后几毫秒的抖动，乘上很短的时间差会被放大成假速度。项目只保留最近 120ms 的
采样，并且用首尾两点，而不是最后两点。测试 `jitter_in_last_sample_does_not_blow_up_speed`
专门钉住这一点。

### 5.3 单步积分

```rust
let damp = (-FRICTION * dt).exp();
let mut vx = self.vx * damp;
let mut vy = if self.grounded {
    self.vy * damp
} else {
    (self.vy + GRAVITY * dt) * damp
};
let mut x = anchor.0 + vx * dt;
let mut y = anchor.1 + vy * dt;
```

- 先算阻尼，再积分位置；
- `grounded` 时不再加重力，否则贴地滑行会被反复“按下 → 弹回”，看起来像抽搐；
- 撞左右墙反弹；撞天花板贴住即停（弹个没完很怪）；
- 落地时，速度大于 `BOUNCE_MIN` 才反弹，否则停住并 `grounded = true`；
- `at_rest()` 要求“已贴地且水平/垂直余速都小”。

### 5.4 测试

`falls_to_the_ground_and_stops`、`thrown_upwards_comes_back_down`、
`bounce_never_gains_energy`（反弹不能越弹越高）、`horizontal_fling_stops_on_the_ground`、
`wall_bounce_reverses_direction`、`anchor_never_leaves_limits`、
`bad_dt_is_ignored`、`degenerate_limits_do_not_panic`。

“反弹不会越来越猛”这种结论靠肉眼看不出，只能靠测试。

---

## 6. `pool.rs`：带权重的随机池

```rust
pub fn pick(&self, rng: &mut Rng) -> Option<&'static str> {
    if self.total == 0 { return None; }
    let mut r = rng.below(self.total);
    for (id, weight) in &self.items {
        if r < *weight { return Some(id); }
        r -= *weight;
    }
    self.items.last().map(|(id, _)| *id)
}
```

- 权重为 0 的项直接丢掉；
- 重复 id 也丢掉（否则那段的实际概率悄悄翻倍）；
- `retained(&available)` 按素材清单瘦身，缺段不会抽到一个打不开的文件。

测试 `weights_decide_the_ratio` 用 1:3 的权重抽 10000 次，断言比例在 23%~27%。

---

## 7. `rng.rs`：为什么不引 rand

只需要“同一种子必然复现同一序列”，自己写 xorshift64 就够，还能在测试里固定种子。

```rust
pub fn next_u64(&mut self) -> u64 {
    let mut x = self.state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    self.state = x;
    x
}
```

`unit()` 取**高 24 位**而不是低位：xorshift 的低位周期性更明显，而 24 位正好是
`f32` 的尾数宽度。`below(n)` 用 `% n`，`n == 0` 返回 0（调用方不必先判空）。

测试：同种子同序列、不同种子发散、0 种子不能卡死、`below` 均匀。

---

## 8. `sleep.rs`：一整段素材拆成三段

### 8.1 问题

`sleep.fishanim` 是 10 秒闭环：睁眼 → 闭眼 → 冒 Z → Z 飘散 → 睁眼起身。
当普通循环播，鱼睡十秒就自己醒；不循环又只能睡两秒。

### 8.2 实测出来的帧段

| 帧段 | 内容 | 气泡个数 |
|---|---|---|
| 0~77 | 睁眼 → 闭眼 → 蜷成一团 | 0 |
| 78~104 | 气泡从 1 个长到 2~3 个 | 1 → 3 |
| **104~156** | **2~3 个挂着继续飘** | 2~3 |
| 157~173 | 一个个脱离、变淡、散尽 | 2 → 0 |
| 174~240 | 一个都没有；身体 176 起开始起身 | 0 |

### 8.3 三段编排

```rust
pub enum SleepStage { Falling, Asleep, Waking }
```

- `Falling`：0~156 播一遍；
- `Asleep`：104~156 **正着循环**；
- `Waking`：从**当前帧**演到 240。

### 8.4 为什么醒来要从当前帧开始

旧实现固定跳到第 170 帧（Z 已消失），画面差实测 11.7，用户看到的是“闪一下”。
从当前帧往后演，Z 在 149~173 自己飘散，一帧都不跳。

```rust
pub fn start_waking(&mut self, from: f32) {
    self.stage = SleepStage::Waking;
    if from.is_finite() {
        self.wake_from = from.clamp(0.0, 1.0);
    }
}
```

`from` 是**比例**而不是帧号：换一套素材帧数变了，比例还能用。

### 8.5 为什么循环点是 104~156 而不是“Z 散尽的 174”

这是三轮返工的结论：

1. 先用“右上角 Z 像素数量相等”选 116~148 → 整帧差 9.8，用户说“一抽一抽”；
2. 改成 76~174（两端 Z 都为 0）→ 腿部差 12.87，用户说“起身一下又回去”；
3. 最后逐帧量“身体相对 76 帧的位移”，发现身体从 76 起就在慢慢起身，
   只有 126 附近回到原点；再穷举 2544 对端点，选 **104↔156**（两端都有气泡，
   且身体差 4.10，是“身体静止”那一族里最小的）。

段尾接段首的整帧差是 5.11，仍然比自然一帧（~2.1）大，所以播放器用 6 帧淡过去
（见 06 的 `Dissolve`）。

### 8.6 测试

- `spans_land_on_the_measured_frames`：比例换算必须落在 0/133/104/156/170/240；
- `waking_starts_where_the_caller_asks`：从任意帧醒，换算后还是那一帧；
- `asleep_span_keeps_bubbles_at_both_ends`：两端都要有气泡；
- `wake_span_reaches_the_last_frame`：必须演到 240，否则鱼永远睁不开眼。

---

## 9. `config.rs`：用户参数

产品参数（入睡间隔、动作间隔、打字联动、走路距离、分类权重）不写在 `Pet` 里，
而是先在这里定义默认值和校验规则：

```rust
pub struct Config {
    pub action_gap: (f32, f32),
    pub sleep_after: (f32, f32),
    pub typing: Typing,
    pub walk: Walk,
    pub category_weights: HashMap<String, u32>,
}
```

### 9.1 为什么放在 core

- 它是纯数据 + 纯校验，不碰窗口，能在无桌面环境单测；
- app 只负责“找文件、读文件”，不把路径/IO 混进状态机；
- 配置的默认值就是代码里的出厂行为，测试可以同时钉住“默认值”和“越界收敛”。

### 9.2 解析：缺字段用默认，坏值不拒绝整份

`RawConfig` 的字段全是 `Option`，`#[serde(default)]` 让用户只写想改的那几项。
`from_json` 里做三件事：

1. 成对区间（`action_gap` / `sleep_after` / `cooldown`）自动排序并夹到下限；
2. 单值（概率、阈值、走路距离）夹到合理区间；`walk.min_px > max_px` 时交换；
3. `category_weights` 只做上限裁剪，未知分类留着（不会影响现有分类）。

只有 JSON 语法/类型错才返回 `Err`；数值不合理只收敛，不把整份配置丢掉。

### 9.3 加载优先级

`feiyu-app` 的 `load_config`：`--config` > exe 旁边 `config.json` > 当前目录
`config.json` > 默认值。显式路径读不了就报错；自动找到但写坏了只告警并回退。

### 9.4 测试

- 缺字段保持默认；
- 倒着写的区间自动排序、负数夹到下限、概率夹到 0~1；
- 反向的 `walk` 距离交换；
- 未知字段不导致失败；
- 语法错必须报错。

---

## 10. `mask.rs`：1-bit alpha 掩码

H.264 颜色不携带 alpha；透明信息单独存成 `.fishmask`：

```text
0   8  "FISHMSK1"
8   2  width   u16
10  2  height  u16
12  4  frames  u32
16  ... zstd(逐帧 1-bit 掩码，frame-major，LSB-first)
```

- 每帧 `ceil(width×height/8)` 字节，alpha ≥ 128 记为 1；
- 整份解压，`MaskSet::frame_bits(idx)` 随机读任意帧；
- 241 帧的掩码压缩后约 85~110KB，解压后约 1.5MB；
- `MaskSet::matches(w,h,frames)` 在打开视频时和清单对账。

为什么不把 alpha 编进 H.264：H.264 的 alpha 也是有损的，硬透明边缘会出现光晕；
1-bit 掩码是精确的。代价是内存里多一份位图，但只保留当前/预加载的 clip。

---

## 11. 本章验收

- [ ] 能解释 `.fishanim` 为什么用“帧表 + 绝对偏移”；
- [ ] 能说出差分的 `resume` 条件为什么必须比较“链号”；
- [ ] 能解释 `origin` / `baseline` / `feet_row` 三者关系；
- [ ] 能说出 `Attention` 与 `Click` 的区别；
- [ ] 能解释 `grounded` 为什么能避免贴地抽搐；
- [ ] 能复述睡眠循环点从 116~148 → 76~174 → 104~156 的三次修正原因。
