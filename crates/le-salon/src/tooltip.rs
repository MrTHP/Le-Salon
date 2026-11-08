//! Info-bulles communes.
//!
//! GPUI affiche automatiquement la vue après un court survol (500 ms) et la
//! place près du curseur. Les cartes ci-dessous reprennent les couleurs du
//! thème actif et les textes passent par l'i18n FR/EN.

use gpui::{
    AnyView, App, AppContext as _, Context, Hsla, IntoElement, ParentElement, Render, Styled,
    Window, div, px,
};
use ircv3_core::model::User;

use crate::{i18n, theme};

pub struct TooltipView {
    title: String,
    title_color: Hsla,
    subtitle: Option<String>,
    rows: Vec<(String, String)>,
    hint: Option<String>,
}

impl TooltipView {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            title_color: theme::text(),
            subtitle: None,
            rows: Vec::new(),
            hint: None,
        }
    }

    pub fn title_color(mut self, color: Hsla) -> Self {
        self.title_color = color;
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn row(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.rows.push((label.into(), value.into()));
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = div()
            .max_w(px(340.))
            .min_w(px(150.))
            .flex()
            .flex_col()
            .gap(px(3.))
            .px_3()
            .py_2()
            .rounded_md()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .shadow_lg()
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(self.title_color)
                    .child(self.title.clone()),
            );

        if let Some(subtitle) = &self.subtitle {
            card = card.child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme::muted())
                    .child(subtitle.clone()),
            );
        }

        for (label, value) in &self.rows {
            card = card.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .text_size(px(10.5))
                    .child(
                        div()
                            .w(px(62.))
                            .flex_none()
                            .text_color(theme::faint())
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme::muted())
                            .child(value.clone()),
                    ),
            );
        }

        if let Some(hint) = &self.hint {
            card = card.child(
                div()
                    .pt_1()
                    .text_size(px(9.5))
                    .text_color(theme::faint())
                    .child(hint.clone()),
            );
        }

        card
    }
}

/// Info-bulle courte : titre + explication.
pub fn simple(cx: &mut App, title: &'static str, detail: &'static str) -> AnyView {
    cx.new(|_| TooltipView::new(title).subtitle(detail)).into()
}

/// Mini-profil affiché au survol d'un pseudo.
/// `clickable` ajoute l'indication des actions disponibles (liste des membres).
pub fn user_card(cx: &mut App, user: &User, clickable: bool) -> AnyView {
    let mut card = TooltipView::new(user.nick.clone())
        .title_color(theme::nick_color(&user.nick))
        .subtitle(role_label(&user.prefixes));

    if user.away {
        card = card.row(i18n::t("État", "Status"), i18n::t("absent", "away"));
    }
    if let Some(account) = user.account.as_deref().filter(|value| !value.is_empty()) {
        card = card.row(i18n::t("Compte", "Account"), account);
    }
    if let Some(user_name) = user.user.as_deref().filter(|value| !value.is_empty()) {
        let host = user
            .host
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("?");
        card = card.row(i18n::t("Hôte", "Host"), format!("{user_name}@{host}"));
    } else if let Some(host) = user.host.as_deref().filter(|value| !value.is_empty()) {
        card = card.row(i18n::t("Hôte", "Host"), host);
    }
    if let Some(realname) = user.realname.as_deref().filter(|value| !value.is_empty()) {
        card = card.row(i18n::t("Nom réel", "Real name"), realname);
    }

    if clickable {
        card = card.hint(i18n::t(
            "Clic : discussion · clic droit : menu",
            "Click: chat · right-click: menu",
        ));
    }
    cx.new(|_| card).into()
}

fn role_label(prefixes: &str) -> &'static str {
    for prefix in prefixes.chars() {
        match prefix {
            '~' => return i18n::t("Propriétaire", "Owner"),
            '&' => return i18n::t("Administrateur", "Admin"),
            '@' => return i18n::t("Opérateur", "Operator"),
            '%' => return i18n::t("Demi-opérateur", "Half-op"),
            '+' => return i18n::t("Voix", "Voice"),
            _ => {}
        }
    }
    i18n::t("Membre", "Member")
}
