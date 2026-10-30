//! 系统界面语言：只给「跟随系统」选菜单语言用。
//!
//! 不引 `windows` 的本地化 API（那套要 COM），`GetUserDefaultUILanguage`
//! 拿到的 LANGID 低 10 位就是主语言 id，够判断中/英/西。

use windows::Win32::Globalization::GetUserDefaultUILanguage;

/// 返回 `"zh"` / `"en"` / `"es"`，其它语言返回 `None`（调用方回退英文）。
pub fn system_language() -> Option<&'static str> {
    // SAFETY: 无参数、只读查询。
    let langid = unsafe { GetUserDefaultUILanguage() };
    match langid & 0x3FF {
        0x04 => Some("zh"),
        0x09 => Some("en"),
        0x0C => Some("es"),
        _ => None,
    }
}
