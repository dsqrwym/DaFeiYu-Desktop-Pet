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
//! | 30~75 | 闭眼、低下头、蜷成一团，**一点气泡都没有** |
//! | 78~104 | 气泡从 1 个长到 2~3 个（逐帧连通域量的） |
//! | 104~156 | **2~3 个气泡挂在上方继续飘** |
//! | 157~173 | 气泡一个个脱离、变淡、散尽 |
//! | 174~240 | **一个气泡都没有**（逐帧数过），身体 176 起开始起身 |
//!
//! 「入睡」播 0~156 一遍：站着 → 闭眼蜷下 → 气泡 1→2→3 → 飘到上方。
//!
//! 「睡着」在 **104~156** 之间循环，也就是用户提的那个形状：
//! 「先演一遍一到三个气泡，随后只在二到三个那一截里循环」。
//! 两端**都有气泡**，所以不会看到「气泡整体消失 → 卡一下 → 从零重新长出来」；
//! 淡过去就是「旧的飘远散掉、新的从嘴边冒出来」。
//!
//! 为什么不是 76~127（上一版）：它的下界没有气泡，回绕时气泡必须整体消失，
//! 而且淡入那 6 拍帧号停住，两件事叠在一起就是用户看到的「消失 + 卡一下」。
//!
//! 为什么偏偏是 104 和 156：穷举了 2544 对候选（`target/sleep_menu.ps1`），
//! 按三条带分开量接缝（自然一帧：身体 0~2.2、很多相邻帧完全相同）：
//!
//! | 端点 | 长度 | 身体 | 腿 | 气泡带 |
//! |---|---|---|---|---|
//! | 80 ↔ 131 | 51 | 4.03 | 4.35 | 9.35 |
//! | **104 ↔ 156** | 52 | **4.10** | **4.45** | **6.81** |
//! | 76 ↔ 174 | 98 | 10.82 | 12.55 | 5.08 |
//! | 83 ↔ 143（≥ 60 帧里最好） | 60 | 7.13 | 7.36 | 8.37 |
//!
//! 104 ↔ 156 是「身体几乎不动」那一族里气泡带最安静的一个。按播放器自己的
//! 度量（预乘 BGRA）这个接缝是 **5.11**，摊薄到 6 帧就是 0.85/步，
//! 低于睡眠段自然一帧的 ~1.5。
//!
//! 硬约束也记一下，免得以后又去想「为什么不循环整段」：**身体静止与气泡对齐
//! 不可兼得**——`气泡带 < 5 且 身体 < 7` 的配对是空集，因为气泡一直在动。
//! 而身体能回到同一姿势的点只有 104↔156 / 76↔127 这几个，再往后（174+）就是真起身。
//!
//! 之前被数据否掉的选法：
//!
//! * 116~148（只量 Z 像素**数量**）：数量相等不代表位置相同，整帧差 9.8，「一抽一抽」。
//! * 来回播 116→148→116：零接缝，但 Z 变成升上去又降下来，「诡异的正放然后倒放」。
//! * 76~174（只看了「两端都没 Z」）：身体在 174 已经开始起身，腿部带差 12.55，
//!   淡入就是一次看得见的「坐回去」。
//!
//! 「醒来」从**当前帧**往后演到最后一帧（不回绕）：气泡会沿 157~173 自己飘散掉，
//! 然后才睁眼起身。以前的实现直接跳到固定的 170 帧——那一下的画面差是 11.7，
//! 比循环接缝还大一倍，用户看到的就是「闪一下」。
//!
//! 区间用**比例**而不是绝对帧号：换一套素材时帧数会变，
//! 写死 174 会在新素材上切到莫名其妙的位置。

/// 帧总数减一（241 帧 → 240），把帧号换成比例用。
const LAST: f32 = 240.0;

/// 帧 156：气泡已经飘到上方、但**还没开始散**，而且身体与 104 帧对得上。
///
/// 入睡段与打呼段共用这个上界：入睡演到 156 就算睡熟，换段时帧号一步不回退。
/// 再往后（157~173）气泡会一个个散尽，循环到那里就会看到「整体消失」。
const FALL_END: f32 = 156.0 / LAST;

/// 帧 104：气泡已经长到 2~3 个。睡着段的**下界**——两端都得有气泡。
///
/// 穷举过：在「身体不动」的那一族端点里，这一对的气泡带最安静（见模块文档）。
const ASLEEP_LO: f32 = 104.0 / LAST;

/// 帧 156：睡着段的上界，与 [`FALL_END`] 同值。
const ASLEEP_HI: f32 = 156.0 / LAST;

/// 帧 170：Z 已完全消失、还闭着眼。
///
/// 只在「不知道当前帧」时用作醒来段的默认起点（见 [`SleepCycle::new`]）；
/// 正常点醒会从当前帧开始，不跳到这里（那里会看到气泡瞬间消失）。
const WAKE_LO: f32 = 170.0 / LAST;

/// 睡觉的三个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepStage {
    /// 入睡中：从睁眼演到气泡长满并飘到上方。播完自动转 [`SleepStage::Asleep`]。
    Falling,
    /// 睡着了：在「2~3 个气泡飘着」那一截里循环，直到被叫醒。
    /// 两端都有气泡，不会看到气泡整体消失后再从零长出来。
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
        assert_eq!(span_to_frames(cycle.span(), count), (0, 156));
        let mut cycle = cycle;
        assert!(cycle.on_segment_end());
        assert_eq!(cycle.stage(), SleepStage::Asleep);
        assert_eq!(span_to_frames(cycle.span(), count), (104, 156));

        cycle.start_waking(WAKE_LO);
        assert_eq!(cycle.stage(), SleepStage::Waking);
        assert_eq!(span_to_frames(cycle.span(), count), (170, 240));
    }

    /// 入睡段必须停在打呼段的上界：换段时帧号一步都不用回退。
    #[test]
    fn falling_ends_exactly_where_the_asleep_span_stops() {
        let falling = span_to_frames(SleepCycle::new().span(), 241);
        let mut c = SleepCycle::new();
        c.on_segment_end();
        let asleep = span_to_frames(c.span(), 241);
        assert_eq!(
            falling.1, asleep.1,
            "入睡停在 {}，打呼段上界是 {}——不相等的话换段会往回跳",
            falling.1, asleep.1
        );
    }

    /// 点醒时从当前帧接着演：传进来的比例必须原样换回那一帧。
    #[test]
    fn waking_starts_where_the_caller_asks() {
        let count = 241;
        let mut c = SleepCycle::new();
        c.on_segment_end();
        for frame in [104u32, 130, 156] {
            let mut c = c.clone();
            c.start_waking(frame as f32 / (count - 1) as f32);
            assert_eq!(
                span_to_frames(c.span(), count),
                (frame, 240),
                "从第 {frame} 帧开始醒，换算后必须还是第 {frame} 帧"
            );
        }
        // 非法值不许把播放位置带到奇怪的地方。
        c.start_waking(f32::NAN);
        assert_eq!(span_to_frames(c.span(), count), (170, 240));
    }

    /// 睡着段必须「两端都有气泡」，而且停在身体起身之前。
    ///
    /// 逐帧数过气泡：78~173 之间存在，174 之后一个都没有；身体 176 起开始起身。
    /// 下界取 104（已经有 2~3 个）而不是 76（还没气泡），否则回绕时气泡必须整体
    /// 消失，用户看到的正是「全部消失 → 卡一下 → 从零重新长出来」。
    #[test]
    fn asleep_span_keeps_bubbles_at_both_ends() {
        let mut c = SleepCycle::new();
        c.on_segment_end();
        let (lo, hi) = span_to_frames(c.span(), 241);
        assert!(
            lo >= 100,
            "起点 {lo} 处必须已经有气泡（实测 104 已经有 2~3 个）"
        );
        assert!(
            hi <= 173,
            "终点 {hi} 必须在气泡散尽、身体起身（实测 176）之前"
        );
        assert!(
            hi - lo >= 40,
            "这一段要装得下 2~3 个气泡飘着的那截（实测 52 帧）"
        );
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
    /// 长到装得下「2~3 个气泡飘着 → 淡掉 → 新的冒出来」就行，实测 52 帧 ≈ 2.2 秒。
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
