# 大肥鱼桌宠（Rust 重写版）

<img src="docs/images/idle.png" width="180" align="right" alt="大肥鱼">

DeepSeek 鲸鱼娘「大肥鱼」的 Windows 桌宠。素材和交互参考 dsh-pet，代码用 Rust 重写，
直接开 Win32 分层窗口画像素，不跑 Electron，也不用开浏览器。

上游 dsh-pet 是 DSH 的插件，余额、碎碎念、工作状态都靠 DSH 的事件。我把桌宠本身
拆出来单独跑：100 段动画、待机/睡觉/拖拽/随机动作/右键点播。余额那 6 段没有打包；
碎碎念和工作状态的动画保留，改成随机播放和打字时触发。

## 素材是怎么处理的

上游给的是 106 段 VP9 的 webm。直接播要带 libvpx，启动慢、发版也大，所以我自己写打包器。
现在有两种产物：

- **生产格式**：H.264 颜色 `.fishvid` + 1-bit alpha `.fishmask`，100 段约 71MB；
- **旧格式**：`.fishanim`，预乘 BGRA + zstd，100 段约 582MB，保留兼容和对照。

H.264 颜色由 Media Foundation 解码，alpha 不交给视频编码，单独存 1 bit/像素，
所以透明边缘没有被有损压缩糊掉。运行时仍然没有第三方解码器依赖。

旧格式取一帧解一帧，单帧约 0.27ms；H.264 原型单帧约 0.24ms（解码 + 套掩码），
两者都在 24fps 下很轻。

## 直接运行

`dist/feiyu/` 是打好的包（H.264 格式，约 71MB）：

```
dist/feiyu/
  feiyu.exe       双击运行，没有控制台窗口
  feiyu-cli.exe   命令行诊断（--selftest / --list / --showcase）
  config.json     可调参数，改完重启生效
  assets/anim/    100 段 .fishvid + 100 段 .fishmask
```

素材就在 exe 旁边，不需要装 Rust。两个 exe 各约 0.8MB（共约 1.6MB），图标已经嵌进资源段。

为什么要两个 exe：Windows 的 PE 只能有一个子系统。console 子系统双击会弹终端，
GUI 子系统又拿不到同步的命令行输出。所以 GUI 版负责日常运行，console 版负责诊断，
两者跑的是同一份代码。

```powershell
.\dist\feiyu\feiyu-cli.exe --selftest
.\dist\feiyu\feiyu-cli.exe --list
.\dist\feiyu\feiyu-cli.exe --config other.json
```

## 想改参数

`dist/feiyu/config.json` 是给用户改的，不用重新编译。没有这个文件就用出厂值；
写坏了会打一行告警、退回默认值，不会起不来。

- `action_gap`：两次待机小动作之间隔多久（秒，区间内随机）
- `sleep_after`：多久没互动就睡（秒）
- `typing`：打字联动的开关、触发概率、热度阈值、衰减速度、冷却
- `walk`：自己走一段的近远（像素）
- `category_weights`：各分类进随机池的权重，`0` = 只留在菜单里，不参与随机

分类权重是**整类**的权重，再平均分给该类每一段；数值大只代表相对更常抽到。
改完重启 `feiyu.exe` 生效。`--config other.json` 可以临时指另一份。

## 从源码构建

环境：Windows 10/11、Rust stable、VS 2022 C++ 生成工具（链接器），打包素材还需要 ffmpeg。

```powershell
powershell -ExecutionPolicy Bypass -File tools/fetch-assets.ps1
# 生产格式：H.264 + 1-bit 掩码
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/h264 --codec h264 --crf 20
# 旧格式（可选，保留兼容/对照）
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim
cargo run -p feiyu-app --release
```

`cargo run` 默认跑 console 版（`default-run = "feiyu-cli"`），日志和退出码都在终端里。

打发布包：

```powershell
powershell -ExecutionPolicy Bypass -File tools/package.ps1 -AnimDir assets/h264
```

## 和原版比

| | 原版 dsh-pet | 这个版本 |
|---|---|---|
| 运行方式 | DSH 插件：浏览器 overlay 或 Electron 透明窗 | 原生 Win32 分层窗口 |
| 依赖 | DSH + Node；桌面模式首次下载 Electron 约 104MB | 无，解压就能跑 |
| 磁盘 | webm 51.8MB + Electron 约 104MB 下载 | **71.1MB**（H.264 + 掩码 + 两个 exe）；旧 `.fishanim` 包 583.1MB |
| 内存 | 没在本机装 DSH 实测 | H.264 包可见 55.0MB RSS / 39.8MB 私有 / 40 线程；旧 `.fishanim` 包 10.5MB / 2.4MB / 4 线程 |
| CPU | 没实测 | H.264 包可见约 7.9% 单核（12 核整机 0.66%）；旧 `.fishanim` 包 0.3%~2.8% 单核 |
| 单帧 | — | H.264 约 0.24ms（解码 + 掩码）；旧 `.fishanim` 0.265ms |

磁盘现在和原版同一量级（71MB vs webm 51.8MB + Electron 约 104MB）。
旧 `.fishanim` 包 583MB 保留作对照，它换来的是“不依赖 Media Foundation、逐帧随机访问最直接”。
原版的内存我没有实测，不写估数；能确定的是 Electron/Chromium 的底噪远不止 10MB。

### H.264 包的实测（单独一组，不和旧包混）

用 `dist/feiyu/feiyu.exe --run 45`，5 秒预热后取 30 秒窗口：

- 可见：RSS **55.0MB**、私有 **39.8MB**、**40 线程**、CPU **7.91% 单核**（12 核整机 0.66%）；
- 隐藏：RSS **39.7MB**、私有 **22.1MB**、19 线程、CPU **0%**；
- 全量 `--showcase`：**100/100 段**、退出码 0、墙钟 **16.7 分钟**。

H.264 包比旧 `.fishanim` 包多占约 5 倍内存、线程数也明显更多——这是 Media Foundation
解码器/线程池的代价；CPU 折到 12 核整机只有 0.66%，但单核读数确实比旧包高。
旧包的 10.5MB / 4 线程 / 0.3%~2.8% 是另一套数字，不要和这一组平均。

CPU 波动是因为窗口被遮挡时 DWM 会跳过合成；旧包 60 秒采样的历史值在 1.4%~1.7% 单核。

## 功能

- 待机、睡觉（三段编排）、点击回应（5 段随机）、拖拽甩抛、自己走一段
- 97 段动画可以从右键菜单点播，89 段进随机池
- 跟随鼠标转头、点哪走哪、打字时陪一段工作动画，都能在菜单里关
- 隐藏到托盘时把图标定格成收走那一刻的样子

## 验证

```powershell
cargo test --workspace --release          # 110
.\target\release\feiyu-cli.exe --selftest # 94 项
```

`selftest` 真的开窗口、真的提交位图、真的抓屏，覆盖透明、命中测试、菜单结构、托盘、
脚底对齐、睡眠帧区间、打字联动。`cargo test` 只测纯逻辑。

全量演示：

```powershell
.\dist\feiyu\feiyu-cli.exe --showcase
```

100 段各播完整一圈，约 16.7 分钟；本次实际跑完 100/100，退出码 0。

常用诊断：

```powershell
.\target\release\feiyu-cli.exe --list
.\target\release\feiyu-cli.exe --showcase 移动
.\target\release\feiyu-cli.exe --typing-probe 6
.\target\release\feiyu-cli.exe --menu
```

## 文档

`docs/` 是写给不会 Rust 的人看的：从跑起来、语法、架构，到每个文件、每个函数、
踩过的坑。入口在 [docs/README.md](docs/README.md)。

## 许可

代码 MIT。动画素材来自 [dsh-pet](https://github.com/PC2005-cloud/dsh-pet)，
形象「大肥鱼」是 CC BY-NC-SA 4.0，禁止商用；再发布请保留原作者信息，
详见 [NOTICE.md](NOTICE.md)。
