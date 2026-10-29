# 大肥鱼桌宠（Rust 重写版）

<img src="docs/images/idle.png" width="180" align="right" alt="大肥鱼">

DeepSeek 鲸鱼娘「大肥鱼」的 Windows 桌宠。素材和交互参考 dsh-pet，代码用 Rust 重写，
直接开 Win32 分层窗口画像素，不跑 Electron，也不用开浏览器。

上游 dsh-pet 是 DSH 的插件，余额、碎碎念、工作状态都靠 DSH 的事件。我把桌宠本身
拆出来单独跑：100 段动画、待机/睡觉/拖拽/随机动作/右键点播。余额那 6 段没有打包；
碎碎念和工作状态的动画保留，改成随机播放和打字时触发。

## 素材是怎么处理的

上游给的是 106 段 VP9 + alpha 的 webm。直接播要带 libvpx，启动慢、发版也大，所以我
写了个打包器，把 webm 转成自己的 `.fishanim`：预乘 BGRA + zstd，取一帧解一帧，
解出来直接交给 `UpdateLayeredWindow`。

这样运行时没有任何解码依赖，24fps 下单帧大约 0.27ms。代价是体积：100 段约 580MB，
原始 webm 只有 52MB。

## 直接运行

`dist/feiyu/` 是打好的包：

```
dist/feiyu/
  feiyu.exe       双击运行，没有控制台窗口
  feiyu-cli.exe   命令行诊断（--selftest / --list / --showcase）
  config.json     可调参数，改完重启生效
  assets/anim/    100 段 .fishanim
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
cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim
cargo run -p feiyu-app --release
```

`cargo run` 默认跑 console 版（`default-run = "feiyu-cli"`），日志和退出码都在终端里。

打发布包：

```powershell
powershell -ExecutionPolicy Bypass -File tools/package.ps1
```

## 和原版比

| | 原版 dsh-pet | 这个版本 |
|---|---|---|
| 运行方式 | DSH 插件：浏览器 overlay 或 Electron 透明窗 | 原生 Win32 分层窗口 |
| 依赖 | DSH + Node；桌面模式首次下载 Electron 约 104MB | 无，解压就能跑 |
| 磁盘 | webm 51.8MB + Electron 约 104MB 下载 | 583.1MB（两个 exe 共 1.6MB + 动画 581.6MB） |
| 内存 | 没在本机装 DSH 实测 | 10.5MB RSS / 2.4MB 私有 / 4 线程 |
| CPU | 没实测 | 可见时约 0.3%~2.8% 单核（12 核整机 0.1%~0.25%），隐藏 0% |
| 单帧 | — | 0.265ms（解码 0.191 + 提交 0.074） |

磁盘我们更大，原因就是前面说的“不带解码器”。原版的内存我没有实测，不写估数；
能确定的是 Electron/Chromium 的底噪远不止 10MB。

CPU 波动是因为窗口被遮挡时 DWM 会跳过合成；60 秒采样的历史值在 1.4%~1.7% 单核。

## 功能

- 待机、睡觉（三段编排）、点击回应（5 段随机）、拖拽甩抛、自己走一段
- 97 段动画可以从右键菜单点播，89 段进随机池
- 跟随鼠标转头、点哪走哪、打字时陪一段工作动画，都能在菜单里关
- 隐藏到托盘时把图标定格成收走那一刻的样子

## 验证

```powershell
cargo test --workspace --release          # 97
.\target\release\feiyu-cli.exe --selftest # 94 项
```

`selftest` 真的开窗口、真的提交位图、真的抓屏，覆盖透明、命中测试、菜单结构、托盘、
脚底对齐、睡眠帧区间、打字联动。`cargo test` 只测纯逻辑。

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
