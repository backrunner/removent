use super::*;

impl HomeView {
    pub(super) fn persist_settings(
        &mut self,
        f: impl FnOnce(&mut removent_core::Settings),
    ) -> bool {
        match self.engine.update_settings(f) {
            Ok(()) => true,
            Err(e) => {
                self.set_status(t!("status.settings_save_failed", err = e), StatusTone::Err);
                false
            }
        }
    }

    pub(super) fn save_device_name(&mut self, cx: &mut Context<Self>) {
        let name = self.device_name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.set_status(t!("status.device_name_empty").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        if self.persist_settings(|s| s.device_name = name) {
            self.set_status(t!("status.settings_saved").to_string(), StatusTone::Ok);
        }
        cx.notify();
    }

    pub(super) fn save_vnc(&mut self, cx: &mut Context<Self>) {
        let password = self.vnc_password_input.read(cx).value().to_string();
        if self.persist_settings(|s| {
            s.vnc_password = password;
        }) {
            self.set_status(t!("status.vnc_updated").to_string(), StatusTone::Ok);
        }
        cx.notify();
    }

    pub(super) fn set_theme(
        &mut self,
        pref: ThemePref,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.persist_settings(|s| s.theme = pref) {
            apply_theme_pref(pref, window, cx);
        }
        cx.notify();
    }

    pub(super) fn set_language(
        &mut self,
        lang: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.persist_settings(|s| s.language = lang) {
            cx.notify();
            return;
        }
        rust_i18n::set_locale(removent_core::resolve_locale(lang));
        for (input, key) in [
            (&self.search_input, "home.search_placeholder"),
            (&self.pin_input, "home.pin_placeholder"),
            (&self.device_name_input, "home.name_placeholder"),
            (
                &self.vnc_password_input,
                "settings.vnc_password_placeholder",
            ),
        ] {
            input.update(cx, |input, cx| {
                input.set_placeholder(t!(key).to_string(), window, cx)
            });
        }
        // Status strings are cached translations: reset them, otherwise they keep the
        // previous language until the next status change.
        self.status = t!("status.ready").to_string();
        self.status_tone = StatusTone::Info;
        cx.notify();
    }
}
