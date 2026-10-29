//! Cursor following: turn the head while idle.

use std::time::Instant;

use super::Pet;

/// 两次「转头看鼠标」之间的最小间隔（秒）。
///
/// 没冷却的话鼠标在"一个肥鱼宽"的边界上抖一下，鱼就会一直扭头。
const LOOK_COOLDOWN: f32 = 4.0;

/// `idle` 素材里「看左 → 回正」的帧区间（用户目视核对过的帧号）。
///
/// 66 开始往左看，102 开始回头，到 126 附近已经回到正面
/// （126 也是下一段「看右」的起点）。
const LOOK_LEFT: (u32, u32) = (66, 126);

/// `idle` 素材里「看右 → 回正」的帧区间。
///
/// 126 开始往右看，168 开始回头，约 228 回到正面（cx≈107，和 0 帧一致）。
const LOOK_RIGHT: (u32, u32) = (126, 228);

impl Pet {
    /// 转头看一边：从 `idle` 里截一段「看过去 → 回正」播一遍。
    ///
    /// 不再整段播 `turn`（那是东张西望，10 秒且包含全身转动）；
    /// 用的是 idle 素材里已经存在的左右看，播完自然回到正面。
    fn play_look(&mut self, towards_left: bool) -> bool {
        if self.play("idle").is_err() {
            return false;
        }
        let (lo, hi) = if towards_left { LOOK_LEFT } else { LOOK_RIGHT };
        let last = self.current_count().saturating_sub(1);
        let lo = lo.min(last);
        let hi = hi.min(last).max(lo);
        self.loop_span = Some((lo, hi));
        self.frame = lo;
        // 这一段本身就是「看过去再回正」，不需要额外镜像。
        self.flip = false;
        self.action_playing = true;
        // 取证开关：方向到底对不对（鼠标在左就该是「看左」）。
        if std::env::var_os("FEIYU_EVENT_TRACE").is_some() {
            println!(
                "看向{}（第 {lo}~{hi} 帧）",
                if towards_left { "左" } else { "右" }
            );
        }
        let _ = self.draw_frame();
        true
    }

    /// 用给定的鼠标 x 跑一次「转向跟随」判定，返回是否转头了。
    ///
    /// `tick` 用真实光标调它；自检用假坐标调它——鼠标没法在测试里"移开"。
    pub(crate) fn poll_look(&mut self, cursor_x: i32) -> bool {
        let width = self.current_width().max(80);
        let dx = cursor_x - self.anchor_x;
        let near = dx.abs() <= width;
        // 只在「刚离开」那一拍、而且它在待机时才转头。
        let mut turned = false;
        if self.was_near
            && !near
            && !self.action_playing
            && self.look_cd.elapsed().as_secs_f32() >= LOOK_COOLDOWN
        {
            self.look_cd = Instant::now();
            turned = self.play_look(dx < 0);
        }
        self.was_near = near;
        turned
    }

    /// 把转头冷却清零。只给自检用（否则测一次要真等 4 秒）。
    pub(crate) fn reset_look_cooldown(&mut self) {
        self.look_cd = Instant::now() - std::time::Duration::from_secs(60);
    }
}
