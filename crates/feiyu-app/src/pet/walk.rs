//! Walking: displacement advances per frame, in sync with the clip.

use anyhow::Result;

use super::{clip, ClipSpec, Pet, CLIPS};

/// 正在走的一段路。
///
/// 位移按**帧**推进（不是按秒）：这样它和动画片段天然同步——一段素材演完，
/// 路也正好走完，不会出现「人已经到了还在原地踏步」。
pub(super) struct Walk {
    /// 还剩多少像素没走。
    left: f32,
    /// 方向：-1 往左，+1 往右。
    dir: f32,
    /// 每帧走多少像素（**由素材自己决定**，不是按距离算）。
    per_frame: f32,
    /// 这段素材朝哪边（改方向时重新算镜像用）。
    faces_left: Option<bool>,
    /// 已经走到哪（**浮点**累加，只在摆窗口时才取整）。
    ///
    /// 必须浮点：短距离时每帧才 0.4 像素，逐帧 `round()` 会被舍成 0，
    /// 结果就是「原地踏步、窗口不动」。
    x: f32,
}

impl Pet {
    /// 按这段素材自己的参数走一趟：朝向、迈步区间都来自参数表。
    ///
    /// 返回 `false` 表示这段走不了（那个方向已经贴墙，或者素材不在）——
    /// 调用方应把动画退回待机，而不是让它在原地比划。
    pub(crate) fn begin_walk(&mut self, spec: &'static ClipSpec) -> bool {
        self.begin_walk_with(spec, None)
    }

    /// 同上，但可以**指定方向**（`Some(-1.0)` 左 / `Some(1.0)` 右）。
    /// 自检靠它验镜像：不能靠抽签碰运气。
    fn begin_walk_with(&mut self, spec: &'static ClipSpec, forced: Option<f32>) -> bool {
        self.begin_walk_full(spec, forced, None)
    }

    /// 同上，但可以把距离也钉住（跟随鼠标时要“走多远就多远”）。
    fn begin_walk_full(
        &mut self,
        spec: &'static ClipSpec,
        forced: Option<f32>,
        distance: Option<f32>,
    ) -> bool {
        let Some(walk) = spec.walk.as_ref() else {
            return false;
        };
        let Ok(entry) = self.manifest.require(spec.id) else {
            return false;
        };
        let limits = self.limits_for(entry);
        let here = self.anchor_x as f32;
        let room_left = here - limits.x.0;
        let room_right = limits.x.1 - here;
        // 两边都能走就掷硬币，贴墙的一边自动排除。
        let dir = if let Some(d) = forced {
            let room = if d < 0.0 { room_left } else { room_right };
            if room <= 24.0 {
                return false;
            }
            d
        } else {
            match (room_left > 24.0, room_right > 24.0) {
                (true, true) => {
                    if self.rng.unit() < 0.5 {
                        -1.0
                    } else {
                        1.0
                    }
                }
                (true, false) => -1.0,
                (false, true) => 1.0,
                (false, false) => return false,
            }
        };
        // 走的方跟素材朝相反时镜像它（否则就是倒退着走）。
        let flip = match walk.faces_left {
            Some(faces_left) => (dir < 0.0) != faces_left,
            None => false,
        };
        let room = if dir < 0.0 { room_left } else { room_right };
        let want = match distance {
            Some(d) => d.clamp(24.0, self.cfg.walk.max_px * 2.0).min(room),
            None => self
                .rng
                .range(self.cfg.walk.min_px, self.cfg.walk.max_px)
                .min(room),
        };
        if want < 24.0 {
            return false;
        }
        // **速度由素材自己定**（`px_per_frame`），不是拿距离除帧数：
        // 那段走得快就走得快、走得慢就慢，不会每段都一个速度。
        let per_frame = walk.px_per_frame.max(0.05);

        if self.play(spec.id).is_err() {
            return false;
        }
        // 从「真正在迈步」的那一帧开始循环，跳过开头的起势——
        // 这就是用户看到的「先移动、一秒后才开始走」的那个一秒。
        let last = self.current_count().saturating_sub(1);
        let (lo, hi) = match spec.span {
            Some((a, b)) => (a.min(last), b.min(last).max(a.min(last))),
            None => (0, last),
        };
        self.loop_span = Some((lo, hi));
        self.frame = lo;
        self.flip = flip;
        let _ = self.draw_frame();

        self.walk = Some(Walk {
            left: want,
            dir,
            per_frame,
            faces_left: walk.faces_left,
            x: self.anchor_x as f32,
        });
        true
    }

    /// 推进一步位移。走完就把 `walk` 清掉（动画仍由 `step_frames` 收尾）。
    pub(crate) fn step_walk(&mut self, step: u32) -> Result<()> {
        let Some((x, done)) = self.walk.as_mut().map(|w| {
            let s = (w.per_frame * step as f32).min(w.left);
            w.left -= s;
            w.x += w.dir * s;
            (w.x, w.left <= 0.0)
        }) else {
            return Ok(());
        };
        if done {
            // 到站就立刻收：继续把这段走路素材播完会变成「原地踏步」。
            self.walk = None;
            self.action_playing = false;
            self.play("idle")?;
            return Ok(());
        }
        let entry = self.manifest.require(&self.current)?.clone();
        let limits = self.limits_for(&entry);
        // 浮点累加后的位置取整再夹边界：短距离（每帧不到 1px）也能慢慢挪。
        self.anchor_x = x.clamp(limits.x.0, limits.x.1).round() as i32;
        self.place_window(&entry);
        Ok(())
    }

    /// 让指定的走路段走一趟。**只给自检用**：真实走路靠待机池抽签。
    ///
    /// 逐段验：每段都得跳过自己的前摇、朝素材自己的方向走。
    pub(crate) fn wander_with(&mut self, id: &str) -> bool {
        let Some(spec) = clip(id) else {
            return false;
        };
        let started = self.begin_walk(spec);
        if started {
            self.action_playing = true;
        }
        started
    }

    /// 指定方向走一段。自检用：要验「往反方向走时镜像」，不能靠抽签。
    pub(crate) fn wander_dir(&mut self, id: &str, right: bool) -> bool {
        let Some(spec) = clip(id) else {
            return false;
        };
        let started = self.begin_walk_with(spec, Some(if right { 1.0 } else { -1.0 }));
        if started {
            self.action_playing = true;
        }
        started
    }

    /// 朝屏幕上的某个 x 走过去。返回是否真的开始走。
    ///
    /// 暂时没入口：`--follow` 按用户要求改成了**转向跟随**（转头看，不挪步）。
    /// 留着这个方法，将来要「走到鼠标旁边」时接线即可。
    #[allow(dead_code)]
    fn walk_toward(&mut self, x: i32) -> bool {
        let dx = (x - self.anchor_x) as f32;
        // 已经在它旁边了就别再挪：鼠标停在鱼身上时不该一直抖。
        if dx.abs() < 60.0 {
            return false;
        }
        let Some(spec) = CLIPS.iter().find(|c| c.walk.is_some()) else {
            return false;
        };
        let started = self.begin_walk_full(spec, Some(dx.signum()), Some(dx.abs()));
        if started {
            self.action_playing = true;
        }
        started
    }

    /// 当前动画的帧宽（判断「一个肥鱼宽」的范围用）。
    pub(crate) fn current_width(&self) -> i32 {
        self.manifest
            .require(&self.current)
            .map(|e| e.width() as i32)
            .unwrap_or(200)
    }

    /// 朝屏幕上的 x 走（或跑）过去——「点哪走哪」。返回是否真的开始。
    ///
    /// 规则：距离不到**一个肥鱼宽**就不动（那种距离走过去像抽筋）；
    /// 走哪一段按权重抽（目前 `move_float` 6 / `move_crab` 4）。
    pub(crate) fn walk_to(&mut self, x: i32) -> bool {
        let width = self.current_width().max(80);
        let dx = x - self.anchor_x;
        if dx.abs() <= width {
            return false;
        }
        let dir = if dx < 0 { -1.0 } else { 1.0 };
        // 已经在走：**就地改方向**（不切动画、不硬切），立即生效。
        // 这就是「走到一半点新方向，能马上跟着新方向走」。
        if let Some(w) = self.walk.as_mut() {
            w.dir = dir;
            w.left = dx.abs() as f32;
            // 镜像要跟着新方向重算。
            self.flip = match w.faces_left {
                Some(faces_left) => (dir < 0.0) != faces_left,
                None => false,
            };
            return true;
        }
        if self.action_playing {
            return false;
        }
        // 三个移动段等概率（原版 `moves.actions` 也是平权列表）。
        let walkable: Vec<&'static ClipSpec> = CLIPS.iter().filter(|c| c.walk.is_some()).collect();
        if walkable.is_empty() {
            return false;
        }
        let spec = walkable[self.rng.below(walkable.len() as u32) as usize];
        let dir = if dx < 0 { -1.0 } else { 1.0 };
        let started = self.begin_walk_full(spec, Some(dir), Some(dx.abs() as f32));
        if started {
            self.action_playing = true;
        }
        started
    }
}
