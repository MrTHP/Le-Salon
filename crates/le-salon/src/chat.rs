//! Rendu d'une ligne de discussion : horodatage, pseudo coloré, texte
//! formaté (mIRC) et lignes d'événement.

use std::collections::HashMap;

use gpui::{
    AnyElement, Font, FontStyle, FontWeight, Hsla, StrikethroughStyle, StyledText, TextRun,
    UnderlineStyle, div, prelude::*, px, rgb,
};

use ircv3_core::{
    formatting::Rgb,
    irc_fold,
    model::{ChatLine, LineKind, User},
};

use crate::{theme, tooltip};

pub fn line_row(
    line: &ChatLine,
    font: &Font,
    users: &HashMap<String, User>,
    index: usize,
) -> AnyElement {
    let color = theme::line_color(line.kind);
    let mut row = div()
        .flex()
        .items_start()
        .gap_2()
        .px_3()
        .py(px(2.))
        .line_height(px(20.))
        .text_size(px(13.5))
        .when(line.mention, |row| {
            row.bg(theme::accent_alpha(0.09))
                .border_l_2()
                .border_color(theme::accent())
                .pl(px(10.))
        });

    row = row.child(
        div()
            .w(px(60.))
            .flex_none()
            .text_size(px(10.5))
            .font_family(theme::MONO_FONT)
            .text_color(theme::faint())
            .child(line.at.format("%H:%M:%S").to_string()),
    );

    match line.kind {
        LineKind::Message | LineKind::History => {
            let nick = line.nick.as_deref().unwrap_or("?");
            let nick_color = if line.self_nick {
                theme::accent()
            } else {
                theme::nick_color(nick)
            };
            let nick_text = format!("<{nick}>");
            row.child(nick_element(index, nick, nick_text, nick_color, users))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(color)
                        .child(styled_text(line, font, color)),
                )
                .into_any_element()
        }
        LineKind::Action => {
            let nick = line.nick.as_deref().unwrap_or("?");
            row.child(nick_element(
                index,
                nick,
                format!("* {nick}"),
                theme::nick_color(nick),
                users,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .italic()
                    .text_color(color)
                    .child(styled_text(line, font, color)),
            )
            .into_any_element()
        }
        LineKind::Notice => {
            let nick = line.nick.as_deref().unwrap_or("serveur");
            row.child(nick_element(
                index,
                nick,
                format!("-{nick}-"),
                theme::nick_color(nick),
                users,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(color)
                    .child(styled_text(line, font, color)),
            )
            .into_any_element()
        }
        _ => {
            let icon = match line.kind {
                LineKind::Join => "→",
                LineKind::Part | LineKind::Quit => "←",
                LineKind::Nick => "↻",
                LineKind::Mode => "⚙",
                LineKind::Topic => "✎",
                LineKind::Kick => "⚠",
                LineKind::Invite => "✉",
                LineKind::Away => "◌",
                LineKind::Account => "●",
                LineKind::Error => "⚠",
                LineKind::Server | LineKind::Motd | LineKind::Info => "•",
                LineKind::History => "⟲",
                _ => "·",
            };
            row.child(div().w(px(16.)).flex_none().text_color(color).child(icon))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(color)
                        .child(styled_text(line, font, color)),
                )
                .into_any_element()
        }
    }
}

/// Pseudo cliquable avec mini-profil au survol (liste des membres connue).
fn nick_element(
    index: usize,
    nick: &str,
    text: String,
    color: Hsla,
    users: &HashMap<String, User>,
) -> AnyElement {
    let mut element = div()
        .id(("chat-nick", index))
        .flex_none()
        .max_w(px(160.))
        .truncate()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color);
    if let Some(user) = users.get(&irc_fold(nick)).cloned() {
        element = element.tooltip(move |_, cx| tooltip::user_card(cx, &user, false));
    }
    element.child(text).into_any_element()
}

fn styled_text(line: &ChatLine, font: &Font, default_color: Hsla) -> AnyElement {
    if line.text.is_empty() {
        return div().into_any_element();
    }
    let mut runs: Vec<TextRun> = Vec::with_capacity(line.spans.len().max(1));
    let mut covered = 0usize;
    for span in &line.spans {
        let len = span.range.end.saturating_sub(span.range.start);
        if len == 0 {
            continue;
        }
        let mut run_font = font.clone();
        if span.bold {
            run_font.weight = FontWeight::BOLD;
        }
        if span.italic {
            run_font.style = FontStyle::Italic;
        }
        if span.mono {
            run_font.family = theme::MONO_FONT.into();
        }
        let (color, background) = if span.reverse {
            let fg = span
                .bg
                .or(span.fg)
                .map(|value| rgb(rgb_from(value)).into())
                .unwrap_or(default_color);
            let bg = span
                .fg
                .or(span.bg)
                .map(|value| rgb(rgb_from(value)).into())
                .unwrap_or_else(|| theme::bg());
            (fg, Some(bg))
        } else {
            (
                span.fg
                    .map(|value| rgb(rgb_from(value)).into())
                    .unwrap_or(default_color),
                span.bg.map(|value| rgb(rgb_from(value)).into()),
            )
        };
        runs.push(TextRun {
            len,
            font: run_font,
            color,
            background_color: background,
            underline: span.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: span.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        });
        covered += len;
    }
    if covered == 0 {
        runs.push(TextRun {
            len: line.text.len(),
            font: font.clone(),
            color: default_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    } else if covered < line.text.len() {
        runs.push(TextRun {
            len: line.text.len() - covered,
            font: font.clone(),
            color: default_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    StyledText::new(line.text.clone())
        .with_runs(runs)
        .into_any_element()
}

fn rgb_from(value: Rgb) -> u32 {
    ((value.r as u32) << 16) | ((value.g as u32) << 8) | value.b as u32
}
