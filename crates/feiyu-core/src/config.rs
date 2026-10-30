//! 用户可调参数。
//!
//! 这里的值都是“产品参数”：多久抽一次小动作、多久没互动就睡、打字联动多敏感、
//! 自己走多远、每个分类在随机池里占多重。改这些数不该重新编译，所以放在
//! `config.json` 里。
//!
//! 原则：**配置缺失或写错时回退默认值**，最多让调用方打一行告警。桌宠不能因为
//! 一个坏 JSON 起不来。数值的合法性在 [`Config::from_json`] 里收敛（排序、
//! 夹到合理区间），调用方拿到的永远是能直接用的值。

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::i18n::Lang;

/// 全部可调参数。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 两次待机小动作之间的间隔（秒，闭区间，随机取）。
    pub action_gap: (f32, f32),
    /// 多久没互动就睡（秒，闭区间，随机取）。
    pub sleep_after: (f32, f32),
    /// 打字联动。
    pub typing: Typing,
    /// 自己走一段的距离（像素）。
    pub walk: Walk,
    /// 分类标签 → 随机池总权重。没有写的分类用代码里的默认权重。
    pub category_weights: HashMap<String, u32>,
    /// 界面语言。`None` = 跟随系统，认不出的一律英文。
    pub language: Option<Lang>,
    /// 菜单开关：「跟随鼠标」。
    pub follow: bool,
    /// 菜单开关：「点哪走哪」。
    pub click_walk: bool,
    /// 菜单开关：「总在最前」。
    pub topmost: bool,
}

/// 打字联动参数。
#[derive(Debug, Clone, PartialEq)]
pub struct Typing {
    /// 是否默认开启（右键菜单还能再关）。
    pub enabled: bool,
    /// 热度到线后这一次掷骰的触发概率（0~1）。
    pub chance: f32,
    /// 热度阈值：大约等于“连敲多少个键”。
    pub warmup_keys: f32,
    /// 热度每秒衰减多少。
    pub decay_per_sec: f32,
    /// 两次触发之间的冷却（秒，闭区间）。
    pub cooldown: (f32, f32),
}

/// 走路距离参数。
#[derive(Debug, Clone, PartialEq)]
pub struct Walk {
    pub min_px: f32,
    pub max_px: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            action_gap: (8.0, 15.0),
            sleep_after: (18.0, 38.0),
            typing: Typing::default(),
            walk: Walk::default(),
            category_weights: HashMap::new(),
            language: None,
            follow: false,
            click_walk: false,
            topmost: true,
        }
    }
}

impl Default for Typing {
    fn default() -> Self {
        Self {
            enabled: true,
            chance: 0.35,
            warmup_keys: 12.0,
            decay_per_sec: 2.0,
            cooldown: (45.0, 90.0),
        }
    }
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            min_px: 120.0,
            max_px: 420.0,
        }
    }
}

impl Config {
    /// 从 JSON 文本解析。缺的字段用默认值；明显不合理的数值会被夹回合理区间。
    ///
    /// 只把“语法错/类型错”当错误返回；数值范围问题不拒绝整份配置——
    /// 用户写错一个数时，其他设置应该照常生效。
    pub fn from_json(text: &str) -> Result<Self, String> {
        let raw: RawConfig = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let d = Self::default();
        let typing = raw.typing.unwrap_or_default();
        let walk = raw.walk.unwrap_or_default();
        // `walk.min_px` / `max_px` 是两个独立字段，不像成对区间那样能自动排序：
        // 这里显式交换，避免 `rng.range(500, 100)` 这种反向区间把距离算成负数。
        let min_px = number(walk.min_px, d.walk.min_px).clamp(0.0, 100_000.0);
        let max_px = number(walk.max_px, d.walk.max_px).clamp(1.0, 100_000.0);
        let (min_px, max_px) = if min_px <= max_px {
            (min_px, max_px)
        } else {
            (max_px, min_px)
        };

        Ok(Self {
            action_gap: pair(raw.action_gap, d.action_gap, 0.5),
            sleep_after: pair(raw.sleep_after, d.sleep_after, 1.0),
            typing: Typing {
                enabled: typing.enabled.unwrap_or(d.typing.enabled),
                chance: number(typing.chance, d.typing.chance).clamp(0.0, 1.0),
                warmup_keys: number(typing.warmup_keys, d.typing.warmup_keys).clamp(1.0, 1000.0),
                decay_per_sec: number(typing.decay_per_sec, d.typing.decay_per_sec)
                    .clamp(0.0, 100.0),
                cooldown: pair(typing.cooldown, d.typing.cooldown, 0.0),
            },
            walk: Walk { min_px, max_px },
            category_weights: raw
                .category_weights
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v.min(100_000)))
                .collect(),
            // `auto` / 认不出的值都当成“跟随系统”，不因为写错一个语言就把桌宠晾在那。
            language: raw.language.as_deref().and_then(Lang::parse),
            follow: raw.follow.unwrap_or(d.follow),
            click_walk: raw.click_walk.unwrap_or(d.click_walk),
            topmost: raw.topmost.unwrap_or(d.topmost),
        })
    }

    /// 从磁盘读。文件不存在是错误，由调用方决定是回退默认还是报错。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
        Self::from_json(&text).map_err(|e| format!("{} 不是合法的配置：{e}", path.display()))
    }

    /// 某个分类的随机池权重。配置里没写就用代码默认值。
    pub fn category_weight(&self, label: &str, fallback: u32) -> u32 {
        self.category_weights
            .get(label)
            .copied()
            .unwrap_or(fallback)
    }

    /// 把菜单里能改的几项写回配置文件。
    ///
    /// 只覆盖 `language` / `follow` / `click_walk` / `topmost` / `typing.enabled`，
    /// 文件里其余字段原样保留：用户可能手写了别的项，不能因为点了一下菜单就丢了。
    /// 文件不存在时新建；内容不是合法 JSON 时也重写，不报错。
    pub fn save_toggles(path: &Path, cfg: &Config) -> Result<(), String> {
        let mut root: serde_json::Value = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        };
        if !root.is_object() {
            root = serde_json::json!({});
        }
        let obj = root.as_object_mut().expect("刚确认过是对象");
        obj.insert(
            "language".to_owned(),
            serde_json::json!(cfg.language.map(Lang::tag).unwrap_or("auto")),
        );
        obj.insert("follow".to_owned(), serde_json::json!(cfg.follow));
        obj.insert("click_walk".to_owned(), serde_json::json!(cfg.click_walk));
        obj.insert("topmost".to_owned(), serde_json::json!(cfg.topmost));
        let typing = obj.entry("typing").or_insert_with(|| serde_json::json!({}));
        if let Some(t) = typing.as_object_mut() {
            t.insert("enabled".to_owned(), serde_json::json!(cfg.typing.enabled));
        }
        let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
        text.push('\n');
        std::fs::write(path, text).map_err(|e| format!("写不了 {}：{e}", path.display()))
    }
}

/// 把 JSON 里的 `[a, b]` 收敛成一个合法区间：有限、升序、不小于 `min`。
fn pair(value: Option<[f32; 2]>, fallback: (f32, f32), min: f32) -> (f32, f32) {
    let Some([a, b]) = value else {
        return fallback;
    };
    if !a.is_finite() || !b.is_finite() {
        return fallback;
    }
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (lo.max(min), hi.max(min))
}

/// 单个数值：非有限值回退默认。
fn number(value: Option<f32>, fallback: f32) -> f32 {
    match value {
        Some(v) if v.is_finite() => v,
        _ => fallback,
    }
}

// ---- 反序列化的中间结构：所有字段都是 Option，缺了就用默认值 ----

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    action_gap: Option<[f32; 2]>,
    sleep_after: Option<[f32; 2]>,
    typing: Option<RawTyping>,
    walk: Option<RawWalk>,
    category_weights: Option<HashMap<String, u32>>,
    language: Option<String>,
    follow: Option<bool>,
    click_walk: Option<bool>,
    topmost: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTyping {
    enabled: Option<bool>,
    chance: Option<f32>,
    warmup_keys: Option<f32>,
    decay_per_sec: Option<f32>,
    cooldown: Option<[f32; 2]>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawWalk {
    min_px: Option<f32>,
    max_px: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn defaults_are_the_documented_values() {
        let c = Config::default();
        assert_eq!(c.action_gap, (8.0, 15.0));
        assert_eq!(c.sleep_after, (18.0, 38.0));
        assert_eq!(c.typing.chance, 0.35);
        assert_eq!(c.walk.max_px, 420.0);
        assert!(c.category_weights.is_empty());
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let c = Config::from_json(r#"{ "sleep_after": [30, 60] }"#).expect("parse");
        assert_eq!(c.sleep_after, (30.0, 60.0));
        assert_eq!(c.action_gap, (8.0, 15.0), "没写的字段必须保持默认");
    }

    #[test]
    fn reversed_and_bad_numbers_are_sanitized() {
        let c = Config::from_json(
            r#"{ "action_gap": [20, 10], "sleep_after": [-5, 3], "typing": { "chance": 9, "cooldown": [90, 45] } }"#,
        )
        .expect("parse");
        assert_eq!(c.action_gap, (10.0, 20.0), "倒着写要自动排序");
        assert_eq!(c.sleep_after, (1.0, 3.0), "负数夹到下限");
        assert_eq!(c.typing.chance, 1.0, "概率夹到 0~1");
        assert_eq!(c.typing.cooldown, (45.0, 90.0));
    }

    #[test]
    fn reversed_walk_range_is_sorted() {
        let c =
            Config::from_json(r#"{ "walk": { "min_px": 500, "max_px": 100 } }"#).expect("parse");
        assert_eq!(c.walk.min_px, 100.0);
        assert_eq!(c.walk.max_px, 500.0);
    }

    #[test]
    fn category_weights_override_defaults() {
        let c = Config::from_json(r#"{ "category_weights": { "玩耍": 0, "文字": 50 } }"#)
            .expect("parse");
        assert_eq!(c.category_weight("玩耍", 20), 0);
        assert_eq!(c.category_weight("文字", 10), 50);
        assert_eq!(c.category_weight("吃什么", 16), 16, "没写的分类用默认");
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let c = Config::from_json(r#"{ "no_such_field": 1, "typing": { "nope": true } }"#)
            .expect("未知字段不该让整份配置失败");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn syntax_error_is_reported() {
        assert!(Config::from_json("{ oops").is_err());
    }

    #[test]
    fn toggles_and_language_parse() {
        let c = Config::from_json(
            r#"{ "language": "es-ES", "follow": true, "click_walk": true, "topmost": false }"#,
        )
        .expect("parse");
        assert_eq!(c.language, Some(Lang::Es));
        assert!(c.follow && c.click_walk);
        assert!(!c.topmost);

        let d = Config::from_json(r#"{ "language": "auto" }"#).expect("parse");
        assert_eq!(d.language, None, "auto = 跟随系统");
        assert!(!d.follow && !d.click_walk && d.topmost, "没写就用默认值");
    }

    #[test]
    fn save_toggles_round_trips_and_keeps_other_fields() {
        let dir = std::env::temp_dir().join(format!("feiyu-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{ "action_gap": [5, 6], "custom": 1 }"#).expect("seed");

        let mut cfg = Config::load(&path).expect("load");
        cfg.follow = true;
        cfg.click_walk = true;
        cfg.topmost = false;
        cfg.typing.enabled = false;
        cfg.language = Some(Lang::Es);
        Config::save_toggles(&path, &cfg).expect("save");

        let back = Config::load(&path).expect("reload");
        assert!(back.follow && back.click_walk);
        assert!(!back.topmost && !back.typing.enabled);
        assert_eq!(back.language, Some(Lang::Es));
        assert_eq!(back.action_gap, (5.0, 6.0), "没动的字段要保留");

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"custom\""), "未知字段不能被抹掉");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_toggles_creates_missing_file() {
        let dir = std::env::temp_dir().join(format!("feiyu-cfg-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        let cfg = Config {
            follow: true,
            ..Config::default()
        };
        Config::save_toggles(&path, &cfg).expect("save");
        assert!(Config::load(&path).expect("load").follow);
        std::fs::remove_dir_all(&dir).ok();
    }
}
