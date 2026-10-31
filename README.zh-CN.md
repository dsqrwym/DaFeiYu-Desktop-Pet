# 大肥鱼桌宠（Rust 重写版）

[English](README.md) | 简体中文

<img src="docs/images/idle.png" width="180" align="right" alt="大肥鱼">

一个 Windows 桌宠，角色是社区二创的 **大肥鱼**。素材和交互参考 dsh-pet，代码用 Rust 重写，直接开 Win32 分层窗口画像素，不跑 Electron，也不用开浏览器。

> **非官方同人项目。** 大肥鱼是社区二创形象，不是 DeepSeek 官方形象；本项目与 DeepSeek 没有隶属、背书或赞助关系。完整版权与素材授权见 [NOTICE.md](NOTICE.md)。

> **AI 辅助的个人学习项目。** 本项目是本人借助 AI 大量辅助完成的 Rust 学习项目，不是生产级软件。对于使用过程中出现的任何 bug、崩溃、数据丢失或其他损失，本人在法律允许范围内不承担责任；请自行评估并承担使用风险。

## 实机演示

<p align="center"><img src="docs/images/demo.gif" width="320" alt="大肥鱼在桌面上：待机、点击回应、吃东西、动物环绕、工作动画、睡觉"></p>

六段都是本仓库的发布版在真机上跑出来的画面，录完再剪在一起：待机、点击回应、吃西瓜、动物环绕、工作动画（平时由打字触发）、睡觉。背景是录制时垫在桌宠后面的一整块窗口，所以画面里没有桌面内容。

## 上游项目

这个仓库是独立重写，不是 fork：

- **素材与交互主要上游**：[PC2005-cloud/dsh-pet](https://github.com/PC2005-cloud/dsh-pet)。动画素材来自它的 `dsh-pet/assets/webm/`；本项目取 100 段，余额那 6 段没有打包。
- **另一份交互参考**：[1190fasheqi/dafeiyu-pet](https://github.com/1190fasheqi/dafeiyu-pet)。它是 Python + PySide6 的三视图桌面宠物；这里只参考交互语义，没有使用它的代码或图片素材。

dsh-pet 是 DSH 的插件，余额、碎碎念、工作状态原本都靠 DSH 事件。这里把桌宠本身拆出来单独跑：100 段动画、待机/睡觉/拖拽/随机动作/右键点播；碎碎念和工作状态改成随机播放和打字时触发。

## 素材是怎么处理的

上游给的是 106 段 VP9 的 webm。直接播要带 libvpx，启动慢、发版也大，所以我自己写打包器。现在有两种产物：

- **生产格式**：H.264 颜色 `.fishvid` + 1-bit alpha `.fishmask`，100 段约 98MB；
- **旧格式**：`.fishanim`，预乘 BGRA + zstd，100 段约 654MB，保留兼容和对照。

两包都从“角色合在纯黑上”的源视频打包。边缘和半透明特效需要 alpha，而源视频没有，所以 `pack-anim --preview` 会再读上游带的预览 GIF（直通色 + 1-bit 透明），逐像素解 `a = |C| / |F|`。上面两行比首个版本大，原因就在这里：半透明区域变亮是要花码率的。v0.1.1 的发布包早于这遍处理。

H.264 颜色由 Media Foundation 解码，alpha 不交给视频编码，单独存 1 bit/像素，所以透明边缘没有被有损压缩糊掉。运行时仍然没有第三方解码器依赖。

旧格式取一帧解一帧，单帧约 0.27ms；H.264 原型单帧约 0.24ms（解码 + 套掩码），两者都在 24fps 下很轻。

## 仓库里有什么

GitHub 仓库只提交源码和文档。动画素材、打包产物和上游 clone 都不入库：

| 目录 | 为什么不在仓库里 |
|---|---|
| `assets/` | 由 `pack-anim` 生成（约 80MB），随 GitHub Release 分发 |
| `dist/` | 由 `package.ps1` 生成；二进制只放在 Releases |
| `_upstream/` | 由 `fetch-assets.ps1` 浅克隆上游，供本地打包 |
| `target/` | Cargo 构建缓存 |
| `config.json` | 本地运行配置，出厂值在代码里 |

所以克隆仓库后不能直接双击运行，先下载 Release，或者按下面的源码步骤自己打包。

## 直接运行（下载 Release）

不需要 Rust。下载 **v0.1.1 发布包（H.264 / legacy）**：

- [feiyu-h264-v0.1.1.zip](https://github.com/dsqrwym/DaFeiYu-Desktop-Pet/releases/download/v0.1.1/feiyu-h264-v0.1.1.zip)（79.95MB，推荐）
- [feiyu-fishanim-v0.1.1.zip](https://github.com/dsqrwym/DaFeiYu-Desktop-Pet/releases/download/v0.1.1/feiyu-fishanim-v0.1.1.zip)（625.70MB，legacy）
- [Release 页面](https://github.com/dsqrwym/DaFeiYu-Desktop-Pet/releases/tag/v0.1.1)

两个包都在同一个 `v0.1.1` Release 里。默认选 H.264；只有明确在意运行内存或系统没有 Media Foundation H.264 解码器时，才选 legacy `.fishanim`。

v0.1.1 用新的抠像重打了这 100 段。上游 webm 是黑底合成，旧抠像把所有非背景像素都给满不透明，于是轮廓一圈黑边、本应半透明的地方（气泡、烟雾、火星）变成实心暗块。现在边缘一圈按“里侧邻居的颜色”反推 alpha，H.264 的颜色通道改存直通色。算法与已知局限见 `docs/07`。

解压后进入 `feiyu/`：

```
feiyu/
  feiyu.exe       双击运行，没有控制台窗口
  feiyu-cli.exe   命令行诊断（--selftest / --list / --showcase）
  config.json     可调参数，改完重启生效
  assets/anim/    100 段 .fishvid + 100 段 .fishmask（legacy 包为 .fishanim）
```

素材就在 exe 旁边，不需要装 Rust。两个 exe 各约 0.8MB（共约 1.6MB），图标已经嵌进资源段。

为什么要两个 exe：Windows 的 PE 只能有一个子系统。console 子系统双击会弹终端，GUI 子系统又拿不到同步的命令行输出。所以 GUI 版负责日常运行，console 版负责诊断，两者跑的是同一份代码。

```powershell
.\feiyu-cli.exe --selftest
.\feiyu-cli.exe --list
.\feiyu-cli.exe --config other.json
```

校验下载包：

```powershell
Get-FileHash .\feiyu-h264-v0.1.1.zip -Algorithm SHA256
```

SHA256（GitHub Release 附件 digest 与本地构建一致）：

```text
429F62E262DCDEF684DB05D3E7E98FE37B7133BB7C01310411930B76A95B3304
```

legacy `.fishanim` 包的 SHA256：

```text
764BEF44E5E0DE2619EB849E505003C3867C2C882810AD630E708E546F358BB3
```

### 两个包怎么选

| | H.264（推荐） | `.fishanim` legacy |
|---|---:|---:|
| zip / 解压后 | **79.95MB / 80.8MB** | **625.70MB / 626.4MB** |
| 可见 RSS | 77.2MB | **10.5MB** |
| 可见私有内存 | 65.2MB | **2.4MB** |
| 线程 | 53 | **4** |
| 可见 CPU | 约 5%~7.5% 单核 | **0.3%~2.8% 单核** |
| 解码依赖 | Windows Media Foundation H.264 | **无视频解码器** |
| 随机访问 | Media Foundation seek | **差分链直接重算** |
| 画面 | H.264 yuv420p + 1-bit alpha | **预乘 BGRA + zstd，alpha 完整** |
| 适合 | 普通用户、优先省磁盘 | 内存敏感、老系统、优先低底噪 |

两包都是 100 段动画，`--selftest` 都是 **94/94**。H.264 省约 8 倍磁盘，但 Media Foundation 解码器会拉起线程池和内部缓冲，所以内存/线程/CPU 明显更高；这不是泄漏，隐藏后 CPU 为 0。如果你更在意运行资源，而不是 Release 体积，选 `.fishanim`。

## 想改参数

`feiyu/config.json` 是解压目录里给用户改的，不用重新编译。没有这个文件就用出厂值；写坏了会打一行告警、退回默认值，不会起不来。

- `language`：`auto`（跟随系统界面语言）或 `zh` / `en` / `es`；右键菜单的「语言」子菜单改的就是这一项
- `follow` / `click_walk` / `topmost`：对应菜单里的「跟随鼠标 / 点哪走哪 / 总在最前」，在菜单里勾一下就会存进这里
- `action_gap`：两次待机小动作之间隔多久（秒，区间内随机）
- `sleep_after`：多久没互动就睡（秒）
- `typing`：打字联动的开关、触发概率、热度阈值、衰减速度、冷却
- `walk`：自己走一段的近远（像素）
- `category_weights`：各分类进随机池的权重，`0` = 只留在菜单里，不参与随机

分类权重是**整类**的权重，再平均分给该类每一段；数值大只代表相对更常抽到。手改完重启 `feiyu.exe` 生效。`--config other.json` 可以临时指另一份。

菜单只会写回它自己能改的几项（`language`、`follow`、`click_walk`、`topmost`、`typing.enabled`），其余字段手写成什么样就保持什么样。右键菜单支持中文 / English / Español；动画名用的是内置译名表，所以已发布的素材包不用重打。

## 从源码构建

环境：Windows 10/11、Rust stable、VS 2022 C++ 生成工具（链接器），打包素材还需要 ffmpeg。

```powershell
git clone https://github.com/dsqrwym/DaFeiYu-Desktop-Pet.git
cd DaFeiYu-Desktop-Pet
powershell -ExecutionPolicy Bypass -File tools/fetch-assets.ps1
# 生产格式：H.264 + 1-bit 掩码
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/h264 --codec h264 --crf 20 --preview _upstream/dsh-pet/dsh-pet/assets/preview
# 旧格式（可选，保留兼容/对照）
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim --preview _upstream/dsh-pet/dsh-pet/assets/preview
cargo run -p feiyu-app --release
```

`fetch-assets.ps1` 会把上游浅克隆到 `_upstream/dsh-pet`；这个目录和 `assets/` 都不会进 Git。

`cargo run` 默认优先 `assets/h264`（H.264），没有这个目录时才回退到 `assets/anim`（旧 `.fishanim`）；也可以用 `--dir` 显式指定。

`cargo run` 默认跑 console 版（`default-run = "feiyu-cli"`），日志和退出码都在终端里。

打发布包：

```powershell
powershell -ExecutionPolicy Bypass -File tools/package.ps1 -AnimDir assets/h264
```

## 和原版比

| | 原版 [dsh-pet](https://github.com/PC2005-cloud/dsh-pet) | 这个版本 |
|---|---|---|
| 运行方式 | DSH 插件：浏览器 overlay 或 Electron 透明窗 | 原生 Win32 分层窗口 |
| 依赖 | DSH + Node；桌面模式首次下载 Electron 约 104MB | 无第三方运行时；H.264 用 Windows 自带 Media Foundation |
| 磁盘 | webm 51.8MB + Electron 约 104MB 下载 | **80.8MB**（H.264 + 掩码 + 两个 exe）；旧 `.fishanim` 包 626.4MB |
| 内存 | 没在本机装 DSH 实测 | H.264 包可见 77.2MB RSS / 65.2MB 私有 / 53 线程；旧 `.fishanim` 包 10.5MB / 2.4MB / 4 线程 |
| CPU | 没实测 | H.264 包可见约 5%~7.5% 单核（12 核整机 0.4%~0.6%）；旧 `.fishanim` 包 0.3%~2.8% 单核 |
| 单帧 | — | H.264 约 0.24ms（解码 + 掩码）；旧 `.fishanim` 0.265ms |

磁盘现在和原版同一量级（81MB vs webm 51.8MB + Electron 约 104MB）。旧 `.fishanim` 包 626MB 保留作对照，它换来的是“不依赖 Media Foundation、逐帧随机访问最直接”。

### H.264 包的实测（单独一组，不和旧包混）

用发布包里的 `feiyu.exe --run 45`，5 秒预热后取 30 秒窗口：

- 可见：RSS **77.2MB**、私有 **65.2MB**、**53 线程**、CPU **4.9%~7.5% 单核**（12 核整机 0.4%~0.6%，三次采样）；
- 隐藏：RSS **39.6MB**、私有 **21.9MB**、19 线程、CPU **0%**；
- 全量 `--showcase`：**100/100 段**、退出码 0、墙钟 **16.7 分钟**。

H.264 包比旧 `.fishanim` 包多占约 7 倍内存、线程数也明显更多——这是 Media Foundation 解码器/线程池的代价；CPU 折到 12 核整机只有 0.4%~0.6%，但单核读数确实比旧包高。旧包的 10.5MB / 4 线程 / 0.3%~2.8% 是另一套数字，不要和这一组平均。

CPU 波动是因为窗口被遮挡时 DWM 会跳过合成；旧包 60 秒采样的历史值在 1.4%~1.7% 单核。

## 功能

- 待机、睡觉（三段编排）、点击回应（5 段随机）、拖拽甩抛、自己走一段
- 97 段动画可以从右键菜单点播，89 段进随机池
- 跟随鼠标转头、点哪走哪、打字时陪一段工作动画，都能在菜单里关
- 隐藏到托盘时把图标定格成收走那一刻的样子

## 验证

```powershell
# 解压后的发布包
.\feiyu-cli.exe --selftest # 94 项
.\feiyu-cli.exe --showcase # 100/100，约 16.7 分钟

# 源码构建后
cargo test --workspace --release          # 115
.\target\release\feiyu-cli.exe --selftest # 94 项
```

`selftest` 真的开窗口、真的提交位图、真的抓屏，覆盖透明、命中测试、菜单结构、托盘、脚底对齐、睡眠帧区间、打字联动。`cargo test` 只测纯逻辑。

全量演示（解压目录）：

```powershell
.\feiyu-cli.exe --showcase
```

100 段各播完整一圈，约 16.7 分钟；本次实际跑完 100/100，退出码 0。

常用诊断（解压目录或源码的 `target/release/`）：

```powershell
.\feiyu-cli.exe --list
.\feiyu-cli.exe --showcase 移动
.\feiyu-cli.exe --typing-probe 6
.\feiyu-cli.exe --menu
```

## 文档

`docs/` 是写给不会 Rust 的人看的：从跑起来、语法、架构，到每个文件、每个函数、踩过的坑。

- 中文完整版：[docs/README.md](docs/README.md)
- English edition：[docs/en/README.md](docs/en/README.md)

## 版权与非官方声明

- **代码**：Copyright © 2026 dsqrwym 与 FeiYu contributors，采用 MIT，见 [LICENSE](LICENSE)。
- **角色与动画**：社区二创，**不是 DeepSeek 官方形象**；素材不适用 MIT。素材来自 [PC2005-cloud/dsh-pet](https://github.com/PC2005-cloud/dsh-pet)，上游条款禁止商用并要求署名；形象授权为 CC BY-NC-SA 4.0。完整署名与条款见 [NOTICE.md](NOTICE.md)。
- **无隶属关系**：本项目与 DeepSeek 没有隶属、背书或赞助关系；“DeepSeek”及相关商标归各自权利人所有，这里只用于描述角色。
- **AI 辅助学习项目**：代码和文档是本人为学习 Rust 借助 AI 大量辅助完成的，不是生产级软件，也没有经过安全、稳定性或适用性审计。
- **免责声明**：对于使用本项目导致的任何 bug、崩溃、数据丢失、系统损坏或其他损失，本人在法律允许范围内不承担责任；使用前请自行评估和测试。
- **无担保**：软件与素材按“现状”提供，不附带任何明示或默示担保。
