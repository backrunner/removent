//! Locale resolution: map the `language` setting to a concrete locale tag.

use crate::settings::Language;

/// Resolve the effective locale tag ("en" or "zh-CN") for the UI language preference.
pub fn resolve_locale(pref: Language) -> &'static str {
    match pref {
        Language::En => "en",
        Language::ZhCn => "zh-CN",
        Language::System => system_locale(),
    }
}

/// System locale mapped onto the supported set; anything non-Chinese falls back to English.
fn system_locale() -> &'static str {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("zh") => "zh-CN",
        _ => "en",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_preferences_win() {
        assert_eq!(resolve_locale(Language::En), "en");
        assert_eq!(resolve_locale(Language::ZhCn), "zh-CN");
    }

    #[test]
    fn system_falls_back_to_a_supported_locale() {
        let l = resolve_locale(Language::System);
        assert!(l == "en" || l == "zh-CN");
    }
}
