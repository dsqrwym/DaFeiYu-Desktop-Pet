//! 松手之后的滑行与下落：纯运动学，不碰窗口。
//!
//! 做成纯逻辑的理由和 [`crate::brain`] 一样——手感靠调参数，而参数对不对
//! 必须能在测试里定住：「甩出去必须停得下来」「一定会落到地面」「不会越弹越高」
//! 这类结论不能靠肉眼看。调用方只负责每帧喂 `dt` 与锚点，并把结果搬进窗口位置。
//!
//! 坐标系与 [`crate::anim::Manifest`] 一致：锚点是**画布坐标**下的
//! `(角色中心 x, 脚底 y)`，单位是屏幕物理像素，速度单位是像素/秒。

/// 滑行阻力：速度按 `e^(-FRICTION·dt)` 衰减，约 1 秒后只剩 4%。
const FRICTION: f32 = 3.2;

/// 重力加速度（像素/秒²）。2400 大约是从 300 像素高落到地面 0.5 秒。
const GRAVITY: f32 = 2400.0;

/// 撞墙/落地反弹后保留的速度比例。
const BOUNCE: f32 = 0.35;

/// 反弹阈值：低于它就直接停住，免得在边缘原地抖动。
const BOUNCE_MIN: f32 = 240.0;

/// 速度上限（像素/秒），防止一次甩动飞到屏幕外。
const MAX_SPEED: f32 = 2600.0;

/// 判定「已经停稳」的速度阈值（像素/秒）。
const REST_SPEED: f32 = 12.0;

/// 低于这个速度的甩动干脆忽略：轻轻一放不该滑出去。
pub const MIN_FLING: f32 = 130.0;

/// 单步积分的时间上限：卡顿一次不该让角色瞬移出屏幕。
const MAX_STEP: f32 = 0.032;

/// 锚点在屏幕上的允许范围（闭区间）。
///
/// `x/y` 的单位都是锚点坐标；`y` 的**上界就是地面**（脚底落到此处即着地）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// 锚点 x 的允许范围。
    pub x: (f32, f32),
    /// 脚底 y 的允许范围；上界即地面。
    pub y: (f32, f32),
}

impl Limits {
    /// 夹回范围内。区间退化（窗口比工作区还大）时取中点，绝不 panic。
    pub fn clamp(&self, anchor: (f32, f32)) -> (f32, f32) {
        (clamp_pair(anchor.0, self.x), clamp_pair(anchor.1, self.y))
    }
}

fn clamp_pair(v: f32, range: (f32, f32)) -> f32 {
    let (lo, hi) = range;
    if lo > hi {
        return (lo + hi) / 2.0;
    }
    v.clamp(lo, hi)
}

/// 从拖拽采样估算松手速度（像素/秒）。
///
/// `samples` 是 `(相对秒数, 窗口左上角 x, y)`，按时间升序；**调用方必须只传
/// 最近一小段**（本项目的做法是只留 120ms）：否则「拖到位后停住两秒再松手」
/// 会被算成一次快甩。
///
/// 用首尾两点算平均速度，而不是最后两点——鼠标最后几毫秒的抖动
/// 乘上很短的时间差会被放大成假速度。
pub fn fling_velocity(samples: &[(f32, i32, i32)]) -> (f32, f32) {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return (0.0, 0.0);
    };
    let span = last.0 - first.0;
    // 采样太少或时间跨度太短：除以它只会得到噪声。
    if samples.len() < 2 || !(span > 0.01) {
        return (0.0, 0.0);
    }
    let vx = (last.1 - first.1) as f32 / span;
    let vy = (last.2 - first.2) as f32 / span;
    if (vx * vx + vy * vy).sqrt() < MIN_FLING {
        return (0.0, 0.0);
    }
    (
        vx.clamp(-MAX_SPEED, MAX_SPEED),
        vy.clamp(-MAX_SPEED, MAX_SPEED),
    )
}

/// 松手后的一段自由运动。
#[derive(Debug, Clone, Copy)]
pub struct Motion {
    vx: f32,
    vy: f32,
    /// 上一帧是否踩在地面上。落地后垂直方向不再受重力，
    /// 否则贴地滑行时会被反复「按下 → 弹回」，看起来像抽搐。
    grounded: bool,
}

impl Motion {
    /// 以松手速度起飞。速度会被夹到上限。
    pub fn fling(vx: f32, vy: f32) -> Self {
        let (vx, vy) = if vx.is_finite() && vy.is_finite() {
            (
                vx.clamp(-MAX_SPEED, MAX_SPEED),
                vy.clamp(-MAX_SPEED, MAX_SPEED),
            )
        } else {
            (0.0, 0.0)
        };
        Self {
            vx,
            vy,
            grounded: false,
        }
    }

    /// 当前速度，测试与日志用。
    pub fn velocity(&self) -> (f32, f32) {
        (self.vx, self.vy)
    }

    /// 推进一帧，返回 `(新锚点, 是否已停稳)`。
    pub fn step(&mut self, anchor: (f32, f32), limits: Limits, dt: f32) -> ((f32, f32), bool) {
        let dt = if dt.is_finite() && dt > 0.0 {
            dt.min(MAX_STEP)
        } else {
            0.0
        };
        if dt == 0.0 {
            return (anchor, self.at_rest());
        }

        let damp = (-FRICTION * dt).exp();
        let mut vx = self.vx * damp;
        let mut vy = if self.grounded {
            self.vy * damp
        } else {
            (self.vy + GRAVITY * dt) * damp
        };

        let mut x = anchor.0 + vx * dt;
        let mut y = anchor.1 + vy * dt;

        // 左右墙：反弹。用 abs 定号，避免「已经朝里飞了还被反向」。
        if x < limits.x.0 {
            x = limits.x.0;
            vx = vx.abs() * BOUNCE;
        } else if x > limits.x.1 {
            x = limits.x.1;
            vx = -vx.abs() * BOUNCE;
        }

        // 天花板：贴住即停，不反弹——否则会在顶上弹个没完，很怪。
        if y < limits.y.0 {
            y = limits.y.0;
            vy = 0.0;
        }

        self.grounded = false;
        if y >= limits.y.1 {
            y = limits.y.1;
            if vy > BOUNCE_MIN {
                vy = -vy * BOUNCE;
            } else {
                vy = 0.0;
                self.grounded = true;
            }
        }

        self.vx = vx;
        self.vy = vy;

        let rest = self.at_rest();
        if rest {
            self.vx = 0.0;
            self.vy = 0.0;
        }
        ((x, y), rest)
    }

    /// 是否已停稳（落地且余速不足）。
    pub fn at_rest(&self) -> bool {
        self.grounded && self.vx.abs() < REST_SPEED && self.vy.abs() < REST_SPEED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUND: f32 = 1000.0;
    const LIMITS: Limits = Limits {
        x: (0.0, 1000.0),
        y: (0.0, GROUND),
    };

    /// 一直推进到停稳，返回最终锚点与用掉的帧数。
    fn settle(mut m: Motion, start: (f32, f32), max_frames: usize) -> ((f32, f32), bool, usize) {
        let mut anchor = start;
        for i in 0..max_frames {
            let (next, rest) = m.step(anchor, LIMITS, 1.0 / 60.0);
            anchor = next;
            if rest {
                return (anchor, true, i + 1);
            }
        }
        (anchor, false, max_frames)
    }

    #[test]
    fn no_samples_means_no_fling() {
        assert_eq!(fling_velocity(&[]), (0.0, 0.0));
        assert_eq!(
            fling_velocity(&[(0.0, 10, 10)]),
            (0.0, 0.0),
            "单点没有速度概念"
        );
    }

    #[test]
    fn slow_drag_release_does_not_fling() {
        // 100ms 走 5 像素 = 50 像素/秒，低于阈值。
        let samples = [(0.00, 0, 0), (0.05, 2, 0), (0.10, 5, 0)];
        assert_eq!(fling_velocity(&samples), (0.0, 0.0));
    }

    #[test]
    fn fast_flick_produces_velocity() {
        // 60ms 走 120 像素 = 2000 像素/秒。
        let samples = [(0.00, 0, 0), (0.03, 60, 0), (0.06, 120, 0)];
        let (vx, vy) = fling_velocity(&samples);
        assert!((vx - 2000.0).abs() < 1.0, "vx={vx}");
        assert_eq!(vy, 0.0);
    }

    #[test]
    fn jitter_in_last_sample_does_not_blow_up_speed() {
        // 最后一点抖动回退 3 像素；按「首尾」算仍是正常速度，
        // 按「最后两点」算会得到 -3px/1ms = -3000，明显是假的。
        let samples = [(0.00, 0, 0), (0.05, 100, 0), (0.06, 97, 0)];
        let (vx, _) = fling_velocity(&samples);
        assert!(vx > 1500.0 && vx < 1700.0, "vx={vx}");
    }

    #[test]
    fn absurd_speed_is_clamped() {
        let samples = [(0.00, 0, 0), (0.02, 9999, 0)];
        let (vx, _) = fling_velocity(&samples);
        assert!((vx - MAX_SPEED).abs() < 1.0, "vx={vx}");
    }

    #[test]
    fn falls_to_the_ground_and_stops() {
        let m = Motion::fling(0.0, 0.0);
        let ((x, y), rest, frames) = settle(m, (500.0, 400.0), 600);
        assert!(rest, "600 帧内必须停稳");
        assert_eq!(y, GROUND, "必须精确贴地，不能悬在半空");
        assert_eq!(x, 500.0);
        assert!(frames < 600);
    }

    #[test]
    fn thrown_upwards_comes_back_down() {
        let m = Motion::fling(0.0, -1200.0);
        let ((_, y), rest, _) = settle(m, (500.0, GROUND), 600);
        assert!(rest);
        assert_eq!(y, GROUND, "往上甩也得落回来");
    }

    #[test]
    fn bounce_never_gains_energy() {
        // 从高处扔下：每一次反弹都应比上一次低，最终停在地面。
        let mut m = Motion::fling(0.0, 0.0);
        let mut anchor = (500.0, 0.0);
        let mut peak = 0.0f32;
        for _ in 0..600 {
            let (next, rest) = m.step(anchor, LIMITS, 1.0 / 60.0);
            anchor = next;
            peak = peak.max(-anchor.1); // 离地高度（y 越小越高）
            if rest {
                break;
            }
        }
        assert_eq!(anchor.1, GROUND);
        // 起点在最顶（y=0），反弹高度不可能超过它。
        assert!(peak <= 0.0, "反弹越过了起始高度");
    }

    #[test]
    fn horizontal_fling_stops_on_the_ground() {
        let m = Motion::fling(2000.0, 0.0);
        let ((x, y), rest, frames) = settle(m, (100.0, GROUND), 900);
        assert!(rest, "横向甩动也必须停得下来");
        assert_eq!(y, GROUND);
        assert!(x > 100.0, "应当往右滑了一段");
        assert!(frames < 300, "1.6 秒内应该停稳，实际 {frames} 帧");
    }

    #[test]
    fn wall_bounce_reverses_direction() {
        let mut m = Motion::fling(2000.0, 0.0);
        // 贴着右墙往外甩。
        let (_, _) = m.step((LIMITS.x.1, GROUND), LIMITS, 1.0 / 60.0);
        assert!(m.velocity().0 < 0.0, "撞右墙后应朝左");
        let mut m = Motion::fling(-2000.0, 0.0);
        let _ = m.step((LIMITS.x.0, GROUND), LIMITS, 1.0 / 60.0);
        assert!(m.velocity().0 > 0.0, "撞左墙后应朝右");
    }

    #[test]
    fn anchor_never_leaves_limits() {
        let mut m = Motion::fling(MAX_SPEED, -MAX_SPEED);
        let mut anchor = (500.0, GROUND);
        for _ in 0..600 {
            let (next, rest) = m.step(anchor, LIMITS, 1.0 / 60.0);
            anchor = next;
            assert!(
                anchor.0 >= LIMITS.x.0 - 0.01 && anchor.0 <= LIMITS.x.1 + 0.01,
                "x={}",
                anchor.0
            );
            assert!(
                anchor.1 >= LIMITS.y.0 - 0.01 && anchor.1 <= LIMITS.y.1 + 0.01,
                "y={}",
                anchor.1
            );
            if rest {
                break;
            }
        }
    }

    #[test]
    fn bad_dt_is_ignored() {
        let mut m = Motion::fling(500.0, 0.0);
        let start = (500.0, GROUND);
        for dt in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert_eq!(m.step(start, LIMITS, dt), (start, false), "dt={dt} 不该动");
        }
    }

    #[test]
    fn degenerate_limits_do_not_panic() {
        // 窗口比工作区还大：区间反过来。取中点，不许 panic、不许 NaN。
        let limits = Limits {
            x: (900.0, 100.0),
            y: (900.0, 100.0),
        };
        assert_eq!(limits.clamp((0.0, 5000.0)), (500.0, 500.0));
        let mut m = Motion::fling(300.0, 300.0);
        let ((x, y), _, _) = {
            let (a, r) = m.step((0.0, 0.0), limits, 1.0 / 60.0);
            (a, r, 0)
        };
        assert!(x.is_finite() && y.is_finite());
    }

    #[test]
    fn resting_on_the_ground_needs_no_frames() {
        let mut m = Motion::fling(0.0, 0.0);
        // 已经贴地、零速度：第一帧就该判定停稳，不该再抖。
        let ((x, y), rest) = m.step((500.0, GROUND), LIMITS, 1.0 / 60.0);
        assert!(rest, "贴地零速度应当立刻停稳");
        assert_eq!((x, y), (500.0, GROUND));
    }
}
