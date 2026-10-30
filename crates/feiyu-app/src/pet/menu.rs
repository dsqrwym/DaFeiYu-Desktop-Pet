//! Context menu: build, dispatch, play-once.

use anyhow::Result;
use feiyu_core::brain::{Mood, Stimulus};
use feiyu_core::i18n::{self, Lang};
use feiyu_platform::{CommandId, Menu};

use super::{clip, Pet, CATEGORIES};

/// 右键菜单的命令 id（含义见 [`Pet::menu`]）。
const CMD_HOME: CommandId = 1;
const CMD_TOPMOST: CommandId = 2;
const CMD_QUIT: CommandId = 3;
const CMD_HIDE: CommandId = 4;
/// 「跟随鼠标」（= 转向跟随）开关。
const CMD_FOLLOW: CommandId = 5;
/// 「点哪走哪」开关。
const CMD_CLICK_WALK: CommandId = 6;
/// 「打字联动」开关（敲键盘时陪一段工作状态动画）。
const CMD_TYPING: CommandId = 7;
/// 「语言」子菜单里的三项。
const CMD_LANG_ZH: CommandId = 8;
const CMD_LANG_EN: CommandId = 9;
const CMD_LANG_ES: CommandId = 10;

/// 「动作」子菜单里每一段的命令 id 都从这个基址往后排。
///
/// 用「基址 + 序号」而不是给 97 段各起一个常量：菜单是**数据驱动**的，
/// 加一段素材只需要动 `NAME_MAP` 和 `CATEGORIES`，命令表自动跟上。
const CMD_CLIP_BASE: CommandId = 1000;

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
    /// 「跟随鼠标」切换后的新状态。
    Follow(bool),
    /// 「点哪走哪」切换后的新状态。
    ClickWalk(bool),
    /// 「打字联动」切换后的新状态。
    Typing(bool),
    /// 界面语言切成了这一种。
    Language(Lang),
    /// 从菜单点播了一段动画。
    Played,
    /// 隐藏状态切换后的新状态（`true` = 已藏进托盘）。
    Hidden(bool),
    /// 用户要退出。
    Quit,
}

impl Pet {
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

    /// 构造菜单（自检用：查 Win32 实际结构，不弹出）。
    pub(crate) fn menu_for_test(&self) -> Result<Menu> {
        self.build_menu()
    }

    /// 菜单首项文案：藏着时是「唤出」，显示着时是「隐藏」。
    pub fn hide_label(&self) -> &'static str {
        let t = self.texts();
        if self.hidden {
            t.show
        } else {
            t.hide
        }
    }

    fn build_menu(&self) -> Result<Menu> {
        let t = self.texts();
        let topmost = self.win.is_topmost();
        let mut menu = Menu::new()?;
        menu.command(CMD_HIDE, self.hide_label())
            .command(CMD_HOME, t.home)
            .check(CMD_FOLLOW, t.follow, self.follow)
            .check(CMD_CLICK_WALK, t.click_walk, self.click_walk)
            .check(CMD_TYPING, t.typing, self.typing_enabled)
            .check(CMD_TOPMOST, t.topmost, topmost)
            .separator();
        // 「动作 → 分类 → 具体动画」：全部可播的 97 段都从这里点播，
        // 缺素材的分类/单段自动不列（菜单是数据驱动的，索引与 `menu_clips` 同源）。
        menu.submenu(t.actions, |m| {
            let mut idx = 0usize;
            for cat in CATEGORIES {
                let clips: Vec<&'static str> = cat
                    .clips
                    .iter()
                    .copied()
                    .filter(|id| self.manifest.get(id).is_some())
                    .collect();
                if clips.is_empty() {
                    continue;
                }
                let label = i18n::category(cat.label, self.lang);
                let label = if label.is_empty() { cat.label } else { label };
                let _ = m.submenu(label, |sub| {
                    for id in &clips {
                        let label = self.display_name(id);
                        sub.command(CMD_CLIP_BASE + idx as CommandId, &label);
                        idx += 1;
                    }
                });
            }
        })?;
        // 语言子菜单：每一项用**本名**，不随当前语言变。
        menu.submenu(t.language, |m| {
            for (lang, cmd) in [
                (Lang::Zh, CMD_LANG_ZH),
                (Lang::En, CMD_LANG_EN),
                (Lang::Es, CMD_LANG_ES),
            ] {
                m.check(cmd, lang.native_name(), self.lang == lang);
            }
        })?;
        menu.separator().command(CMD_QUIT, t.quit);
        Ok(menu)
    }

    /// 菜单里显示的段名：非中文优先用内置译名，没登记就回退清单里的中文名。
    fn display_name(&self, id: &str) -> String {
        i18n::clip(id, self.lang)
            .map(str::to_owned)
            .or_else(|| self.manifest.get(id).map(|a| a.name.clone()))
            .unwrap_or_else(|| id.to_owned())
    }

    /// 菜单里可点播的段（顺序 = 菜单顺序 = 命令 id 顺序）。
    fn menu_clips(&self) -> Vec<&'static str> {
        CATEGORIES
            .iter()
            .flat_map(|c| c.clips.iter().copied())
            .filter(|id| self.manifest.get(id).is_some())
            .collect()
    }

    /// 菜单里可点播的段 id（自检用：核对素材与分类表一一对应）。
    pub fn menu_clip_ids(&self) -> Vec<&'static str> {
        self.menu_clips()
    }

    /// 全部分类 `(标签, 权重, 素材 id)`。`--list` / `--showcase` 用。
    pub fn categories() -> Vec<(&'static str, u32, &'static [&'static str])> {
        CATEGORIES
            .iter()
            .map(|c| (c.label, c.weight, c.clips))
            .collect()
    }

    /// 某段素材属于哪个分类（`idle` / `sleep` / `drag` 不属于任何分类）。
    pub fn category_of(id: &str) -> Option<&'static str> {
        CATEGORIES
            .iter()
            .find(|c| c.clips.contains(&id))
            .map(|c| c.label)
    }

    /// 直接派发「动作」菜单里的第 `at` 项（自检用）。
    ///
    /// 验的是「菜单序号 ↔ 素材」这一层：`build_menu` 与 `menu_clips` 是两段
    /// 各自过滤清单的代码，一旦某处漏了一个 filter，点第 5 项会播出第 6 段的动画，
    /// 而两侧的计数仍然都是 97。
    pub(crate) fn dispatch_menu_clip(&mut self, at: usize) -> Result<()> {
        self.dispatch(CMD_CLIP_BASE + at as CommandId)?;
        Ok(())
    }

    /// 从菜单点播一段：完整演一圈就回待机（移动段会真的走一段）。
    pub(crate) fn play_once(&mut self, id: &str) -> Result<()> {
        // 睡着时点播：先把睡眠编排收掉再换段，否则演完一圈会被「醒来」逻辑拽回去。
        if self.brain.mood() == Mood::Sleep {
            self.waking = false;
            self.action_playing = false;
            self.sleep_cycle = None;
            self.stimulate(Stimulus::Awaken)?;
        }
        // 点播优先：正在走的路、正在落的惯性都停掉，别把上一段的位移带进这一段。
        self.walk = None;
        self.motion = None;
        self.landed_at = None;
        self.dissolve = None;
        // 移动类点播对齐原版：「点播会真实行走一段」。贴墙走不了就地演。
        if clip(id).is_some_and(|c| c.walk.is_some()) && self.wander_with(id) {
            return Ok(());
        }
        self.play(id)?;
        self.action_playing = true;
        Ok(())
    }

    /// 自检用：走和菜单一样的派发路径切语言。
    pub(crate) fn switch_language(&mut self, lang: Lang) -> Result<()> {
        let cmd = match lang {
            Lang::Zh => CMD_LANG_ZH,
            Lang::En => CMD_LANG_EN,
            Lang::Es => CMD_LANG_ES,
        };
        self.dispatch(cmd)?;
        Ok(())
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
            CMD_FOLLOW => {
                let next = !self.follow;
                self.follow = next;
                self.cfg.follow = next;
                self.persist_toggles();
                Ok(Action::Follow(next))
            }
            CMD_CLICK_WALK => {
                let next = !self.click_walk;
                self.click_walk = next;
                self.cfg.click_walk = next;
                self.persist_toggles();
                Ok(Action::ClickWalk(next))
            }
            CMD_TYPING => {
                let next = !self.typing_enabled;
                self.typing_enabled = next;
                self.cfg.typing.enabled = next;
                self.persist_toggles();
                Ok(Action::Typing(next))
            }
            CMD_TOPMOST => {
                // 重读一次而不是拿弹菜单前的快照：菜单是模态的，
                // 中间可能已经改过了。
                let next = !self.win.is_topmost();
                self.win.set_topmost(next);
                self.cfg.topmost = next;
                self.persist_toggles();
                Ok(Action::Topmost(next))
            }
            CMD_LANG_ZH | CMD_LANG_EN | CMD_LANG_ES => {
                let lang = match cmd {
                    CMD_LANG_EN => Lang::En,
                    CMD_LANG_ES => Lang::Es,
                    _ => Lang::Zh,
                };
                self.lang = lang;
                self.cfg.language = Some(lang);
                self.persist_toggles();
                Ok(Action::Language(lang))
            }
            CMD_QUIT => Ok(Action::Quit),
            other if other >= CMD_CLIP_BASE => {
                let at = (other - CMD_CLIP_BASE) as usize;
                let Some(id) = self.menu_clips().get(at).copied() else {
                    eprintln!("菜单命令 {other} 没有对应的动画（菜单与素材不同步），已忽略");
                    return Ok(Action::None);
                };
                let name = self.display_name(id);
                println!("点播：{name}（{id}）");
                self.play_once(id)?;
                Ok(Action::Played)
            }
            other => {
                // 菜单里没有这一项：不静默吞掉，留条线索。
                eprintln!("未知菜单命令 {other}，已忽略");
                Ok(Action::None)
            }
        }
    }
}
