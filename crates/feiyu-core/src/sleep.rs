//! 睡觉的三段编排。
//!
//! `sleep.fishanim` 是一整段 10 秒的闭环：**睁眼 → 闭眼 → 冒出 Z → 睁眼醒来**。
//! 素材里根本没有「一直睡着」的状态，所以把它当普通动画循环播放，看到的
//! 就是「慢慢睡着、打两下呼、又自己醒了」——鱼睡了十秒就醒，像睡不踏实。
//!
//! 这里把它拆成两截，边界是**实测出来的**（`feiyu --dump sleep` 每 5 帧导一张图，
//! 拼成 7×7 的图看清了每个转折）：
//!
//! | 帧段 | 内容 |
//! |---|---|
//! | 0~32 | 睁眼，清醒 |
//! | 33~70 | 闭眼，头慢慢低下去，还没有 Z |
//! | **75~165** | **睡着打呼，Z 一直在**（用户要的就是停在这里） |
//! | 170~205 | 闭眼，Z 消失 |
//! | 210~240 | 睁眼醒来 |
//!
//! 「入睡」播 0~165：睁眼到冒 Z 一次演完，观众才看得出它睡着了。
//! 「睡着」在 75~165 之间循环：Z 一直在飘，看起来是持续的睡眠，
//! 而不是每隔十秒醒一次。
//!
//! 区间用**比例**而不是绝对帧号：换一套素材时帧数会变，
//! 写死 165 会在新素材上切到莫名其妙的位置。

/// 帧总数减一（241 帧 → 240），把帧号换成比例用。
const LAST: f32 = 240.0;

/// 帧 165：Z 消失前的最后一帧。入睡演到这里就算睡着了。
const FALL_END: f32 = 165.0 / LAST;

/// 帧 75：Z 稳定出现的第一帧。睡着了从这里开始循环。
const ASLEEP_LO: f32 = 75.0 / LAST;

/// 睡觉的两个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepStage {
    /// 入睡中：从睁眼演到冒出 Z。播完自动转 [`SleepStage::Asleep`]。
    Falling,
    /// 睡着了：在打呼那一段里循环，直到被叫醒。
    Asleep,
}

/// 一段睡眠的进度。
#[derive(Debug, Clone)]
pub struct SleepCycle {
    stage: SleepStage,
}

impl Default for SleepCycle {
    fn default() -> Self {
        Self::new()
    }
}

impl SleepCycle {
    /// 从「刚开始睡着」起算。
    pub fn new() -> Self {
        Self {
            stage: SleepStage::Falling,
        }
    }

    /// 当前阶段。
    pub fn stage(&self) -> SleepStage {
        self.stage
    }

    /// 当前阶段该播的帧区间（比例，闭区间）。
    pub fn span(&self) -> (f32, f32) {
        match self.stage {
            SleepStage::Falling => (0.0, FALL_END),
            SleepStage::Asleep => (ASLEEP_LO, FALL_END),
        }
    }

    /// 当前段播到最后一帧了。返回是否换了段。
    ///
    /// 换段后调用方要按新的 [`Self::span`] 重置播放位置；
    /// 返回 `false` 表示就在本段里循环（睡着了就是这样）。
    pub fn on_segment_end(&mut self) -> bool {
        match self.stage {
            SleepStage::Falling => {
                self.stage = SleepStage::Asleep;
                true
            }
            SleepStage::Asleep => false,
        }
    }
}

/// 把比例区间换算成帧号闭区间。`count` 是总帧数。
///
/// 独立成函数是为了能单测：换算错一帧，循环就会闪一下或卡一下。
pub fn span_to_frames((lo, hi): (f32, f32), count: u32) -> (u32, u32) {
    if count == 0 {
        return (0, 0);
    }
    let last = count - 1;
    let to_idx = |r: f32| ((r.clamp(0.0, 1.0) * last as f32).round() as u32).min(last);
    let a = to_idx(lo);
    let b = to_idx(hi);
    // 退化成单帧时也保证 a <= b，调用方不必再判一次。
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实素材是 241 帧，边界换算必须落在实测出来的那些帧号上。
    #[test]
    fn spans_land_on_the_measured_frames() {
        let count = 241;
        let cycle = SleepCycle::new();
        assert_eq!(span_to_frames(cycle.span(), count), (0, 165));
        let mut cycle = cycle;
        assert!(cycle.on_segment_end());
        assert_eq!(cycle.stage(), SleepStage::Asleep);
        assert_eq!(span_to_frames(cycle.span(), count), (75, 165));
    }

    #[test]
    fn falling_happens_once_then_stays_asleep() {
        let mut c = SleepCycle::new();
        assert_eq!(c.stage(), SleepStage::Falling);
        assert!(c.on_segment_end(), "入睡演完要换到睡着");
        assert_eq!(c.stage(), SleepStage::Asleep);
        for _ in 0..10 {
            assert!(!c.on_segment_end(), "睡着之后就一直在本段循环");
            assert_eq!(c.stage(), SleepStage::Asleep);
        }
    }

    #[test]
    fn span_is_inside_the_clip() {
        for count in [1u32, 2, 24, 241] {
            let mut c = SleepCycle::new();
            loop {
                let (lo, hi) = span_to_frames(c.span(), count);
                assert!(lo <= hi, "区间必须左小右大（{lo}..{hi}）");
                assert!(hi < count, "右端不能越界（{hi} >= {count}）");
                if !c.on_segment_end() {
                    break;
                }
            }
        }
    }

    #[test]
    fn zero_frames_does_not_panic() {
        assert_eq!(span_to_frames((0.0, 1.0), 0), (0, 0));
    }

    /// 睡着段必须比入睡段短：它是要一直循环的，太长会让 Z 飘得没完没了。
    #[test]
    fn asleep_span_is_shorter_than_falling() {
        let falling = span_to_frames(SleepCycle::new().span(), 241);
        let mut c = SleepCycle::new();
        c.on_segment_end();
        let asleep = span_to_frames(c.span(), 241);
        assert!(
            asleep.1 - asleep.0 < falling.1 - falling.0,
            "睡着段 {asleep:?} 应短于入睡段 {falling:?}"
        );
    }
}
