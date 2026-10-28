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
//! | 76~127 | Z 从一个小点长到最大、飘到画布上沿 |
//! | 128~175 | 气泡上浮、变淡、彻底散掉 |
//! | 176~209 | 眼睛闭上但**身体开始往上起** |
//! | 210~240 | 睁眼起身站好 |
//!
//! 「入睡」播 0~127，一路演到 Z 长满。
//!
//! 「睡着」在 **76~127** 之间循环：每一圈都是完整的「Z 冒出来 → 长满」，
//! 然后淡掉重新开始。
//!
//! 为什么是 127 而不是「Z 散尽」的 174：**身体一直在慢慢起身，不会回到原点**。
//! 把每帧的身体（裁掉 Z 所在的上方 100 像素）和 76 帧比，位移曲线是：
//!
//! | 帧 | 76 | 110 | **126** | 160 | 174 | 180 | 210 |
//! |---|---|---|---|---|---|---|---|
//! | 身体位移 | 0 | 10.8 | **4.32** | 9.9 | 10.8 | 14.4 | 24.9 |
//!
//! （素材自身相邻一帧的身体位移只有 2.2。）126~127 是身体回到 76 姿势的那个点，
//! 也是「起身」开始前的最后一刻。上一版选 174 正好卡在起身的坡道上，
//! 于是每一圈都是「慢慢起来一点 → 淡回去」——用户说的「腿部开始站立但刚起来一点就回到睡觉」。
//!
//! 剩下的身体位移 4.32 由淡入摊薄到 0.7/帧，低于自然运动的 2.2，看不出在滑。
//! Z 那一条带两端差 9.38（一端没有气泡、一端长满），淡过去就是「气泡散掉、
//! 新的冒出来」——本来就该这样，不是跳位。
//!
//! 之前的几种选法都被数据否了，记在这里免得再走回去：
//!
//! * 在 Z 挂着的那段里循环（116~148）：段尾接段首整帧差 9.8，是相邻帧的 4.6 倍，
//!   用户看到的是「一抽一抽」。穷举 116~176 所有循环点，最好的 113 ↔ 164 也有 5.03。
//! * 来回播（116→148→116）：接缝确实没了，但 Z 变成升上去又降下来，
//!   用户一眼就看出来「正放然后倒放」——素材本来就不是来回动的。
//! * 76~174（「两端都没有 Z」那一版）：Z 确实两端都消失，但身体在 174 已经开始起身，
//!   而且腿部那条带在接缝处差 **12.87**（自然值 2），淡入就是一次看得见的「坐回去」。
//!
//! 「醒来」从**当前帧**往后演到最后一帧（不回绕）：Z 会沿 128~175 自己飘散掉，
//! 然后才睁眼起身。以前的实现直接跳到固定的 170 帧——那一下的画面差是 11.7，
//! 比循环接缝还大一倍，用户看到的就是「闪一下」。
//!
//! 区间用**比例**而不是绝对帧号：换一套素材时帧数会变，
//! 写死 174 会在新素材上切到莫名其妙的位置。

/// 帧总数减一（241 帧 → 240），把帧号换成比例用。
const LAST: f32 = 240.0;

/// 帧 127：Z 长满、而且身体还没开始往上起。入睡演到这里就算睡熟了。
///
/// 和 [`ASLEEP_HI`] 取同一个值：入睡段停在 127，换段时帧号正好落在打呼段的
/// 上界，一步都不用回退。
const FALL_END: f32 = 127.0 / LAST;

/// 帧 76：Z 刚要冒出来。睡着段的**下界**——再往前就没有气泡了。
const ASLEEP_LO: f32 = 76.0 / LAST;

/// 帧 127：身体回到 76 帧那个姿势的那一刻（逐帧量过：与 76 的身体位移只有 4.32）。
///
/// 再往后身体就一路往上起（174 已经 10.8、180 是 14.4），循环到那里就会
/// 每圈都「起身一点再坐回去」。见模块文档里的位移曲线。
const ASLEEP_HI: f32 = 127.0 / LAST;

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
        assert_eq!(span_to_frames(cycle.span(), count), (0, 127));
        let mut cycle = cycle;
        assert!(cycle.on_segment_end());
        assert_eq!(cycle.stage(), SleepStage::Asleep);
        assert_eq!(span_to_frames(cycle.span(), count), (76, 127));

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
        for frame in [76u32, 100, 127] {
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

    /// 睡着段必须「从 Z 还没冒」到「身体开始起身之前」。
    ///
    /// 关键是上界：身体从 76 帧起就一直在慢慢往上起，126~127 是它回到
    /// 起点姿势的那一刻，再往后（174 已经位移 10.8）循环就会看到「起身一点又坐回去」。
    #[test]
    fn asleep_span_stops_before_the_body_gets_up() {
        let mut c = SleepCycle::new();
        c.on_segment_end();
        let (lo, hi) = span_to_frames(c.span(), 241);
        assert!(lo <= 76, "起点 {lo} 要在 Z 冒出来（实测 76）之前");
        assert!(
            hi <= 130,
            "终点 {hi} 必须在身体起身（实测 176 开始）之前，而且越贴近 127 越好"
        );
        assert!(
            hi - lo >= 40,
            "这一段要装得下一整个 Z 的生长周期（实测 51 帧）"
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
    /// 长到装得下一整个「冒出来 → 长满」就行，实测 51 帧 ≈ 2.1 秒。
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
