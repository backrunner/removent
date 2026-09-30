//! Embedded app assets: icons/*.svg are bundled at compile time and served via GPUI AssetSource.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[
            $((concat!("icons/", $name, ".svg"), include_str!(concat!("../assets/icons/", $name, ".svg")))),*
        ]
    };
}

static ICONS: &[(&str, &str)] = icons![
    "alert-triangle",
    "arrow-left",
    "check",
    "chevron-right",
    "clipboard",
    "copy",
    "eye",
    "folder",
    "gauge",
    "info",
    "loader-circle",
    "maximize",
    "minimize",
    "monitor",
    "moon",
    "plus",
    "power",
    "search",
    "settings",
    "shield-check",
    "sun",
    "trash-2",
    "volume-2",
    "volume-x",
    "wifi",
    "x",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, svg)| Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}
