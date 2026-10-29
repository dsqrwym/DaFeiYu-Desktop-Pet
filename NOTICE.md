# 许可与致谢

本仓库包含两类东西，授权**不一样**，再发布前请分别确认：

## 1. 代码

`crates/`、`tools/` 下的源码采用 **MIT**，全文见 [`LICENSE`](LICENSE)。

## 2. 动画素材与形象

动画素材来自 **[PC2005-cloud/dsh-pet](https://github.com/PC2005-cloud/dsh-pet)**
（`dsh-pet/assets/webm/`）。该项目的授权口径：

- 代码：MIT
- 素材（动画 / 提示词 / 源视频）：允许开源使用，**禁止商用**
- 二次创作（二创）约定：基于本项目的衍生 / 改版 / 换皮作品，在 GitHub 仓库、
  产品官网等**公开渠道**上须附上原作者 GitHub 地址：
  <https://github.com/PC2005-cloud/dsh-pet>

宠物形象为社区二创的 DeepSeek「大肥鱼」（鲸鱼娘），**非 DeepSeek 官方形象**：

- 原型「溟月」出自 B 站 [@上善无形](https://space.bilibili.com/4456176)
- DeepSeek 版（女仆鲸鱼娘）设计出自 B 站 [@ZipZipPipe](https://space.bilibili.com/4168597)
- 形象授权：[CC BY-NC-SA 4.0](https://creativecommons.org/licenses/by-nc-sa/4.0/deed.zh-hans)
  （署名 · 非商业 · 相同方式共享）
- 授权声明原文：<https://www.bilibili.com/opus/1247156596114456581>

> 「大肥鱼是遵守 CC BY-NC-SA 4.0 协议的形象，不可以以任何形式商业化。」

## 3. 另一份参考实现

`1190fasheqi/dafeiyu-pet`（Python + PySide6，三视图 PNG）只作为**交互语义参考**，
本项目不包含它的代码与图片素材。

## 4. 本仓库对素材的处理

- 原始 `.webm` **不入库**，由 `tools/fetch-assets.ps1` 从上游拉取；
- 生成的 `assets/anim/*.fishanim` 同样**不入库**，由 `tools/pack-anim` 在本地生成；
- 因此本仓库分发的只有代码与文档，素材授权仍以原项目为准。
