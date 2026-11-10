//! Fenêtre « Préférences » : apparence, langue, notifications et fichiers.
//!
//! La fenêtre ne manipule que des réglages d'interface. Les secrets et les
//! paramètres de connexion restent dans le formulaire de connexion et le
//! trousseau système.

use gpui::{
    AnyElement, Context, Div, FocusHandle, Focusable, MouseButton, MouseDownEvent, Render,
    ScrollHandle, Window, div, prelude::*, px, rgb,
};
use ircv3_core::Config;

use crate::{i18n, theme};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub theme: String,
    pub language: String,
    pub notifications: bool,
    pub in_app_toasts: bool,
    pub sound_notifications: bool,
    pub toast_seconds: u64,
    pub confirm_upload: bool,
    pub notify_mentions_only: bool,
}

impl Preferences {
    pub fn from_config(config: &Config) -> Self {
        Self {
            theme: config.theme.clone(),
            language: config.language.clone(),
            notifications: config.notifications,
            in_app_toasts: config.in_app_toasts,
            sound_notifications: config.sound_notifications,
            toast_seconds: config.toast_seconds.clamp(1, 10),
            confirm_upload: config.confirm_upload,
            notify_mentions_only: config.notify_mentions_only,
        }
    }

    pub fn apply_to(&self, config: &mut Config) {
        config.theme = self.theme.clone();
        config.language = self.language.clone();
        config.notifications = self.notifications;
        config.in_app_toasts = self.in_app_toasts;
        config.sound_notifications = self.sound_notifications;
        config.toast_seconds = self.toast_seconds.clamp(1, 10);
        config.confirm_upload = self.confirm_upload;
        config.notify_mentions_only = self.notify_mentions_only;
    }
}

pub struct PreferencesWindow {
    prefs: Preferences,
    tx: async_channel::Sender<Preferences>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
}

impl PreferencesWindow {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        prefs: Preferences,
        tx: async_channel::Sender<Preferences>,
    ) -> Self {
        window.set_window_title(&format!(
            "{} — Le Salon",
            i18n::t("Préférences", "Preferences")
        ));
        Self {
            prefs,
            tx,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        }
    }

    fn section(title: &'static str, detail: &'static str, content: Div) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme::muted())
                    .child(detail),
            )
            .child(content)
    }

    fn toggle_row(
        &self,
        id: &'static str,
        label: &'static str,
        detail: &'static str,
        checked: bool,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let mark = if checked { "✓" } else { " " };
        div()
            .id(id)
            .flex()
            .items_start()
            .gap_3()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(on_click))
            .child(
                div()
                    .mt(px(1.))
                    .size(px(17.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(if checked {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if checked {
                        theme::accent_dark()
                    } else {
                        theme::input_bg()
                    })
                    .text_size(px(11.))
                    .text_color(theme::accent())
                    .child(mark),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::muted())
                            .child(detail),
                    ),
            )
            .into_any_element()
    }

    fn choice_chip(
        &self,
        label: &'static str,
        selected: bool,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .px_3()
            .py_1()
            .rounded_full()
            .border_1()
            .border_color(if selected {
                theme::accent()
            } else {
                theme::border()
            })
            .bg(if selected {
                theme::accent_dark()
            } else {
                theme::panel_alt()
            })
            .text_color(if selected {
                theme::accent()
            } else {
                theme::text()
            })
            .text_size(px(11.5))
            .cursor_pointer()
            .hover(|style| style.border_color(theme::accent()))
            .on_mouse_down(MouseButton::Left, cx.listener(on_click))
            .child(label)
            .into_any_element()
    }
}

impl Focusable for PreferencesWindow {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PreferencesWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut theme_cards: Vec<AnyElement> = Vec::new();
        for entry in theme::themes() {
            let selected = entry.key == self.prefs.theme;
            let key = entry.key;
            theme_cards.push(
                div()
                    .w(px(148.))
                    .h(px(68.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(entry.bg))
                    .border_2()
                    .border_color(if selected {
                        rgb(entry.accent)
                    } else {
                        rgb(entry.border)
                    })
                    .cursor_pointer()
                    .hover(|style| style.border_color(rgb(entry.accent)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.prefs.theme = key.to_string();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(entry.text))
                            .child(i18n::t(entry.name_fr, entry.name_en)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(div().size(px(12.)).rounded_sm().bg(rgb(entry.accent)))
                            .child(div().size(px(12.)).rounded_sm().bg(rgb(entry.panel_alt)))
                            .child(div().size(px(12.)).rounded_sm().bg(rgb(entry.green)))
                            .child(div().size(px(12.)).rounded_sm().bg(rgb(entry.red))),
                    )
                    .into_any_element(),
            );
        }

        let mut language_chips: Vec<AnyElement> = Vec::new();
        for (code, label) in [
            ("auto", i18n::t("Automatique", "Automatic")),
            ("fr", "Français"),
            ("en", "English"),
        ] {
            let selected = self.prefs.language == code;
            language_chips.push(self.choice_chip(
                label,
                selected,
                move |this, _, _, cx| {
                    this.prefs.language = code.to_string();
                    cx.notify();
                },
                cx,
            ));
        }

        let mut duration_chips: Vec<AnyElement> = Vec::new();
        for seconds in [2u64, 3, 5] {
            let selected = self.prefs.toast_seconds == seconds;
            duration_chips.push(self.choice_chip(
                match seconds {
                    2 => "2 s",
                    3 => "3 s",
                    _ => "5 s",
                },
                selected,
                move |this, _, _, cx| {
                    this.prefs.toast_seconds = seconds;
                    cx.notify();
                },
                cx,
            ));
        }

        let appearance = Self::section(
            i18n::t("Apparence", "Appearance"),
            i18n::t(
                "Le thème et la langue s'appliquent à toutes les fenêtres.",
                "Theme and language apply to every window.",
            ),
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(div().flex().flex_wrap().gap_2().children(theme_cards))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme::muted())
                                .child(i18n::t("Langue :", "Language:")),
                        )
                        .children(language_chips),
                ),
        );

        let notifications = Self::section(
            i18n::t("Notifications", "Notifications"),
            i18n::t(
                "Choisis ce qui déclenche une alerte et comment elle s'affiche.",
                "Choose what triggers an alert and how it is displayed.",
            ),
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(self.toggle_row(
                    "pref-desktop-notifications",
                    i18n::t("Notifications du bureau", "Desktop notifications"),
                    i18n::t(
                        "Afficher une notification système quand la fenêtre est inactive.",
                        "Show a system notification when the window is inactive.",
                    ),
                    self.prefs.notifications,
                    |this, _, _, cx| {
                        this.prefs.notifications = !this.prefs.notifications;
                        cx.notify();
                    },
                    cx,
                ))
                .child(self.toggle_row(
                    "pref-in-app-toasts",
                    i18n::t("Toasts dans la fenêtre", "In-app toasts"),
                    i18n::t(
                        "Afficher un petit popup dans le coin pendant quelques secondes.",
                        "Show a small corner popup for a few seconds.",
                    ),
                    self.prefs.in_app_toasts,
                    |this, _, _, cx| {
                        this.prefs.in_app_toasts = !this.prefs.in_app_toasts;
                        cx.notify();
                    },
                    cx,
                ))
                .child(self.toggle_row(
                    "pref-sound-notifications",
                    i18n::t("Sons de notification", "Notification sounds"),
                    i18n::t(
                        "Jouer un son discret lors d'une alerte.",
                        "Play a discreet sound when an alert fires.",
                    ),
                    self.prefs.sound_notifications,
                    |this, _, _, cx| {
                        this.prefs.sound_notifications = !this.prefs.sound_notifications;
                        cx.notify();
                    },
                    cx,
                ))
                .child(self.toggle_row(
                    "pref-mentions-only",
                    i18n::t(
                        "Mentions et messages privés uniquement",
                        "Mentions and private messages only",
                    ),
                    i18n::t(
                        "Décoche pour être alerté aussi des autres messages des salons.",
                        "Uncheck to also be alerted for other channel messages.",
                    ),
                    self.prefs.notify_mentions_only,
                    |this, _, _, cx| {
                        this.prefs.notify_mentions_only = !this.prefs.notify_mentions_only;
                        cx.notify();
                    },
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme::muted())
                                .child(i18n::t("Durée des toasts :", "Toast duration:")),
                        )
                        .children(duration_chips),
                ),
        );

        let files = Self::section(
            i18n::t("Fichiers", "Files"),
            i18n::t(
                "Les transferts DCC et les uploads ont leurs propres règles de confidentialité.",
                "DCC transfers and uploads have their own privacy rules.",
            ),
            div().flex().flex_col().gap_3().child(self.toggle_row(
                "pref-confirm-upload",
                i18n::t(
                    "Confirmer avant un upload public",
                    "Confirm before a public upload",
                ),
                i18n::t(
                    "Afficher un avertissement avant d'envoyer un fichier sur litterbox (public, 72 h).",
                    "Show a warning before sending a file to litterbox (public, 72 h).",
                ),
                self.prefs.confirm_upload,
                |this, _, _, cx| {
                    this.prefs.confirm_upload = !this.prefs.confirm_upload;
                    cx.notify();
                },
                cx,
            )),
        );

        let security = Self::section(
            i18n::t("Sécurité", "Security"),
            i18n::t(
                "Ces protections ne sont pas désactivables depuis les préférences.",
                "These protections cannot be disabled from preferences.",
            ),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(11.5))
                .text_color(theme::muted())
                .child(i18n::t(
                    "• SASL exige TLS et la vérification du certificat.",
                    "• SASL requires TLS and certificate verification.",
                ))
                .child(i18n::t(
                    "• Les mots de passe sont indexés par réseau dans le trousseau.",
                    "• Passwords are indexed per network in the system keyring.",
                ))
                .child(i18n::t(
                    "• Les toasts affichent le contenu des messages : désactive-les si ton écran est visible.",
                    "• Toasts display message content: disable them if your screen is visible.",
                )),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_size(px(13.))
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_6()
                    .py_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(i18n::t("Préférences", "Preferences")),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme::muted())
                            .child(i18n::t(
                                "Réglages du client Le Salon",
                                "Le Salon client settings",
                            )),
                    ),
            )
            .child(
                div()
                    .id("preferences-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .p_6()
                            .child(appearance)
                            .child(notifications)
                            .child(files)
                            .child(security),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::faint())
                            .child(format!("Le Salon {}", ircv3_core::VERSION)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::panel_alt()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _, window, _| window.remove_window()),
                                    )
                                    .child(i18n::t("Annuler", "Cancel")),
                            )
                            .child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(theme::accent())
                                    .text_color(theme::on_accent())
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::accent_hover()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, _cx| {
                                            let _ = this.tx.try_send(this.prefs.clone());
                                            window.remove_window();
                                        }),
                                    )
                                    .child(i18n::t("Enregistrer", "Save")),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_roundtrip_and_clamp() {
        let mut config = Config::default();
        config.theme = "tux".into();
        config.in_app_toasts = false;
        config.toast_seconds = 3;
        config.confirm_upload = false;
        config.notify_mentions_only = false;
        config.sound_notifications = false;

        let mut prefs = Preferences::from_config(&config);
        assert_eq!(prefs.theme, "tux");
        assert!(!prefs.in_app_toasts);
        assert!(!prefs.confirm_upload);
        assert!(!prefs.notify_mentions_only);
        assert!(!prefs.sound_notifications);

        prefs.toast_seconds = 99;
        prefs.apply_to(&mut config);
        assert_eq!(config.toast_seconds, 10);
        assert_eq!(config.theme, "tux");
        assert!(!config.in_app_toasts);
    }
}
