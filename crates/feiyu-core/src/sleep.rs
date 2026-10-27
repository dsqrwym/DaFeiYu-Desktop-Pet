//! 睡觉的三段编排。
//!
//! `sleep.fishanim` 是一整段 10 秒的闭环：**睁眼 → 闭眼 → 冒出 Z → 睁眼醒来**。
//! 素材里根本没有「一直睡着」的状态，所以把它当普通动画循环播放，看到的
//! 就是「慢慢睡着、打两下呼、又自己醒了」——鱼睡了十秒就醒，像睡不踏实。
//!
//! 这里把它拆成三截，边界是**实测出来的**（`feiyu --dump sleep` 逐帧导图，
//! 再用像素统计量出 Z 气泡在右上角占了多少面积）：
//!
//! | 帧段 | 内容 | Z 气泡像素量 |
//! |---|---|---|
//! | 0~79 | 睁眼 → 闭眼，**一点 Z 都没有** | 0 |
//! | 80~133 | Z 从小长到最大 | 1 → 106 |
//! | **116~148** | **Z 完整挂着** | 93 ↔ 92 |
//! | 149~173 | 气泡上浮、变淡、消失 | 105 → 0 |
//! | 174~209 | 闭眼，无 Z | 0 |
//! | 210~240 | 睁眼醒来 | 0 |
//!
//! 「入睡」播 0~133：睁眼到冒出完整的 Z 一次演完，观众才看得出它睡着了。
//! 「睡着」在 **116~148** 之间循环：这两端的像素量几乎相等（93 vs 92），
//! 说明气泡大小最接近，回绕时看不出「跳了一下」；而两端都**已经有完整的 Z**，
//! 不会出现旧版本那种「循环回去 Z 还没冒出来」的空白。
//!
//! 「醒来」播 170~240：Z 已经消散、鱼慢慢睁眼坐起来。
//! 这一段播完由调用方切回待机——用户点一下不该看到它「啪」地站好。
//!
//! 区间用**比例**而不是绝对帧号：换一套素材时帧数会变，
//! 写死 133 会在新素材上切到莫名其妙的位置。

/// 帧总数减一（241 帧 → 240），把帧号换成比例用。
const LAST: f32 = 240.0;

/// 帧 133：Z 气泡长到最大的那一帧。入睡演到这里就算睡熟了。
const FALL_END: f32 = 133.0 / LAST;

/// 帧 116：Z 已经完整、气泡处于中段位置。睡着从这里开始循环。
const ASLEEP_LO: f32 = 116.0 / LAST;

/// 帧 148：气泡还没开始飘散。它的像素量与 116 几乎相同，
/// 所以 `148 → 116` 这一下回绕在视觉上只是气泡略微下沉，不会“重新冒出来一个”。
const ASLEEP_HI: f32 = 148.0 / LAST;

/// 帧 170：Z 已完全消失、还闭着眼。醒来的演出从这里开始。
const WAKE_LO: f32 = 170.0 / LAST;

/// 睡觉的三个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepStage {
    /// 入睡中：从睁眼演到冒出完整的 Z。播完自动转 [`SleepStage::Asleep`]。
    Falling,
    /// 睡着了：在 Z 一直挂着的那一段里循环，直到被叫醒。
    Asleep,
    /// 正在醒：播 Z 消散后的睁眼起身段。播完由调用方切回待机。
    Waking,
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

    /// 被叫醒了：改播醒来的那段。
    ///
    /// 只能在 [`SleepStage::Asleep`] 上调——入睡途中被点醒的话，
    /// 从「刚闭眼」直接跳到「睁眼起身」反而比继续睡下去更怪。
    pub fn start_waking(&mut self) {
        self.stage = SleepStage::Waking;
    }

    /// 当前阶段该播的帧区间（比例，闭区间）。
    pub fn span(&self) -> (f32, f32) {
        match self.stage {
            SleepStage::Falling => (0.0, FALL_END),
            SleepStage::Asleep => (ASLEEP_LO, ASLEEP_HI),
            SleepStage::Waking => (WAKE_LO, 1.0),
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
            // 睡着段要一直转，醒来段播完由调用方接手。
            SleepStage::Asleep | SleepStage::Waking => false,
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
        assert_eq!(span_to_frames(cycle.span(), count), (0, 133));
        let mut cycle = cycle;
        assert!(cycle.on_segment_end());
        assert_eq!(cycle.stage(), SleepStage::Asleep);
        assert_eq!(span_to_frames(cycle.span(), count), (116, 148));

        cycle.start_waking();
        assert_eq!(cycle.stage(), SleepStage::Waking);
        assert_eq!(span_to_frames(cycle.span(), count), (170, 240));
    }

    /// 睡着段的起点必须在 Z 已经冒出来之后。
    ///
    /// 像素统计显示 Z 从第 80 帧才开始出现，之前是一片空白——
    /// 如果区间起点落在那里，循环回去就会看到“Z 还没冒出来”的空帧。
    #[test]
    fn asleep_span_starts_after_z_appeared() {
        let mut c = SleepCycle::new();
        c.on_segment_end();
        let (lo, hi) = span_to_frames(c.span(), 241);
        assert!(lo >= 80, "睡着段起点 {lo} 不能早于 Z 出现的第 80 帧");
        assert!(hi >= 130, "睡着段终点 {hi} 也要在 Z 完整区间内");
    }

    /// 醒来段必须走到最后一帧——否则鱼永远睁不开眼。
    #[test]
    fn wake_span_reaches_the_last_frame() {
        let mut c = SleepCycle::new();
        c.on_segment_end();
        c.start_waking();
        let (lo, hi) = span_to_frames(c.span(), 241);
        assert!(lo >= 169, "醒来段要从 Z 已经消散的帧开始（实测 170）");
        assert_eq!(hi, 240, "醒来段要一路演到最后一帧");
        assert!(!c.on_segment_end(), "醒来段不回绕，播完由调用方接管");
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
