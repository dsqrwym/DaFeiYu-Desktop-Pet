# 02 Rust 语法地图（用本项目当例子）

这一章不追求覆盖全部 Rust，只覆盖**本项目实际用到的**语法。每一条都配真实代码，
你可以边读边在编辑器里搜。

---

## 1. 工作区、crate、模块

- **crate**：一次编译的单位。本项目有 4 个：`feiyu-core`、`feiyu-platform`、
  `feiyu-app`、`pack-anim`。
- **workspace**：根目录 `Cargo.toml` 里的 `[workspace] members = [...]` 把它们绑在一起，
  `cargo test --workspace` 会一次跑完。
- **mod**：crate 内部的模块。`crates/feiyu-core/src/lib.rs` 里：

```rust
pub mod anim;
pub mod brain;
pub mod motion;
```

`pub mod` 表示“外部可以访问”。`feiyu-app` 里则用 `use feiyu_core::brain::{Brain, Mood};`
把它引进来。

**为什么在意模块**：`pub` 是 API 边界。`feiyu-core` 只把 `Brain`、`Motion` 这些
“纯逻辑”暴露出去，内部实现（比如 `switch_to`）保持私有，改它不会影响别的 crate。

---

## 2. 变量、可变性、常量

```rust
const MAX_STEP: f32 = 0.032;          // 编译期常量，不能改
let mut ticks = 0u32;                 // 可变变量
let (vx, vy) = fling_velocity(&samples); // 解构赋值
```

Rust 的变量**默认不可变**。要改必须写 `mut`。
这不是洁癖：`mut` 一出现，读代码的人就知道“这里的状态会变”，
而 `const` 保证参数不会被运行时偷偷改掉。

`0u32` 是“带类型后缀的字面量”，等价于 `let ticks: u32 = 0;`。
项目里常用 `1e3`、`1.0 / 60.0` 这种写法，类型由上下文推断。

---

## 3. 函数与“一切皆表达式”

```rust
fn clamp_pair(v: f32, range: (f32, f32)) -> f32 {
    let (lo, hi) = range;
    if lo > hi {
        return (lo + hi) / 2.0;
    }
    v.clamp(lo, hi)          // 最后一行没有分号 = 返回值
}
```

- `fn 名字(参数: 类型) -> 返回类型 { ... }`
- 块里最后一个**不带分号**的表达式就是返回值（`v.clamp(...)`）。
- 提前返回用 `return`。
- 没有返回值时返回 `()`，可以省略不写。

`if` 也是表达式：

```rust
let how = if weight == 0 {
    "不进随机池".to_owned()
} else {
    format!("随机权重 {weight}")
};
```

---

## 4. 结构体、枚举、match

**结构体**把相关字段放在一起：

```rust
pub struct Limits {
    pub x: (f32, f32),
    pub y: (f32, f32),
}
```

**枚举**在 Rust 里是“带数据的联合体”，比 C 的 enum 强得多：

```rust
pub enum Mood {
    Idle,
    Sleep,
    Happy,
    Pout,
    Dragged,
}
```

`match` 必须**穷尽所有分支**，编译器会检查：

```rust
impl Mood {
    pub fn anim_id(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Sleep => "sleep",
            Mood::Happy => "click_happy",
            Mood::Pout => "click_pout",
            Mood::Dragged => "drag",
        }
    }
}
```

为什么用枚举而不是字符串？因为**拼错一个字符串编译期不会报**，
而漏写一个 `Mood` 分支当场编译失败。

带数据的枚举，本项目也有：

```rust
pub enum Event {
    Tick,
    MouseDown { x: i32, y: i32 },
    RightClick { x: i32, y: i32 },
    TrayLeftClick,
    ...
}
```

用法：

```rust
match ev {
    Event::Tick => self.tick(),
    Event::MouseDown { .. } => { ... }
    Event::Moved { x, y } => self.on_moved(x, y),
    ...
}
```

`..` 表示“其余字段我不关心”。

---

## 5. `Option` 与 `Result`：Rust 没有 null 和异常

### Option：可能没有

```rust
pub fn get(&self, id: &str) -> Option<&AnimEntry> {
    self.animations.iter().find(|a| a.id == id)
}
```

调用方必须处理“没有”：

```rust
let Some(cycle) = self.sleep_cycle.as_ref() else {
    return;
};
```

`let ... else` 的意思是：匹配成功就继续，否则走 `else` 分支（必须 `return`/`break`/
`panic`）。这比 `if let Some(x) = ... { 一大堆缩进 }` 少一层嵌套。

### Result：可能失败

```rust
pub fn play(&mut self, id: &str) -> Result<()> {
    self.ensure_loaded(id)?;            // 失败就立刻返回错误
    let entry = self.manifest.require(id)?.clone();
    ...
    self.draw_frame()
}
```

`?` 是“失败就返回”的语法糖：

- 函数返回 `Result<T, E>`；
- 表达式是 `Result<U, E>`；
- `?` 成功取出 `U`，失败则把 `E` 原样返回给调用方。

**错误类型是怎么统一的**：`feiyu-app` 用 `anyhow::Result`（错误类型是
`anyhow::Error`，什么错都能装）；`feiyu-core` 用自己的 `AnimError`（枚举，
调用方可以 `match` 具体错误）。在最外层用 `{e:#}` 打印，`#` 会带上错误链。

---

## 6. 所有权、借用、生命周期（重点）

这是 Rust 最容易卡住的地方。用项目里的代码讲。

### 6.1 所有权：值只能有一个主人

```rust
let frames: Vec<Vec<u8>> = ...;
let file = encode(header, frames, 3)?;   // frames 被 move 进函数，之后不能再 used
```

如果还要用，就传引用 `&frames`，或者 `clone()`。
项目里 `Pet::play` 每次都要用 `entry`，所以写 `let entry = ...?.clone();`。

### 6.2 借用：同一时刻，要么多个只读，要么一个可写

```rust
fn draw_frame(&mut self) -> Result<()> {
    let entry = self.manifest.require(&self.current)?.clone();
    let Self { win, anims, current, frame, .. } = self;   // 一次性拆字段
    let anim = anims.get_mut(current).unwrap();
    let raw = anim.frame(*frame)?;
    win.draw(entry.width() as u32, entry.height() as u32, raw)?;
    Ok(())
}
```

这里有两个问题同时存在：

1. `win` 要 `&mut`（提交位图），`anims` 也要 `&mut`（解帧）；
2. 如果写成 `self.win.draw(..., self.anims.get_mut(...).frame(...)?)`，
   编译器会拒绝：不能在借用 `self.anims` 的同时可变借用 `self.win`。

解法是**解构字段**：`let Self { win, anims, .. } = self;` 把 `self` 拆成几个
互不相干的 `&mut`，借用检查器就满意了。

### 6.3 生命周期：引用的有效期

```rust
pub fn frame(&mut self, idx: u32) -> Result<&[u8], AnimError>
```

返回的 `&[u8]` 借自 `self.scratch`，所以只要你还拿着这个切片，
就不能再调 `reader.frame(...)`（第二次调用会要求 `&mut self`）。
项目里的写法是“取一帧、立刻提交/拷贝”，不跨调用保存切片。

`&'static str` 表示“指向程序里永生的字符串字面量”，比如：

```rust
struct ClipSpec {
    id: &'static str,
    ...
}
```

这样 `ClipSpec` 可以放在 `const` 表里，不依赖任何运行时分配。

### 6.4 一条实用规则

> 先问“这个函数需不需要拥有这个值？”
> 不需要 → 传 `&T`；需要改 → 传 `&mut T`；需要长期保存 → `clone()` 或 `to_owned()`。

项目里大量 `entry.clone()` 就是最后一种：窗口尺寸信息很小，克隆一下换来借用简单。

---

## 7. 字符串、集合

| 类型 | 用途 | 项目里的例子 |
|---|---|---|
| `&str` | 借来的字符串切片 | 函数参数 `id: &str` |
| `String` | 拥有的、可增长的字符串 | `self.current: String` |
| `&'static str` | 程序里永生的字面量 | `Mood::anim_id()` 的返回值 |
| `Vec<T>` | 动态数组 | `Vec<u8>` 像素、`Vec<FrameEntry>` |
| `HashMap<K, V>` | 哈希表 | `anims: HashMap<String, Anim>` |
| `VecDeque<T>` | 双端队列 | `trail: VecDeque<(Instant, i32, i32)>` |

`to_owned()` / `.to_string()` 把 `&str` 变成 `String`；`&s` 把 `String` 借成 `&str`。
项目里 `self.current.as_str()` 就是为了把 `String` 借给需要 `&str` 的函数。

---

## 8. 迭代器与闭包

```rust
let available: Vec<&str> = manifest.ids().collect();
let pool = Pool::new(pool_items()).retained(&available);
```

```rust
CATEGORIES
    .iter()
    .filter(|c| c.weight > 0)
    .map(|c| (c.id, c.weight))
    .collect()
```

- `iter()` 产生“引用迭代器”；
- `filter(|x| ...)`、`map(|x| ...)` 接收**闭包**（匿名函数）；
- `collect()` 把迭代器收成 `Vec`/`HashMap` 等；
- 闭包参数 `|c|` 里的 `c` 是 `&&Category`（因为 `iter()` 已经借了一层），
  所以字段访问会自动解引用。

`for` 循环本质也是迭代器：

```rust
for (label, weight, clips) in Pet::categories() {
    ...
}
```

---

## 9. trait、derive、泛型

**trait** 是“共同能力的接口”。本项目用得最多的是自动派生：

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits { ... }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood { ... }
```

- `Debug`：能用 `{:?}` 打印（日志与测试断言）；
- `Clone`：能 `.clone()`；
- `Copy`：赋值时按位复制（小结构体才用，`Mood` 这种枚举）；
- `PartialEq`：能用 `==` 比较。

**自定义 trait 实现**：错误类型用 `thiserror`：

```rust
#[derive(Debug, thiserror::Error)]
pub enum AnimError {
    #[error("读写出错: {0}")]
    Io(#[from] std::io::Error),
    #[error("文件损坏: {0}")]
    Corrupt(&'static str),
    ...
}
```

`#[from]` 自动生成 `From<std::io::Error>`，于是 `?` 能把 IO 错误自动转成
`AnimError::Io`。这就是为什么项目里到处是 `?` 却看不到手写转换。

**泛型**：

```rust
pub struct AnimReader<R: Read + Seek> { r: R, ... }
```

`R: Read + Seek` 读作“R 必须实现 Read 和 Seek 两个 trait”。
`AnimReader<BufReader<File>>` 是真实文件，测试里用 `AnimReader<Cursor<Vec<u8>>>`
（内存里的假文件）——同一套代码，两种数据源。

---

## 10. 测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_to_the_ground_and_stops() {
        let m = Motion::fling(0.0, 0.0);
        let ((x, y), rest, frames) = settle(m, (500.0, 400.0), 600);
        assert!(rest, "600 帧内必须停稳");
        assert_eq!(y, GROUND, "必须精确贴地");
    }
}
```

- `#[cfg(test)]`：只在 `cargo test` 时编译；
- `use super::*`：把父模块的东西引进来；
- `assert!` / `assert_eq!` / `assert_ne!`：断言，第二个参数是失败时的说明；
- 测试和被测代码放在**同一个文件**里（项目约定），改实现时测试就在眼前。

本项目 113 条单测里，很多是“回归测试”：注释里写着“早期版本这里错过一次”。
例如 `sequential_playback_decodes_one_per_frame` 钉住“顺序播放每帧只解压一次”，
因为曾经有一次把链号算错，导致每帧重解整条差分链、慢 8 倍。

---

## 11. `unsafe` 与 SAFETY 注释

Rust 保证内存安全，但调 Win32 必须 `unsafe`。项目约定：**每一处 `unsafe` 前面
都要有一行 `// SAFETY:` 说明“为什么这里成立”**。

```rust
let mut info = LASTINPUTINFO { ... };
// SAFETY: info 是栈上有效结构，cbSize 已按 API 要求填好。
if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
    return None;
}
```

读 `unsafe` 代码时，先读 `SAFETY`，再看调用。如果一个 `unsafe` 块没有 SAFETY 注释，
在本项目里算 bug。

---

## 12. 原子与时间

打字计数跨线程（wndproc 写、tick 读），所以用原子量：

```rust
static KEYS: AtomicU32 = AtomicU32::new(0);

pub fn typing_keys() -> u32 {
    KEYS.swap(0, Ordering::Relaxed)
}

pub(crate) fn note_key_down() {
    KEYS.fetch_add(1, Ordering::Relaxed);
}
```

`Ordering::Relaxed` 的意思是“只保证这一个变量自身的原子性，不提供跨变量顺序”。
计数器只需要这个强度。

时间用 `std::time::Instant`（单调时钟，不受系统时间修改影响）：

```rust
self.landed_at = Some(now);
if now.duration_since(t).as_secs_f32() >= LANDING_HOLD { ... }
```

**不要用 `SystemTime` 做间隔**：用户改一下系统时间，间隔就乱了。

---

## 13. 可见性（pub 的层级）

| 写法 | 谁能用 |
|---|---|
| `pub` | 任何 crate |
| `pub(crate)` | 同一个 crate 内（本项目自检用） |
| 不写 | 当前模块及其子模块 |
| `pub(super)` | 父模块 |

项目里 `pub(crate) fn feed_typing`、`pub(crate) fn advance_frames` 都是
“只给自检用，不对外暴露”的接口。

---

## 14. 一张语法对照表

| 你看到的 | 意思 |
|---|---|
| `let x = ...;` | 不可变绑定 |
| `let mut x = ...;` | 可变绑定 |
| `&T` / `&mut T` | 只读借用 / 可变借用 |
| `T::new()` | 关联函数（类似静态方法） |
| `self.xxx()` | 方法调用（第一个参数是 self） |
| `Option<T>` | 可能没有值 |
| `Result<T, E>` | 可能失败 |
| `?` | 失败就返回 |
| `match` | 穷尽匹配 |
| `if let Some(x) = ...` | 只关心一种情况 |
| `let Some(x) = ... else { ... }` | 同上，但要求提前返回 |
| `#[derive(...)]` | 自动生成实现 |
| `#[cfg(test)]` | 条件编译 |
| `unsafe { ... }` | 调用不安全的底层 API |
| `impl Trait for Type` | 为类型实现 trait |

看完这一章，再回去看 `motion.rs` 或 `brain.rs`，应该能读懂 80%。读不懂的，
在 04 里按文件查。
