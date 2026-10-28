//! 睡觉的三段编排。
//!
//! `sleep.fishanim` 是一整段 10 秒的闭环：**睁眼 → 闭眼 → 冒出 Z → 睁眼醒来**。
//! 素材里根本没有「一直睡着」的状态，所以把它当普通动画循环播放，看到的
//! 就是「慢慢睡着、打两下呼、又自己醒了」——鱼睡了十秒就醒，像睡不踏实。
//!
//! 这里把它拆成三截，边界是**实测出来的**（`feiyu --dump sleep` 逐帧导图，
//! 再用像素统计量出 Z 气泡在右上角占了多少面积，并逐帧看原图确认）：
//!
//! | 帧段 | 内容 |
//! |---|---|
//! | 0~25 | 睁着眼站着 |
//! | 30~75 | 闭眼、低下头、蜷成一团，**一点 Z 都没有** |
//! | 76~130 | Z 从一个小点长满 |
//! | 130~165 | 满格的 Z 一路往上飘（面积 1187~1519 起伏） |
//! | 166~175 | 气泡变淡、彻底散掉 |
//! | 176~209 | 闭眼蜷着睡，**又没有 Z 了** |
//! | 210~240 | 睁眼起身 |
//!
//! 「入睡」播 0~133，一路演到冒出 Z。
//!
//! 「睡着」在 **76~174** 之间循环：正好是「Z 还没冒」到「Z 已经散干净」这两个
//! 身姿几乎相同的时刻，所以每一轮都是完整的「长出 Z → 飘上去 → 散掉」，
//! 而不是硬把一个气泡拽回起点。
//!
//! 这个区间是穷举出来的（A 取 44~80、B 取 174~209，共 1296 对）：最好的一对就是
//! 76 ↔ 174，而且 **Z 那块的差是 0.00**——两端气泡都完全消失，没有任何东西会跳位置。
//!
//! 但整帧还是差 **8.45**，而素材自身相邻两帧的平均差只有 2.12、全片最大一步是 8.50
//! —— 也就是说这个接缝比整段素材里**几乎任何一次自然运动**都狠（只有一步比它大）。
//! 所以播放器在段尾接段首时**把这 8.45 分 6 帧淡过去**（每步 1.4，落回素材自身的噪声里），
//! 详见 `feiyu-app/pet.rs` 的 `Dissolve`。
//!
//! 之前的两种做法都被否了，记在这里免得再走回去：
//!
//! * 在 Z 挂着的那段里循环（旧版 116~148）：Z 一直在上浮，段尾接段首画面差
//!   实测 9.8，是相邻帧的 3 倍，用户看到的是一条条「一抽一抽」。换成
//!   113 ↔ 164 这种「最优对」也有 5.03，气泡照样跳位。
//! * 来回播（116→148→116）：接缝确实没了，但 Z 变成升上去又降下来，
//!   用户一眼就看出来「正放然后倒放」——素材本来就不是来回动的。
//!
//! 「醒来」从**当前帧**往后演到最后一帧（不回绕）：Z 会沿 130~175 自己飘散掉，
//! 然后才睁眼起身。以前的实现直接跳到固定的 170 帧——那一下的画面差是 11.7，
//! 比循环接缝还大一倍，用户看到的就是「闪一下」。
//!
//! 区间用**比例**而不是绝对帧号：换一套素材时帧数会变，
//! 写死 174 会在新素材上切到莫名其妙的位置。

/// 帧总数减一（241 帧 → 240），把帧号换成比例用。
const LAST: f32 = 240.0;

/// 帧 133：Z 气泡已经长满、正在往上飘。入睡演到这里就算睡熟了。
const FALL_END: f32 = 133.0 / LAST;

/// 帧 76：Z 刚要冒出来。睡着段的**下界**——再往前就没有气泡了。
const ASLEEP_LO: f32 = 76.0 / LAST;

/// 帧 174：气泡已经彻底散掉、身体又回到蜷着的姿势。睡着段的**上界**。
///
/// 76 和 174 这两个帧身体姿态最接近（穷举 1296 对里最好的），
/// 而且两端都没有 Z：回绕时没有任何东西会跳位置。见模块文档。
const ASLEEP_HI: f32 = 174.0 / LAST;

/// 帧 170：Z 已完全消失、还闭着眼。
///
/// 只在「不知道当前帧」时用作醒来段的默认起点（见 [`SleepCycle::new`]）；
/// 正常点醒会从当前帧开始，不跳到这里（那里会看到气泡瞬间消失）。
const WAKE_LO: f32 = 170.0 / LAST;

/// 睡觉的三个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepStage {
    /// 入睡中：从睁眼演到冒出完整的 Z。播完自动转 [`SleepStage::Asleep`]。
    Falling,
    /// 睡着了：在「Z 还没冒」到「Z 已散尽」这一段里循环，直到被叫醒。
    /// 每一圈都是完整的「长出 Z → 飘上去 → 散掉」，不是把气泡拽回起点。
    Asleep,
    /// 正在醒：播 Z 消散后的睁眼起身段。播完由调用方切回待机。
    Waking,
}

/// 一段睡眠的进度。
#[derive(Debug, Clone)]
pub struct SleepCycle {
    stage: SleepStage,
    /// 醒来段从哪一帧开始（比例）。由调用方传入当前帧，
    /// 这样「点醒」永远不会跳帧——Z 是慢慢飘散的，不是被抹掉的。
    wake_from: f32,
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
            wake_from: WAKE_LO,
        }
    }

    /// 当前阶段。
    pub fn stage(&self) -> SleepStage {
        self.stage
    }

    /// 被叫醒了：改播醒来的那段，**从 `from`（比例）开始**。
    ///
    /// 调用方传当前正在显示的那一帧：醒来段不回绕，从当前帧往后演，
    /// 中途没有一帧是跳过去的。传 0 就等于「从入睡段开头演」——
    /// 所以不要把不相关的帧号传进来。
    ///
    /// 只能在 [`SleepStage::Asleep`] 上调——入睡途中被点醒的话，
    /// 从「刚闭眼」直接跳到「睁眼起身」反而比继续睡下去更怪。
    pub fn start_waking(&mut self, from: f32) {
        self.stage = SleepStage::Waking;
        if from.is_finite() {
            self.wake_from = from.clamp(0.0, 1.0);
        }
    }

    /// 当前阶段该播的帧区间（比例，闭区间）。
    pub fn span(&self) -> (f32, f32) {
        match self.stage {
            SleepStage::Falling => (0.0, FALL_END),
            SleepStage::Asleep => (ASLEEP_LO, ASLEEP_HI),
            SleepStage::Waking => (self.wake_from, 1.0),
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
        assert_eq!(span_to_frames(cycle.span(), count), (76, 174));

        cycle.start_waking(WAKE_LO);
        assert_eq!(cycle.stage(), SleepStage::Waking);
        assert_eq!(span_to_frames(cycle.span(), count), (170, 240));
    }

    /// 点醒时从当前帧接着演：传进来的比例必须原样换回那一帧。
    #[test]
    fn waking_starts_where_the_caller_asks() {
        let count = 241;
        let mut c = SleepCycle::new();
        c.on_segment_end();
        for frame in [76u32, 120, 174] {
            let mut c = c.clone();
            c.start_waking(frame as f32 / (count - 1) as f32);
            assert_eq!(
                span_to_frames(c.span(), count),
                (frame, 240),
                "从第 {frame} 帧开始醒，换算后必须还是第 {frame} 帧"
            );
        }
        // 非法值不许把播放位置带到奇怪的地方。
        let mut c = c;
        c.start_waking(f32::NAN);
        assert_eq!(span_to_frames(c.span(), count), (170, 240));
    }

    /// 睡着段的起止必须是「Z 还没冒」和「Z 已散尽」——
    /// 中间夹着完整的生长、上浮、消散，两端又都没有气泡会跳位置。
    #[test]
    fn asleep_span_holds_one_whole_bubble() {
        let mut c = SleepCycle::new();
        c.on_segment_end();
        let (lo, hi) = span_to_frames(c.span(), 241);
        assert!(lo <= 76, "睡着段起点 {lo} 要落在 Z 冒出来（实测 76）之前");
        assert!(hi >= 174, "睡着段终点 {hi} 要落在 Z 散尽（实测 174）之后");
        assert!(hi - lo >= 90, "这一段要装得下整个气球周期（实测 98 帧）");
    }

    /// 醒来段必须走到最后一帧——否则鱼永远睁不开眼。
    #[test]
    fn wake_span_reaches_the_last_frame() {
        let mut c = SleepCycle::new();
        c.on_segment_end();
        c.start_waking(WAKE_LO);
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

    /// 睡着段要比入睡段短：太长了 Z 一轮得等太久，看不出在打呼。
    ///
    /// 只需要长到装得下一整个「长出来 → 飘上去 → 散掉」就行，实测 98 帧 ≈ 4.1 秒。
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
