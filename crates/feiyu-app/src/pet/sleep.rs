//! Sleep: wake-up rolls and the sleep loop span.

use anyhow::Result;
use feiyu_core::brain::{Mood, Stimulus};
use feiyu_core::sleep::{span_to_frames, SleepStage};

use super::Pet;

/// 睡着时，系统有键鼠输入就按这个间隔掷一次「自然醒」的骰子（秒）。
///
/// 不按输入事件逐次掷：鼠标一动就是几十条消息，逐次掷会让它一秒不到就醒。
/// 按秒掷才符合「你在电脑前坐了一阵，它被你弄醒了」的观感。
pub(crate) const WAKE_ROLL_EVERY: f32 = 1.0;

/// 每次掷骰醒来的概率。
///
/// 10% → 持续操作时平均十秒醒一次：既有「有几率」的随机感，
/// 又不会让人敲一下键盘它就从床上掉下来。
pub(crate) const WAKE_ROLL_CHANCE: f32 = 0.10;

/// 最后一次键鼠操作在这个时间内（毫秒）算「人正在电脑前」。
///
/// 比 `WAKE_ROLL_EVERY` 略短：要的是「持续在动」，偶尔碰一下鼠标不算。
pub(crate) const INPUT_FRESH_MS: u32 = 700;

impl Pet {
    /// 开始一次「醒来」：从**当前帧**接着演到最后一帧，播完回待机。
    ///
    /// 不直接 `stimulate(Click)`：那会切到 `click_happy`，观众看到的是
    /// 鱼从躺着一下跳到站着笑，像被剪了一帧。素材里本来就有睁眼起身的过程，
    /// 不花额外成本就能把它演完。
    ///
    /// 也不能从固定的第 170 帧起播（旧实现）：睡着时 Z 正挂在中间，一下跳到
    /// Z 已经消失的 170，画面差实测 11.7——比循环接缝还大一倍，用户说的「闪一下」
    /// 就是它。从当前帧往后演，Z 会在 149~173 自己飘散掉，一帧都没跳过。
    ///
    /// 睡到一半（`Falling`，还闭着眼往下沉）被点则直接醒：那时跳到起身段反而突兀。
    pub(crate) fn wake_up(&mut self) -> Result<()> {
        if self.waking || self.brain.mood() != Mood::Sleep {
            return Ok(());
        }
        // 当前帧换成比例：`span_to_frames` 的 round 是精确的，传进去多少换回来还是多少。
        let count = self.current_count();
        let from = match count.checked_sub(1) {
            Some(last) if last > 0 => self.frame.min(last) as f32 / last as f32,
            _ => 0.0,
        };
        match self.sleep_cycle.as_mut() {
            Some(cycle) if cycle.stage() == SleepStage::Asleep => cycle.start_waking(from),
            _ => return self.stimulate(Stimulus::Awaken),
        }
        self.apply_sleep_span();
        // 正在淡接缝时被点醒：淡入的两端已经不相关了，直接收掉。
        self.dissolve = None;
        self.frame = self.loop_span.map_or(0, |(lo, _)| lo);
        self.waking = true;
        // 复用「播完一圈就收尾」那条路：醒来段不回绕，一落到段尾就交回 `step_frames`。
        self.action_playing = true;
        self.draw_frame()
    }

    /// 按睡眠编排的当前阶段重算循环区间。
    pub(crate) fn apply_sleep_span(&mut self) {
        let Some(cycle) = self.sleep_cycle.as_ref() else {
            return;
        };
        let count = self.current_count();
        self.loop_span = Some(span_to_frames(cycle.span(), count));
    }
}
