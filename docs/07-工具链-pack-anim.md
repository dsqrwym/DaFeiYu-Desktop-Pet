# 07 工具链：`pack-anim`（webm → .fishanim / .fishvid）

> [English](en/07-pack-anim.md) | 简体中文

`tools/pack-anim` 是一个独立的小程序：把上游 640×360 的透明 webm 变成运行时
零转换成本的 `.fishanim`，或 H.264 颜色 + 1-bit 掩码。它只依赖 `feiyu-core` 的格式定义，解码交给外部 ffmpeg。

```
crates/feiyu-core/src/anim/format.rs   ← 格式定义
tools/pack-anim/src/
├── main.rs    参数、批量打包、写清单、自检
├── decode.rs  ffmpeg/ffprobe 管道
└── mask.rs    黑底抠像、量化、脚底线
```

---

## 1. 为什么要有这一步

上游素材是 **VP9 + Alpha** 的 webm。运行时直接播它需要 `libvpx`/ffmpeg 动态库，
启动慢、发版大、还要处理解码线程与 YUV→RGBA 转换。

`.fishanim` 直接存**预乘 BGRA**：

- 解压出来就是 `UpdateLayeredWindow` 要的格式，零转换；
- 随机访问单帧，只解目标帧；
- 差分 + zstd 把体积压到原始帧数据的 10~20 倍。

代价是磁盘大（100 段约 625MB）和打包耗时（约 3 分钟）。这是一次性成本。

---

## 2. 整条链路

```
webm
 │  ffprobe 读宽高
 ▼
ffmpeg -vf scale=544:306 -pix_fmt rgba  （管道输出裸 RGBA）
 │
 ▼
mask::build_alpha     黑底泛洪抠像 → alpha
 │
 ▼
mask::quantize        砍掉颜色低 3 位（降熵）
 │
 ▼
统计包围盒 → crop    裁到不透明像素并集
 │
 ▼
RGBA → BGRA           与 Windows DIB 同序
 │
 ▼
format::encode_with   zstd + 帧间差分
 │
 ▼
写 .fishanim + manifest.json
```

### 2.1 为什么是 544×306

源视频 640×360，16:9。按高度 306 等比缩放：`640 × 306 / 360 = 544.0`，正好整数。
306 是桌宠在 1080p 下合适的高度（约屏幕的 28%）。

### 2.2 解码交给 ffmpeg

`decode.rs` 起一个 `ffmpeg` 进程，参数：

```rust
.args(["-hide_banner", "-loglevel", "error", "-nostdin"])
.arg("-i").arg(input)
.args(["-vf", &format!("scale={width}:{height}:flags=lanczos")])
.args(["-pix_fmt", "rgba", "-f", "rawvideo", "-"])
```

为什么不用 Rust 视频库：透明/抠像只需要“解一帧、拿像素”，ffmpeg 的 `scale`
已经把缩放和色彩转换做到位；引 libav 绑定只会让构建复杂化。

---

## 3. 抠像：`mask::extract`

上游 webm 是**黑底合成**（没有 alpha 通道）：`rgb = fg × a + 0`，画面本身已经是
预乘形式。所以不需要反预乘，只需要算出 alpha 与“直通色”（H.264 编码用）。

### 3.1 为什么不能全局阈值

角色内部有大量深色像素（深蓝头发、黑鞋、裙摆阴影）。全局“亮度 ≤ 12 就算背景”
会在角色身上打洞。

### 3.2 正确做法：从边框泛洪

```rust
let is_dark = |idx| lum(rgba[o], rgba[o+1], rgba[o+2]) <= thresh;
// 从四条边播种：只有连通到边框的暗区才是背景
for x in 0..width { for y in [0, height-1] { if is_dark(idx) { bg[idx] = true; stack.push(idx) } } }
...
// 四邻域泛洪
while let Some(idx) = stack.pop() {
    push(x-1, y); push(x+1, y); push(x, y-1); push(x, y+1);
}
```

被角色围住的暗像素不会被标记成背景，因此保留在体内。

`lum` 取三通道最大值而不是加权亮度：更保守，不会把饱和色误判成暗。

### 3.3 只有 0/255 会有什么问题

泛洪只回答“是不是背景”，不回答“有多不透明”。边缘和半透明特效的像素都是
`C = F × a` 且 `a < 1`，直接给 255 就会：角色轮廓一圈**黑边**；气泡中心、烟雾、
火星整块发黑。两者在两种素材包里都看得到，跟编码无关。

### 3.4 现在的做法：分三关

1. **打底**：背景 0、其余 255；面积在 `effect_min..=主块×effect_rel` 之间的小连通块
   （气泡、火星）按自身 90 分位亮度归一化，半透明而不是黑圆盘；
2. **过渡带**：从背景往内推 `--rim` 圈（默认 2），每个边界像素用**里侧最亮的邻居**
   当 `F`，反推 `a = lum(C) / lum(F)`。拿里侧邻居而不是“整个连通块的核心色”：
   辫子、草叶、深色描边这些一两像素宽的结构整体都在过渡带里，拿远处的亮色反推
   会把它们淡成半透明（实测圣诞树的草地会整块消失）；
3. **背景补色**：向外扩 1 像素，颜色取临近角色像素的直通色。H.264 的色度是 2×2
   采样，背景留纯黑就会往角色边上渗出一圈黑晕。

同时 `extract` 会返回直通色（`F = C × 255 / a`）：`.fishanim` 仍存预乘色，
H.264 的颜色通道必须存直通色——1-bit 掩码配上预乘色就是黑边。

已知局限：贴在角色身上、面积很大的半透明烟雾（例如烟花）仍会和“本来画得就深
的衣服”混在一起，在鱼身上表现为一块暗色区域。黑底合成区分不了这两者，
没有自动修法。

---

## 4. 量化：`mask::quantize`

```rust
let keep = 0xFFu8 << bits.min(7);
for px in rgba.chunks_exact_mut(4) {
    px[0] &= keep; px[1] &= keep; px[2] &= keep;
}
```

上游 webm 是损失压缩产物，帧间充满低位抖动；这些位对肉眼几乎无影响，却把熵拉满。
砍掉低 3 位（默认）后 zstd 的匹配长度大幅变长。alpha 不动。

---

## 5. 裁切与脚底线

### 5.1 裁切

统计全片不透明像素的**并集包围盒**，只保留这块。角色只占 16:9 画面的三成，
空白纯属浪费。裁切信息写进帧头的 `origin_x/origin_y`，渲染端据此还原画布位置。

### 5.2 脚底线：逐帧最底行的**众数**

```rust
let mut bottoms = vec![0u32; h + 1];
if let Some(bottom) = mask::last_opaque_row(&alpha, w, h) {
    let bottom = bottom.min(h);
    baseline = baseline.max(bottom as u16);
    bottoms[bottom] += 1;
}
let ground = bottoms.iter().enumerate()
    .filter(|(_, n)| **n > 0)
    .max_by_key(|(row, n)| (**n, *row))
    .map(|(row, _)| row as u16)
    .unwrap_or(baseline);
```

为什么不是最大值：烟花（火星落到底）、气球、水枪（水花）、落叶（叶堆）只有几十帧
里有东西沉到画面最底，取最大值会把**整段素材连同角色**抬起 20 像素。
众数 = 角色大多数时候站的那条线；票数相同取更靠下的一行。

实测效果：`idle` / `sleep` / `drag` 从 287/290/284 统一到 283，
换姿态不再上下跳 6 像素。

---

## 6. 编码与清单

`format::encode_with(header, frames, level, delta)` 负责差分 + zstd，
细节在 04 的 `format.rs`。默认参数：`level=3`、`delta=16`、`quant=3`、`crop=true`。

`write_manifest` 写出：

```json
{
  "format": 1,
  "canvas": { "width": 544, "height": 306 },
  "native_height": 306,
  "fps": 24,
  "animations": [
    { "id": "idle", "name": "待机呼吸休闲", "file": "idle.fishanim",
      "size": [211, 241], "origin": [163, 46], "baseline": 237,
      "frames": 241, "bytes": 5531091 }
  ]
}
```

`id` 是稳定契约：状态机、随机池、菜单都用它，文件名可以随打包策略变化。

### 6.1 H.264 模式：`.fishvid + .fishmask`

```powershell
cargo run -p pack-anim --release -- `
  --src _upstream/dsh-pet/dsh-pet/assets/webm `
  --out assets/h264 --codec h264 --crf 20
```

对每段写出：

- `<slug>.fishvid`：颜色 H.264（MP4 内容，`.fishvid` 扩展名）；
- `<slug>.fishmask`：1-bit alpha + zstd（见 `core/anim/mask.rs`）；
- manifest 条目增加 `"codec": "h264"`、`"mask"`，`bytes` 是视频 + 掩码。

编码命令显式写 `-framerate <fps> -r <fps>`。不写的话 ffmpeg 对 rawvideo 默认 25fps，
而 manifest 是 24fps，seek 第 240 帧会提前 EOS——原型踩过一次。

颜色尺寸补到偶数宽高；解码端裁回清单里的 `size`。CRF20 全量 100 段实测约 79.2MB
素材（直通色 + 边缘补色之前是 69.5MB），打包后约 81MB。

---

## 7. 中文名 → 文件名

`NAME_MAP` 把中文素材名映射成 ASCII slug：

```rust
("待机呼吸休闲", "idle"),
("点击回应-挠痒咯咯笑", "click_tickle"),
("工作状态-垂头叹气冒汗", "work_sigh"),
...
```

- 中文只留在 `manifest.json` 的 `name` 里做显示；
- 文件名一律 ASCII：避免 git、zip、ffmpeg 三处的中文路径隐患；
- **余额 6 段刻意不登记**：目录打包时会打印“跳过未登记素材”，不会误入包内。

---

## 8. 打包与自检

```powershell
# 全量打包（约 3 分钟）
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim

# 生产格式：H.264 颜色 + 1-bit 掩码（全量约 2~3 分钟）
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/h264 --codec h264 --crf 20

# 只自检（fishanim）：逐个解帧、倒序重取一遍，确认差分链从任意位置重算都一致
cargo run -p pack-anim --release -- --verify assets/anim
```

`--dump <n>` 每隔 n 帧导一张 PNG（棋盘底），用来肉眼抽检。

---

## 9. 新增一段动画的完整步骤

假设要加 `assets/webm/新动作.webm`：

1. 在 `NAME_MAP` 加 `("新动作", "new_action")`；
2. 在 `feiyu-app/src/pet/clips.rs` 的 `CATEGORIES` 里放进对应分类；
3. 如果它有前摇/收势/位移，在 `CLIPS` 里加一条 `ClipSpec`（`span`、`walk`、`on_enter`）；
4. 重新打包；
5. `feiyu-cli.exe --list` 看它有没有出现在正确分类；
6. `feiyu-cli.exe --showcase new_action` 肉眼过一遍；
7. 跑 `--selftest`（它会自动核对“清单 100 段 → 菜单 97 段”的数量关系）。

---

## 10. 本章验收

- [ ] 能画出 webm → `.fishanim` 的流程图；
- [ ] 能解释为什么抠像必须从边框泛洪，不能全局阈值；
- [ ] 能说出脚底线为什么取众数而不是最大值；
- [ ] 能独立新增一段动画并让它在菜单里出现。
