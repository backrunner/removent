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
    "chevron-down",
    "chevron-up",
    "clipboard",
    "cloud",
    "copy",
    "eye",
    "eye-off",
    "ellipsis",
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
    "wifi-off",
    "x",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let local = ICONS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, svg)| Cow::Borrowed(svg.as_bytes()));
        if local.is_some() {
            Ok(local)
        } else {
            // Components also request their own icons, including the Windows
            // caption buttons, which are not used by the macOS title bar.
            gpui_kit_assets::Assets.load(path)
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = gpui_kit_assets::Assets.list(path)?;
        paths.extend(
            ICONS
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .map(|(p, _)| SharedString::from(*p)),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_window_controls_have_embedded_icons() {
        use gpui_component::{IconName, IconNamed};
        for icon in [
            IconName::WindowMinimize,
            IconName::WindowMaximize,
            IconName::WindowRestore,
            IconName::WindowClose,
        ] {
            let data = Assets.load(&icon.path()).unwrap().unwrap();
            assert!(std::str::from_utf8(&data).unwrap().contains("<svg"));
        }
    }
}
