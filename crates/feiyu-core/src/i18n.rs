//! 界面文案：中文 / English / Español。
//!
//! 只有右键菜单会展示文案，所以这里就一份平铺的表：菜单命令、十个分类、
//! 以及 100 段动画的显示名。文案直接写死在代码里，不依赖素材清单里那份中文名，
//! 这样老素材包（v0.1.0 / v0.1.1 的 manifest 只有中文名）也能切语言。
//!
//! 新增一段动画时，除了 `NAME_MAP` 和 `CATEGORIES`，还要在 [`CLIPS`] 里补一条，
//! 否则英文/西语菜单会回退到中文名（测试 `every_manifest_id_has_a_translation`
//! 会点名缺失的 id）。

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
    Es,
}

impl Lang {
    /// 解析 `zh` / `en` / `es`（大小写不敏感，`zh-cn` 这类也认）。
    pub fn parse(text: &str) -> Option<Self> {
        let head = text
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match head.as_str() {
            "zh" => Some(Self::Zh),
            "en" => Some(Self::En),
            "es" => Some(Self::Es),
            _ => None,
        }
    }

    /// 写回 `config.json` 的值。
    pub fn tag(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
            Self::Es => "es",
        }
    }

    /// 语言选择菜单里显示的**本名**（当前语言是什么就写什么，不翻译）。
    pub fn native_name(self) -> &'static str {
        match self {
            Self::Zh => "简体中文",
            Self::En => "English",
            Self::Es => "Español",
        }
    }

    /// 从系统语言标签推断。认不出的一律回英文。
    pub fn from_system(tag: Option<&str>) -> Self {
        tag.and_then(Self::parse).unwrap_or(Self::En)
    }

    pub fn texts(self) -> &'static Texts {
        match self {
            Self::Zh => &ZH,
            Self::En => &EN,
            Self::Es => &ES,
        }
    }
}

/// 一张语言的菜单文案。
#[derive(Debug, Clone, Copy)]
pub struct Texts {
    pub hide: &'static str,
    pub show: &'static str,
    pub home: &'static str,
    pub follow: &'static str,
    pub click_walk: &'static str,
    pub typing: &'static str,
    pub topmost: &'static str,
    pub actions: &'static str,
    pub language: &'static str,
    pub quit: &'static str,
}

pub static ZH: Texts = Texts {
    hide: "隐藏到托盘",
    show: "唤出桌宠",
    home: "回到屏幕底部",
    follow: "跟随鼠标",
    click_walk: "点哪走哪",
    typing: "打字联动",
    topmost: "总在最前",
    actions: "动作",
    language: "语言",
    quit: "退出",
};

pub static EN: Texts = Texts {
    hide: "Hide to tray",
    show: "Show the pet",
    home: "Move to bottom centre",
    follow: "Follow the cursor",
    click_walk: "Click to walk",
    typing: "Typing companion",
    topmost: "Always on top",
    actions: "Actions",
    language: "Language",
    quit: "Quit",
};

pub static ES: Texts = Texts {
    hide: "Ocultar en la bandeja",
    show: "Mostrar la mascota",
    home: "Volver abajo al centro",
    follow: "Seguir el cursor",
    click_walk: "Clic para caminar",
    typing: "Acompañar al teclear",
    topmost: "Siempre visible",
    actions: "Acciones",
    language: "Idioma",
    quit: "Salir",
};

/// 分类名。键是 `CATEGORIES` 里的中文标签。
pub fn category(label_zh: &str, lang: Lang) -> &'static str {
    if lang == Lang::Zh {
        return "";
    }
    for (zh, en, es) in CATEGORIES {
        if *zh == label_zh {
            return if lang == Lang::En { en } else { es };
        }
    }
    ""
}

const CATEGORIES: &[(&str, &str, &str)] = &[
    ("转向", "Looking around", "Mirar alrededor"),
    ("移动", "Moving", "Moverse"),
    ("小动作", "Little actions", "Pequeñas acciones"),
    ("玩耍", "Playing", "Jugar"),
    ("吃什么", "Eating", "Comer"),
    ("时节", "Seasons", "Estaciones"),
    ("文字", "Text", "Texto"),
    ("点击回应", "Click responses", "Respuestas al clic"),
    ("说话", "Chatter", "Charla"),
    ("工作状态", "Work", "Trabajo"),
];

/// 单段动画的显示名。`None` = 这张表里没登记（调用方回退中文名）。
pub fn clip(slug: &str, lang: Lang) -> Option<&'static str> {
    if lang == Lang::Zh {
        return None;
    }
    CLIPS
        .iter()
        .find(|(id, _, _)| *id == slug)
        .map(|(_, en, es)| if lang == Lang::En { *en } else { *es })
}

const CLIPS: &[(&str, &str, &str)] = &[
    ("idle", "Idle breathing", "Respirar tranquila"),
    ("sleep", "Dozing on the spot", "Siesta en el sitio"),
    ("drag", "Dangling from the cursor", "Colgando del cursor"),
    ("turn", "Look around", "Mirar alrededor"),
    ("move_crab", "Crab walk", "Caminar de cangrejo"),
    ("move_float", "Float in place", "Flotar en el sitio"),
    ("move_run", "Run in place", "Correr en el sitio"),
    ("click_happy", "Click: happy hop", "Clic: salto feliz"),
    ("click_shy", "Click: shy surprise", "Clic: sorpresa tímida"),
    ("click_pout", "Click: playful pout", "Clic: mohín juguetón"),
    (
        "click_tickle",
        "Click: giggly tickle",
        "Clic: cosquillas risueñas",
    ),
    ("click_wave", "Click: cheerful wave", "Clic: saludo alegre"),
    ("hum", "Hum a tune", "Tararear"),
    ("stretch", "Big stretch", "Gran estiramiento"),
    ("tap_desk", "Tap the desk", "Tocar la mesa"),
    ("squat", "Squat down", "Agacharse"),
    ("yawn", "Yawn", "Bostezo"),
    ("curtsey", "Maid curtsey", "Reverencia de doncella"),
    ("startle", "Startled", "Sobresalto"),
    ("spin360", "Spin 360°", "Giro de 360°"),
    ("snack_caught", "Caught snacking", "Pillada comiendo"),
    (
        "tail_pat",
        "Drum the floor with the tail",
        "Golpear el suelo con la cola",
    ),
    ("doze_startle", "Doze and jolt awake", "Cabezada y susto"),
    ("mirror", "Check the mirror", "Mirarse al espejo"),
    ("dressup", "Try on outfits", "Probarse ropa"),
    ("note", "Take quick notes", "Tomar notas"),
    ("code", "Write code", "Escribir código"),
    ("fan", "Fan yourself", "Abanicarse"),
    ("brush_teeth", "Morning brush", "Cepillarse los dientes"),
    ("rubik", "Rubik's cube", "Cubo de Rubik"),
    ("toy_car", "Toy car", "Coche de juguete"),
    ("bubbles", "Blow bubbles", "Soplar burbujas"),
    ("jump_grab", "Jump and grab", "Saltar y atrapar"),
    ("game_rage", "Gamer rage", "Rabia de jugador"),
    ("water_gun", "Water gun", "Pistola de agua"),
    ("violin", "Play the violin", "Tocar el violín"),
    ("whale", "A whale appears", "Aparece una ballena"),
    ("maid_dance", "Maid dance", "Baile de doncella"),
    ("sway_dance", "Sway dance", "Baile balanceado"),
    ("otaku_dance", "Otaku dance", "Baile otaku"),
    ("balloon", "Blow a balloon", "Inflar un globo"),
    ("animals", "Animal friends", "Amigos animales"),
    ("kite", "Fly a kite", "Volar una cometa"),
    ("gift", "Open a gift", "Abrir un regalo"),
    ("dove", "Turn into a dove", "Convertirse en paloma"),
    ("poker", "Card trick", "Truco de cartas"),
    ("top", "Spin a top", "Hacer girar el trompo"),
    ("flute", "Play the flute", "Tocar la flauta"),
    (
        "butterflies",
        "Butterflies and flowers",
        "Mariposas y flores",
    ),
    ("pet_cat", "Pet the cat", "Acariciar al gato"),
    ("flowers", "Flowers out of thin air", "Flores de la nada"),
    ("hobby_horse", "Rocking horse", "Caballo de balancín"),
    ("juggle", "Juggle three balls", "Malabares con tres bolas"),
    ("shuttlecock", "Shuttlecock", "Volante (jianzi)"),
    ("gomoku", "Gomoku", "Cinco en línea"),
    ("swing", "On the swing", "En el columpio"),
    ("eat_rice", "Eat plain rice", "Comer arroz blanco"),
    ("eat_snack", "Munch a big snack", "Atracón de snacks"),
    ("eat_token", "Eat a token", "Comer un token"),
    ("eat_breakfast", "Breakfast", "Desayuno"),
    ("eat_lunch", "Lunch", "Almuerzo"),
    ("eat_dinner", "Dinner", "Cena"),
    ("eat_icecream", "Melting ice cream", "Helado derritiéndose"),
    ("eat_crab", "Eat crab", "Comer cangrejo"),
    ("eat_tanghulu", "Candied hawthorn", "Espino confitado"),
    ("eat_noodles", "Long-life noodles", "Fideos de longevidad"),
    ("eat_watermelon", "Eat watermelon", "Comer sandía"),
    ("hotpot", "Hot pot", "Olla caliente"),
    ("leaves", "Buried in leaves", "Enterrada en hojas"),
    ("mooncake", "Mooncake night", "Noche de pastel de luna"),
    ("snowman", "Build a snowman", "Hacer un muñeco de nieve"),
    ("fireworks", "Fireworks", "Fuegos artificiales"),
    ("eat_zongzi", "Zongzi", "Zongzi"),
    ("eat_niangao", "Rice cake", "Pastel de arroz"),
    ("eat_qingtuan", "Green rice ball", "Bola de arroz verde"),
    ("eat_laba", "Laba porridge", "Gachas de Laba"),
    ("eat_chongyang", "Chongyang cake", "Pastel de Chongyang"),
    ("red_packet", "Red envelope", "Sobre rojo"),
    (
        "fu_character",
        "Write the 福 character",
        "Escribir el carácter 福",
    ),
    ("needle", "Needle and thread", "Aguja e hilo"),
    ("lion_dance", "Lion dance", "Danza del león"),
    ("halloween", "Pumpkin lantern", "Calabaza de Halloween"),
    ("chrysanthemum", "Chrysanthemums", "Crisantemos"),
    ("river_lantern", "River lantern", "Farolillo en el río"),
    ("ghost", "Cute little ghost", "Fantasmita"),
    ("christmas", "Decorate the tree", "Decorar el árbol"),
    ("sky_lantern", "Sky lantern", "Farol de cielo"),
    ("eat_tangyuan", "Tangyuan", "Tangyuan"),
    ("eat_dumpling", "Dumplings", "Empanadillas"),
    ("what_to_eat", "What should I eat?", "¿Qué como?"),
    ("deep_think", "Deep thinking", "Pensar a fondo"),
    (
        "whisper_wipe",
        "Muttering: wipe it down",
        "Murmullo: limpiar",
    ),
    (
        "whisper_daze",
        "Muttering: spaced out",
        "Murmullo: distraída",
    ),
    (
        "whisper_screen",
        "Muttering at the screen",
        "Murmullo a la pantalla",
    ),
    ("work_think", "Work: thinking", "Trabajo: pensar"),
    ("work_busy", "Work: busy typing", "Trabajo: teclear"),
    ("work_tidy", "Work: filing", "Trabajo: archivar"),
    ("work_pace", "Work: pacing", "Trabajo: pasearse"),
    ("work_cheer", "Work: cheering", "Trabajo: celebrar"),
    ("work_sigh", "Work: sighing", "Trabajo: suspirar"),
];

/// 某段缺哪种语言的译名（`None` = 中英西齐全）。自检用：
/// 新加了素材却忘了补译名时，菜单会在英文/西语下掉回中文名，这里能点名。
pub fn missing_translation(slug: &str) -> Option<&'static str> {
    for lang in [Lang::En, Lang::Es] {
        if clip(slug, lang).is_none() {
            return Some(lang.tag());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_tags_and_rejects_junk() {
        assert_eq!(Lang::parse("zh"), Some(Lang::Zh));
        assert_eq!(Lang::parse("ZH-cn"), Some(Lang::Zh));
        assert_eq!(Lang::parse("en_US"), Some(Lang::En));
        assert_eq!(Lang::parse("es"), Some(Lang::Es));
        assert_eq!(Lang::parse("auto"), None);
        assert_eq!(Lang::parse(""), None);
    }

    #[test]
    fn from_system_falls_back_to_english() {
        assert_eq!(Lang::from_system(Some("es-ES")), Lang::Es);
        assert_eq!(Lang::from_system(Some("zh-Hans")), Lang::Zh);
        assert_eq!(Lang::from_system(Some("fr")), Lang::En);
        assert_eq!(Lang::from_system(None), Lang::En);
    }

    #[test]
    fn table_has_no_duplicates() {
        let mut ids: Vec<&str> = CLIPS.iter().map(|(id, _, _)| *id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(n, ids.len(), "CLIPS 里有重复 id");
        assert_eq!(n, 100, "100 段素材都该有译名");
    }

    #[test]
    fn every_category_is_translated() {
        for (zh, en, es) in CATEGORIES {
            assert!(!zh.is_empty() && !en.is_empty() && !es.is_empty());
            assert_eq!(category(zh, Lang::En), *en);
            assert_eq!(category(zh, Lang::Es), *es);
            assert_eq!(category(zh, Lang::Zh), "");
        }
    }

    #[test]
    fn clips_fall_back_to_chinese() {
        assert_eq!(clip("idle", Lang::Zh), None);
        assert_eq!(clip("idle", Lang::En), Some("Idle breathing"));
        assert_eq!(clip("nope", Lang::En), None);
    }
}
