//! 素材清单：`manifest.json` 把「动画 id」映射到文件与画布几何。
//!
//! 运行时只认 `id`（`idle` / `sleep` / `drag`…），不认文件名——文件名可以随打包
//! 策略变化（比如换高度重打包），`id` 才是状态机与素材之间的稳定契约。
//!
//! 清单里的 `origin` + `baseline` 解决的是「不同动画的裁切尺寸不一样，怎么摆回同一个
//! 位置」：`origin` 是该帧左上角在原 544×306 画布中的坐标，`baseline` 是**帧内**落脚行。
//! 因此画布中的落脚行 = `origin_y + baseline`，切换动画时只要保持这一行不动，
//! 角色的脚就不会跳。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::format::AnimError;

/// 本程序能读的清单版本。
pub const SUPPORTED_FORMAT: u32 = 1;

/// 一整份清单。
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// 清单结构版本，不是素材版本。
    pub format: u32,
    /// 原始画布尺寸（打包前 ffmpeg 缩放后的尺寸）。
    pub canvas: Canvas,
    /// 原始画布高度，等价于 `canvas.height`，保留作人工核对用。
    pub native_height: u16,
    /// 建议播放帧率。
    pub fps: f32,
    /// 全部动画。
    pub animations: Vec<AnimEntry>,
}

/// 原始画布尺寸。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Canvas {
    pub width: u16,
    pub height: u16,
}

/// 一段动画。
#[derive(Debug, Clone, Deserialize)]
pub struct AnimEntry {
    /// 稳定标识，状态机用它选动画。
    pub id: String,
    /// 人类可读名称（中文，仅菜单/日志展示）。
    pub name: String,
    /// 相对清单所在目录的文件名。
    pub file: String,
    /// 裁切后的帧尺寸 `[宽, 高]`。
    pub size: [u16; 2],
    /// 裁切原点 `[x, y]`，相对原画布。
    pub origin: [u16; 2],
    /// 帧内落脚行（距帧顶的行数）。
    pub baseline: u16,
    /// 帧数。
    pub frames: u32,
    /// 压缩后字节数。
    pub bytes: u64,
}

impl AnimEntry {
    /// 帧宽。
    pub fn width(&self) -> u16 {
        self.size[0]
    }

    /// 帧高。
    pub fn height(&self) -> u16 {
        self.size[1]
    }

    /// 在原画布坐标系里的落脚行：`origin_y + baseline`。
    pub fn feet_row(&self) -> u16 {
        self.origin[1].saturating_add(self.baseline)
    }

    /// 清单磁盘位置 → 动画文件磁盘位置。
    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(&self.file)
    }
}

impl Manifest {
    /// 从 JSON 文本解析，并做完整性校验。
    pub fn from_json(text: &str) -> Result<Self, AnimError> {
        let me: Self =
            serde_json::from_str(text).map_err(|e| AnimError::Manifest(e.to_string()))?;
        me.validate()?;
        Ok(me)
    }

    /// 从磁盘读取。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnimError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_json(&text)
    }

    /// 校验：版本、非空、id 唯一、几何不越界。
    ///
    /// 这些检查放在读入阶段而不是用到深处才炸——桌宠启动时就能报出「清单坏了」，
    /// 比播放到第 7 秒才发现某个 id 不存在要好。
    fn validate(&self) -> Result<(), AnimError> {
        if self.format != SUPPORTED_FORMAT {
            return Err(AnimError::Manifest(format!(
                "清单版本 {} 不支持（本程序只认 {SUPPORTED_FORMAT}）",
                self.format
            )));
        }
        if self.animations.is_empty() {
            return Err(AnimError::Manifest("清单里没有任何动画".into()));
        }
        if !(self.fps.is_finite() && self.fps > 0.0 && self.fps <= 240.0) {
            return Err(AnimError::Manifest(format!("帧率不合理: {}", self.fps)));
        }

        for (i, a) in self.animations.iter().enumerate() {
            if a.id.is_empty() {
                return Err(AnimError::Manifest(format!("第 {i} 条动画缺少 id")));
            }
            if self.animations[..i].iter().any(|b| b.id == a.id) {
                return Err(AnimError::Manifest(format!("动画 id 重复: {}", a.id)));
            }
            if a.size[0] == 0 || a.size[1] == 0 {
                return Err(AnimError::Manifest(format!("{} 的帧尺寸为 0", a.id)));
            }
            if a.frames == 0 {
                return Err(AnimError::Manifest(format!("{} 没有帧", a.id)));
            }
            let right = a.origin[0].saturating_add(a.size[0]);
            let bottom = a.origin[1].saturating_add(a.size[1]);
            if right > self.canvas.width || bottom > self.canvas.height {
                return Err(AnimError::Manifest(format!(
                    "{} 的画布范围越界：origin {:?} + size {:?} 超出 {}×{}",
                    a.id, a.origin, a.size, self.canvas.width, self.canvas.height
                )));
            }
            if a.baseline > a.size[1] {
                return Err(AnimError::Manifest(format!(
                    "{} 的落脚行 {} 超过帧高 {}",
                    a.id, a.baseline, a.size[1]
                )));
            }
        }
        Ok(())
    }

    /// 按 id 取动画。
    pub fn get(&self, id: &str) -> Option<&AnimEntry> {
        self.animations.iter().find(|a| a.id == id)
    }

    /// 按 id 取动画，取不到就报错（状态机里用，避免静默播错段）。
    pub fn require(&self, id: &str) -> Result<&AnimEntry, AnimError> {
        self.get(id)
            .ok_or_else(|| AnimError::Manifest(format!("清单里没有动画 id: {id}")))
    }

    /// 全部 id，按清单顺序。
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.animations.iter().map(|a| a.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "format": 1,
        "canvas": { "width": 544, "height": 306 },
        "native_height": 306,
        "fps": 24,
        "animations": [
            { "id": "idle", "name": "待机", "file": "idle.fishanim",
              "size": [211, 241], "origin": [163, 46], "baseline": 241,
              "frames": 241, "bytes": 5300000 },
            { "id": "sleep", "name": "睡觉", "file": "sleep.fishanim",
              "size": [384, 266], "origin": [84, 24], "baseline": 266,
              "frames": 241, "bytes": 5200000 }
        ]
    }"#;

    #[test]
    fn parses_and_looks_up() {
        let m = Manifest::from_json(SAMPLE).expect("解析应成功");
        assert_eq!(m.fps, 24.0);
        assert_eq!((m.canvas.width, m.canvas.height), (544, 306));
        assert_eq!(m.ids().collect::<Vec<_>>(), vec!["idle", "sleep"]);

        let idle = m.require("idle").expect("idle 存在");
        assert_eq!(idle.width(), 211);
        assert_eq!(idle.height(), 241);
        // 落脚行 = origin_y + baseline，这是切动画时对齐的唯一依据。
        assert_eq!(idle.feet_row(), 287);
        assert_eq!(m.require("sleep").unwrap().feet_row(), 290);
        assert_eq!(
            idle.path_in(Path::new("assets/anim")).to_string_lossy(),
            "assets/anim\\idle.fishanim"
        );
    }

    #[test]
    fn rejects_duplicate_id() {
        let bad = SAMPLE.replace(r#""id": "sleep""#, r#""id": "idle""#);
        match Manifest::from_json(&bad) {
            Err(AnimError::Manifest(msg)) => {
                assert!(msg.contains("重复"), "错误信息应指出重复：{msg}")
            }
            other => panic!("期望拒绝重复 id，实际 {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_version_and_geometry() {
        let bad_version = SAMPLE.replace(r#""format": 1"#, r#""format": 2"#);
        assert!(matches!(
            Manifest::from_json(&bad_version),
            Err(AnimError::Manifest(_))
        ));

        // 把 sleep 的裁切宽度撑到画布之外。
        let bad_geo = SAMPLE.replace(r#""size": [384, 266]"#, r#""size": [500, 266]"#);
        match Manifest::from_json(&bad_geo) {
            Err(AnimError::Manifest(msg)) => assert!(msg.contains("越界"), "应报越界：{msg}"),
            other => panic!("期望拒绝越界几何，实际 {other:?}"),
        }
    }

    #[test]
    fn requires_known_id() {
        let m = Manifest::from_json(SAMPLE).unwrap();
        assert!(m.get("fly").is_none());
        match m.require("fly") {
            Err(AnimError::Manifest(msg)) => assert!(msg.contains("fly")),
            other => panic!("期望报缺少 id，实际 {other:?}"),
        }
    }
}
