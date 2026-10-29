//! Animation data tables: per-clip specs, categories, pool weights.

use feiyu_core::config::Config;

/// 一段动画自己的参数——**驱动逻辑只认这张表**。
///
/// 素材是画家一段段画的，每段的起势、收势、朝向都不一样：统一处理就会出现
/// 「人先动、一秒后才迈腿」（前摇没跳）和「素材朝左、人却往右走」（朝向没跟素材）。
///
/// **只有需要特殊处理的段才写进来**：整段循环、位置不动的那 95 段走默认口径
/// （[`clip`] 返回 `None`），不在这张表里重复登记。
pub(crate) struct ClipSpec {
    /// 素材 id。
    pub(crate) id: &'static str,
    /// 一句话说清「这段应该看到什么」。`--showcase` 会把它打出来给人对照。
    pub(crate) desc: &'static str,
    /// 只播这一截（闭区间，已跳过前摇/收势）。`None` = 整段循环。
    pub(crate) span: Option<(u32, u32)>,
    /// 这段要**带着位移**播（抽中它就去走一段）。
    pub(crate) walk: Option<WalkSpec>,
    /// 切到这段时要做的事。
    pub(crate) on_enter: OnEnter,
}

/// 走路段自己的参数。
pub(crate) struct WalkSpec {
    /// 素材本身朝哪边：`Some(true)` 朝左、`Some(false)` 朝右、`None` 方向不明显。
    ///
    /// 不做水平翻转，所以朝左的段就只能往左走；方向不明显的段两边都行。
    pub(crate) faces_left: Option<bool>,
    /// **这段素材自己的移动速度**（像素/帧）。
    ///
    /// 必须按段给：侧身跳步和正面小碎步的步频不一样，用统一速度就会“滑步”。
    /// `24fps` 下：`px_per_frame × 24` 就是每秒多少像素。
    pub(crate) px_per_frame: f32,
}

/// 切进某段时要做的收尾。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnEnter {
    /// 什么都不用做（小动作、点击反馈）。
    Plain,
    /// 回到待机：重抽入睡时间与下次小动作。
    Idle,
    /// 起睡眠编排（三段式，见 `core/sleep.rs`）。
    Sleep,
}

/// 一段「点击回应」。点一下从这 5 段里随机抽一段，不连抽同一个。
pub(crate) const CLICK_IDS: &[&str] = &[
    "click_happy",
    "click_shy",
    "click_pout",
    "click_tickle",
    "click_wave",
];

/// 素材分类：决定**随机池权重**与右键菜单「动作 → 分类 → 具体动画」的层级。
///
/// `weight` 照上游 `config.jsonc` 的 `animationWeights` 填（turn 5 / move 5 /
/// 小动作 20 / 玩耍 20 / 吃什么 16 / 时节 14 / 文字 10）；`0` = 不进随机链——
/// 点击回应、碎碎念、工作状态在原版里也只由事件或菜单显式触发。
///
/// 清单顺序 = 菜单顺序。`clips` 里的 id 必须与 `manifest.json` 一致（打包时由
/// `tools/pack-anim` 的 `NAME_MAP` 决定），缺段的会在建池/建菜单时自动过滤。
pub(crate) struct Category {
    pub(crate) label: &'static str,
    pub(crate) weight: u32,
    pub(crate) clips: &'static [&'static str],
}

pub(crate) const CATEGORIES: &[Category] = &[
    Category {
        label: "转向",
        weight: 5,
        clips: &["turn"],
    },
    Category {
        label: "移动",
        weight: 5,
        clips: &["move_crab", "move_float", "move_run"],
    },
    // 小动作 17 段：原版 18 段里的「原地小憩沉眠」不在这儿——
    // 它是系统睡觉状态（`sleep`）的素材，进了随机池会真把它哄睡。
    Category {
        label: "小动作",
        weight: 20,
        clips: &[
            "hum",
            "stretch",
            "tap_desk",
            "squat",
            "yawn",
            "curtsey",
            "startle",
            "spin360",
            "snack_caught",
            "tail_pat",
            "doze_startle",
            "mirror",
            "dressup",
            "note",
            "code",
            "fan",
            "brush_teeth",
        ],
    },
    Category {
        label: "玩耍",
        weight: 20,
        clips: &[
            "rubik",
            "toy_car",
            "bubbles",
            "jump_grab",
            "game_rage",
            "water_gun",
            "violin",
            "whale",
            "maid_dance",
            "sway_dance",
            "otaku_dance",
            "balloon",
            "animals",
            "kite",
            "gift",
            "dove",
            "poker",
            "top",
            "flute",
            "butterflies",
            "pet_cat",
            "flowers",
            "hobby_horse",
            "juggle",
            "shuttlecock",
            "gomoku",
            "swing",
        ],
    },
    Category {
        label: "吃什么",
        weight: 16,
        clips: &[
            "eat_rice",
            "eat_snack",
            "eat_token",
            "eat_breakfast",
            "eat_lunch",
            "eat_dinner",
            "eat_icecream",
            "eat_crab",
            "eat_tanghulu",
            "eat_noodles",
            "eat_watermelon",
            "hotpot",
        ],
    },
    Category {
        label: "时节",
        weight: 14,
        clips: &[
            "leaves",
            "mooncake",
            "snowman",
            "fireworks",
            "eat_zongzi",
            "eat_niangao",
            "eat_qingtuan",
            "eat_laba",
            "eat_chongyang",
            "red_packet",
            "fu_character",
            "needle",
            "lion_dance",
            "halloween",
            "chrysanthemum",
            "river_lantern",
            "ghost",
            "christmas",
            "sky_lantern",
            "eat_tangyuan",
            "eat_dumpling",
        ],
    },
    Category {
        label: "文字",
        weight: 10,
        clips: &["what_to_eat", "deep_think"],
    },
    Category {
        label: "点击回应",
        weight: 0,
        clips: CLICK_IDS,
    },
    // 下面两类的原触发源（DSH 会话的碎碎念 / 工作状态）已剔除，
    // 素材不浪费：留在菜单里可以随时点播。
    Category {
        label: "说话",
        weight: 0,
        clips: &["whisper_wipe", "whisper_daze", "whisper_screen"],
    },
    Category {
        label: "工作状态",
        weight: 10,
        clips: &[
            "work_think",
            "work_busy",
            "work_tidy",
            "work_pace",
            "work_cheer",
            "work_sigh",
        ],
    },
];

/// 把分类权重摊到每一段上时的放大倍数。
///
/// 分类权重本身只有 20/27 这样的整数，直接除会被取整抹平（20 ÷ 27 = 0，
/// 整类凭空消失）；乘 1000 之后每段至少拿到一位数，比例仍是原版的。
pub(crate) const WEIGHT_SCALE: u32 = 1000;

/// 拖拽段从第 35 帧开始循环：素材开头还站在地上，跳过。
pub(crate) const DRAG_LO: u32 = 35;
/// 拖到第 110 帧收住：再往后是「被放下」的收势。
pub(crate) const DRAG_HI: u32 = 110;

/// 全部动画的参数表。
///
/// 帧区间都是**逐帧量出来的**（`--dump` + 逐帧差），别凭印象改：
///
/// | 段 | 区间 | 依据 |
/// |---|---|---|
/// | `drag` | 35~110 | 前 35 帧人还站在地上 |
/// | `move_float` | 18~205 | 0~17 帧站着，18 帧起踏步；205 后收势 |
/// | `move_crab` | 40~205 | 前 8 帧不动，40 帧才侧身迈步；**朝左** |
/// | `move_run` | 42~156 | 42 帧起转身，108~144 贴地俯冲，150 后起身站好 |
pub(crate) const CLIPS: &[ClipSpec] = &[
    ClipSpec {
        id: "idle",
        desc: "站着呼吸，偶尔眨眼；它会一直循环，直到困了或被点醒",
        span: None,
        walk: None,
        // 回待机要重抽入睡时间与下次小动作（`after_switch` 靠它）。
        on_enter: OnEnter::Idle,
    },
    ClipSpec {
        id: "sleep",
        desc: "站着 → 闭眼端下 → Z 气泡冒出来 → 睡着；点一下就慢慢起身醒来",
        span: None,
        walk: None,
        on_enter: OnEnter::Sleep,
    },
    ClipSpec {
        id: "drag",
        desc: "被提起来悬空飘着（从第 35 帧起：前 35 帧人还站在地上）",
        span: Some((DRAG_LO, DRAG_HI)),
        walk: None,
        on_enter: OnEnter::Plain,
    },
    ClipSpec {
        id: "move_float",
        desc: "漂浮踏步：正面小碎步（演示时原地播；实际会带着走）",
        span: Some((18, 205)),
        walk: Some(WalkSpec {
            faces_left: None,
            // 小碎步，腿走得快：约 62px/秒。
            px_per_frame: 2.6,
        }),
        on_enter: OnEnter::Plain,
    },
    ClipSpec {
        id: "move_crab",
        desc: "螃蟹走路：侧身向左迈步（演示时原地播；实际朝哪边走就镜像成哪边）",
        span: Some((40, 205)),
        walk: Some(WalkSpec {
            faces_left: Some(true),
            // 横着挪，步幅小：约 29px/秒——和漂浮踏步（≈62）明显不一样。
            px_per_frame: 1.2,
        }),
        on_enter: OnEnter::Plain,
    },
    ClipSpec {
        id: "move_run",
        desc: "原地左转奔跑：转身 → 低姿俯冲 → 起身；第 108~144 帧贴地，所以只在 42~156 里循环",
        span: Some((42, 156)),
        walk: Some(WalkSpec {
            faces_left: Some(true),
            // 转身 + 俯冲合计 4.8 秒，原版一次走 120~320px：约 53px/秒。
            px_per_frame: 2.2,
        }),
        on_enter: OnEnter::Plain,
    },
];

/// 查某段的特殊参数。不在表里的段（绝大多数）按整段循环、位置不动处理。
pub(crate) fn clip(id: &str) -> Option<&'static ClipSpec> {
    CLIPS.iter().find(|c| c.id == id)
}

/// 随机池的全部 `(id, 权重)`：把每个分类的总权重均摊到它的每一段上。
///
/// 权重来自配置（`Config::category_weights`），没写的分类用代码里的默认值。
/// 不在任何分类里的 `idle` / `sleep` / `drag` 不进池：它们由状态机驱动。
pub(crate) fn pool_items(cfg: &Config) -> Vec<(&'static str, u32)> {
    let mut items = Vec::new();
    for cat in CATEGORIES {
        let weight = cfg.category_weight(cat.label, cat.weight);
        if weight == 0 {
            continue;
        }
        let per = (weight * WEIGHT_SCALE / cat.clips.len().max(1) as u32).max(1);
        for id in cat.clips {
            items.push((*id, per));
        }
    }
    items
}
