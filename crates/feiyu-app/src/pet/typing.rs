//! Typing link: accumulate heat, then play a work clip.

use std::time::Instant;

use anyhow::Result;
use feiyu_core::brain::Mood;

use super::Pet;

/// 打字时播的工作状态段（权重按「像不像在一起忙」排）。
///
/// 成功/出错没有上下文（不接 DSH 任务），只留很小份额当彩蛋。
/// 热度、概率、冷却都在 `Config::typing` 里。
const TYPING_WORK: &[(&str, u32)] = &[
    ("work_busy", 40),
    ("work_think", 25),
    ("work_tidy", 20),
    ("work_pace", 8),
    ("work_cheer", 4),
    ("work_sigh", 3),
];

impl Pet {
    /// 把打字触发概率钉成固定值。只给自检用（否则「有概率」测不稳）。
    pub(crate) fn set_typing_chance(&mut self, chance: f32) {
        if chance.is_finite() {
            self.typing_chance = chance.clamp(0.0, 1.0);
        }
    }

    /// 开/关打字联动。菜单项与自检用。
    pub(crate) fn set_typing_enabled(&mut self, on: bool) {
        self.typing_enabled = on;
    }

    /// 把打字冷却清零。只给自检用（否则要真等 45 秒）。
    pub(crate) fn reset_typing_cooldown(&mut self) {
        self.typing_next = Instant::now() - std::time::Duration::from_secs(1);
    }

    /// 直接喂「刚敲了 n 个键」（不经过真实键盘）。只给自检用。
    pub(crate) fn feed_typing(&mut self, keys: u32) -> Result<bool> {
        self.note_typing(keys, 0.0, Instant::now())
    }

    /// 累计打字热度；够热、且真闲着时掷一次骰子，决定要不要演一段工作动画。
    ///
    /// 睡着 / 正在演别的动作 / 藏着 / 强制模式都不抢戏——打字只是「陪」,
    /// 不该打断已经开始的表演。返回是否真的播了。
    pub(crate) fn note_typing(&mut self, keys: u32, dt: f32, now: Instant) -> Result<bool> {
        // 热度一直衰减：按键不会攒到下一次空闲时一口气放出来。
        self.typing_heat = (self.typing_heat - self.cfg.typing.decay_per_sec * dt).max(0.0);
        if keys == 0 {
            return Ok(false);
        }
        if self.forced.is_some()
            || self.hidden
            || self.waking
            || !self.typing_enabled
            || self.brain.mood() != Mood::Idle
            || self.action_playing
        {
            return Ok(false);
        }
        self.typing_heat = (self.typing_heat + keys as f32).min(self.cfg.typing.warmup_keys * 4.0);
        // 取证开关：`FEIYU_TYPING_EAGER=1` 时不过冷却、必触发。
        // 只用于手动验证「真敲键盘能不能带出工作动画」，正常使用不会设它。
        let eager = std::env::var_os("FEIYU_TYPING_EAGER").is_some();
        if self.typing_heat < self.cfg.typing.warmup_keys || (!eager && now < self.typing_next) {
            return Ok(false);
        }
        // 热度到线：这一下就重置并记冷却。不重置的话每帧都会重掷，
        // 「有概率」会变成「必触发」。
        self.typing_heat = 0.0;
        self.typing_next = if eager {
            now
        } else {
            now + std::time::Duration::from_secs_f32(
                self.rng
                    .range(self.cfg.typing.cooldown.0, self.cfg.typing.cooldown.1),
            )
        };
        let chance = if eager { 1.0 } else { self.typing_chance };
        if self.rng.unit() >= chance {
            return Ok(false);
        }
        let id = self.pick_typing_work();
        // 取证开关：手动打字验证时，看日志里到底有没有走到这一步。
        if std::env::var_os("FEIYU_TYPING_TRACE").is_some() {
            eprintln!("打字联动：热度到线，陪一段 {id}");
        }
        self.play(id)?;
        self.action_playing = true;
        Ok(true)
    }

    /// 按 [`TYPING_WORK`] 的权重抽一段工作动画（缺素材的自动跳过）。
    fn pick_typing_work(&mut self) -> &'static str {
        let total: u32 = TYPING_WORK
            .iter()
            .filter(|(id, _)| self.manifest.get(id).is_some())
            .map(|(_, w)| *w)
            .sum();
        if total == 0 {
            return "work_busy";
        }
        let mut r = self.rng.below(total);
        for (id, w) in TYPING_WORK {
            if self.manifest.get(id).is_none() {
                continue;
            }
            if r < *w {
                return id;
            }
            r -= *w;
        }
        "work_busy"
    }
}
