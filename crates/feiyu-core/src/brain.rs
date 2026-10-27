//! 状态机：把「用户动作 + 时间」映射成「该播哪段动画」。
//!
//! 刻意做成**纯逻辑**：不碰窗口、不读时钟，调用方负责每次喂 `advance(dt)` 或
//! `stimulus(..)`，状态机只回答「现在该播哪个 id」。好处是全部转移都能在单元测试里钉死，
//! 不需要真的开一个窗口等 25 秒看它会不会睡觉。

/// 角色当前姿态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    /// 待机。
    Idle,
    /// 长时间无互动，睡着。
    Sleep,
    /// 被点了一下，开心。
    Happy,
    /// 短时间内被连点，生气。
    Pout,
    /// 正被拖拽。
    Dragged,
}

impl Mood {
    /// 对应的素材 id（与 `manifest.json` 的 `animations[].id` 对齐）。
    pub fn anim_id(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Sleep => "sleep",
            Mood::Happy => "click_happy",
            Mood::Pout => "click_pout",
            Mood::Dragged => "drag",
        }
    }

    /// 日志用的中文名。
    pub fn label(self) -> &'static str {
        match self {
            Mood::Idle => "待机",
            Mood::Sleep => "睡觉",
            Mood::Happy => "开心",
            Mood::Pout => "生气",
            Mood::Dragged => "被拖拽",
        }
    }
}

/// 外部刺激。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stimulus {
    /// 左键点了一下（按下又抬起）。
    Click,
    /// 开始拖拽。
    DragStart,
    /// 松手结束拖拽。
    DragEnd,
}

/// 连续点击的统计窗口（秒）：窗口内的点击才算“连点”。
const POKE_WINDOW: f32 = 2.0;

/// 连点几次会生气。
const POKES_TO_POUT: u32 = 3;

/// 默认多久不互动就睡觉（秒）。
pub const DEFAULT_SLEEP_AFTER: f32 = 25.0;

/// 桌宠状态机。
#[derive(Debug, Clone)]
pub struct Brain {
    mood: Mood,
    /// 当前姿态已持续的秒数。
    elapsed: f32,
    /// 连点统计窗口剩余秒数。
    poke_window: f32,
    /// 窗口内累计的点击数。
    pokes: u32,
    /// 多久不互动会睡着。
    sleep_after: f32,
}

impl Default for Brain {
    fn default() -> Self {
        Self::new(DEFAULT_SLEEP_AFTER)
    }
}

impl Brain {
    /// 新建，`sleep_after` 为无互动多久后睡觉（秒）。
    pub fn new(sleep_after: f32) -> Self {
        Self { mood: Mood::Idle, elapsed: 0.0, poke_window: 0.0, pokes: 0, sleep_after }
    }

    /// 当前姿态。
    pub fn mood(&self) -> Mood {
        self.mood
    }

    /// 当前该播的素材 id。
    pub fn anim_id(&self) -> &'static str {
        self.mood.anim_id()
    }

    /// 时间前进 `dt` 秒。返回姿态是否发生变化。
    ///
    /// 只拦非法值（NaN / 负数 / 无穷），**不设上限**：上限会把「电脑休眠两小时后唤醒」
    /// 误判成「只过了一秒」，那样桌宠永远睡不着。按帧喂小步长是调用方的事。
    pub fn advance(&mut self, dt: f32) -> bool {
        if !dt.is_finite() || dt <= 0.0 {
            return false;
        }

        if self.poke_window > 0.0 {
            self.poke_window -= dt;
            if self.poke_window <= 0.0 {
                self.poke_window = 0.0;
                self.pokes = 0;
            }
        }

        self.elapsed += dt;

        // 只有在待机且无人搭理时才睡；被拖拽/生气时不该突然睡着。
        if self.mood == Mood::Idle && self.elapsed >= self.sleep_after {
            return self.switch_to(Mood::Sleep);
        }
        false
    }

    /// 处理一次刺激。返回姿态是否发生变化。
    pub fn stimulus(&mut self, s: Stimulus) -> bool {
        match s {
            Stimulus::Click => {
                self.pokes = self.pokes.saturating_add(1);
                self.poke_window = POKE_WINDOW;
                // 睡着时点一下先醒过来，不算“戳烦了”。
                let next = if self.pokes >= POKES_TO_POUT { Mood::Pout } else { Mood::Happy };
                self.switch_to(next)
            }
            Stimulus::DragStart => {
                self.pokes = 0;
                self.poke_window = 0.0;
                self.switch_to(Mood::Dragged)
            }
            Stimulus::DragEnd => self.switch_to(Mood::Idle),
        }
    }

    /// 切换姿态并重置计时；姿态未变则返回 `false`（调用方据此决定要不要换动画）。
    fn switch_to(&mut self, next: Mood) -> bool {
        if self.mood == next {
            // 同一姿态重复触发（例如连点仍在窗口内）也重置计时：
            // 否则「开心」播到一半会因为计时到点又切回别的。
            self.elapsed = 0.0;
            return false;
        }
        self.mood = next;
        self.elapsed = 0.0;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_idle() {
        let b = Brain::default();
        assert_eq!(b.mood(), Mood::Idle);
        assert_eq!(b.anim_id(), "idle");
    }

    #[test]
    fn falls_asleep_after_quiet_time() {
        let mut b = Brain::new(25.0);
        // 累计 24.9 秒，还不到点。
        for _ in 0..249 {
            assert!(!b.advance(0.1));
        }
        assert_eq!(b.mood(), Mood::Idle);
        assert!(b.advance(0.2), "跨过 25 秒应当有状态变化");
        assert_eq!(b.mood(), Mood::Sleep);
        assert_eq!(b.anim_id(), "sleep");

        // 睡着之后继续推进，不应再产生变化（也不该反复触发换动画）。
        assert!(!b.advance(0.5));
        assert_eq!(b.mood(), Mood::Sleep);
    }

    #[test]
    fn click_wakes_up_and_resets_timer() {
        let mut b = Brain::new(1.0);
        b.advance(1.5);
        assert_eq!(b.mood(), Mood::Sleep);

        assert!(b.stimulus(Stimulus::Click), "睡着时被点应当醒来");
        assert_eq!(b.mood(), Mood::Happy);
        assert_eq!(b.anim_id(), "click_happy");

        // 计时被重置：再等 0.9 秒不会立刻又睡。
        assert!(!b.advance(0.9));
        assert_eq!(b.mood(), Mood::Happy);
        // 但“开心”本身不是待机，永远不会自动转睡；需要先回到 Idle。
        assert!(!b.advance(5.0));
        assert_eq!(b.mood(), Mood::Happy);
    }

    #[test]
    fn rapid_pokes_annoy() {
        let mut b = Brain::new(25.0);
        assert!(b.stimulus(Stimulus::Click));
        assert_eq!(b.mood(), Mood::Happy);
        assert!(!b.stimulus(Stimulus::Click), "第二次点击仍是开心，姿态不变");
        assert_eq!(b.mood(), Mood::Happy);
        assert!(b.stimulus(Stimulus::Click), "第三次点击该生气了");
        assert_eq!(b.mood(), Mood::Pout);
        assert_eq!(b.anim_id(), "click_pout");
    }

    #[test]
    fn slow_pokes_do_not_annoy() {
        let mut b = Brain::new(25.0);
        for _ in 0..3 {
            b.stimulus(Stimulus::Click);
            // 间隔 3 秒超过 2 秒统计窗口，计数被清零。
            b.advance(3.0);
            assert_eq!(b.mood(), Mood::Happy, "间隔够长不应当生气");
        }
    }

    #[test]
    fn drag_takes_priority_and_releases_to_idle() {
        let mut b = Brain::new(1.0);
        b.advance(5.0);
        assert_eq!(b.mood(), Mood::Sleep);

        assert!(b.stimulus(Stimulus::DragStart));
        assert_eq!(b.mood(), Mood::Dragged);
        assert_eq!(b.anim_id(), "drag");

        // 拖拽期间无论多久都不该睡。
        for _ in 0..100 {
            b.advance(0.1);
        }
        assert_eq!(b.mood(), Mood::Dragged);

        assert!(b.stimulus(Stimulus::DragEnd));
        assert_eq!(b.mood(), Mood::Idle);
        // 松手后计时重置，立刻再等 0.5 秒不该睡。
        assert!(!b.advance(0.5));
        assert_eq!(b.mood(), Mood::Idle);
    }

    #[test]
    fn every_mood_maps_to_a_unique_anim() {
        let moods = [Mood::Idle, Mood::Sleep, Mood::Happy, Mood::Pout, Mood::Dragged];
        let mut ids: Vec<&str> = moods.iter().map(|m| m.anim_id()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "不同姿态必须对应不同素材");
        assert!(moods.iter().all(|m| !m.anim_id().is_empty() && !m.label().is_empty()));
    }

    #[test]
    fn long_gap_puts_it_to_sleep() {
        let mut b = Brain::new(25.0);
        // 休眠两小时后唤醒：这段空档必须算数，不能被内部上限吃掉。
        assert!(b.advance(7200.0), "长时间空档应直接睡着");
        assert_eq!(b.mood(), Mood::Sleep);
    }

    #[test]
    fn ignores_bogus_dt() {
        let mut b = Brain::new(1.0);
        assert!(!b.advance(f32::NAN));
        assert!(!b.advance(-5.0));
        assert!(!b.advance(f32::INFINITY), "无穷大按 0 处理，不应瞬间睡着");
        assert_eq!(b.mood(), Mood::Idle);
    }
}
