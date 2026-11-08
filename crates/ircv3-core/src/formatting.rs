//! Formatage de texte IRC (codes mIRC).
//!
//! Les codes de contrôle sont retirés du texte affiché et transformés en
//! plages de style : gras, italique, souligné, barré, monospace, couleurs
//! 16/256 et couleurs hexadécimales.
//!
//! `FormattedText::text` contient le texte nettoyé ; `spans` décrit les
//! plages d'octets (UTF-8) et leurs attributs.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatSpan {
    pub range: Range<usize>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub mono: bool,
    pub reverse: bool,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
}

impl FormatSpan {
    pub fn is_plain(&self) -> bool {
        !self.bold
            && !self.italic
            && !self.underline
            && !self.strikethrough
            && !self.mono
            && !self.reverse
            && self.fg.is_none()
            && self.bg.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormattedText {
    pub text: String,
    pub spans: Vec<FormatSpan>,
}

impl FormattedText {
    pub fn is_plain(&self) -> bool {
        self.spans.iter().all(FormatSpan::is_plain)
    }
}

#[derive(Clone, Debug, Default)]
struct StyleState {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    mono: bool,
    reverse: bool,
    fg: Option<Rgb>,
    bg: Option<Rgb>,
}

pub fn parse(text: &str) -> FormattedText {
    let mut out = String::with_capacity(text.len());
    let mut spans = Vec::new();
    let mut style = StyleState::default();
    let mut span_start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;

    while index < text.len() {
        let c = text[index..].chars().next().unwrap();
        match c {
            '\u{03}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                let mut cursor = index + 1;
                let (fg, next) = read_color_number(bytes, cursor);
                cursor = next;
                if fg.is_some() {
                    style.fg = fg.and_then(mirc_color);
                    if cursor < bytes.len() && bytes[cursor] == b',' {
                        let (bg, next) = read_color_number(bytes, cursor + 1);
                        style.bg = bg.and_then(mirc_color);
                        cursor = next;
                    } else {
                        style.bg = None;
                    }
                } else {
                    style.fg = None;
                    style.bg = None;
                    if cursor < bytes.len() && bytes[cursor] == b',' {
                        cursor += 1;
                    }
                }
                index = cursor;
                continue;
            }
            '\u{04}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                let mut cursor = index + 1;
                let (fg, next) = read_hex_color(bytes, cursor);
                cursor = next;
                if let Some(rgb) = fg {
                    style.fg = Some(rgb);
                    if cursor < bytes.len() && bytes[cursor] == b',' {
                        let (bg, next) = read_hex_color(bytes, cursor + 1);
                        if let Some(rgb) = bg {
                            style.bg = Some(rgb);
                            cursor = next;
                        }
                    }
                }
                index = cursor;
                continue;
            }
            '\u{02}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style.bold = !style.bold;
                index += 1;
                continue;
            }
            '\u{1d}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style.italic = !style.italic;
                index += 1;
                continue;
            }
            '\u{1f}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style.underline = !style.underline;
                index += 1;
                continue;
            }
            '\u{1e}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style.strikethrough = !style.strikethrough;
                index += 1;
                continue;
            }
            '\u{16}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style.reverse = !style.reverse;
                index += 1;
                continue;
            }
            '\u{11}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style.mono = !style.mono;
                index += 1;
                continue;
            }
            '\u{0f}' => {
                flush(&out, &mut spans, &style, &mut span_start);
                style = StyleState::default();
                index += 1;
                continue;
            }
            '\u{01}' | '\u{00}' => {
                index += 1;
                continue;
            }
            c @ ('\t' | '\n' | '\r' | '\u{2028}' | '\u{2029}') => {
                out.push(' ');
                index += c.len_utf8();
                continue;
            }
            c if c.is_control() || is_bidi_control(c) => {
                index += c.len_utf8();
                continue;
            }
            _ => {
                out.push(c);
                index += c.len_utf8();
            }
        }
    }
    flush(&out, &mut spans, &style, &mut span_start);
    FormattedText { text: out, spans }
}

/// Contrôles de direction Unicode : ils permettraient de retourner ou de
/// réordonner visuellement le texte (usurpation d'un lien, d'un pseudo, etc.).
pub(crate) fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn flush(out: &str, spans: &mut Vec<FormatSpan>, style: &StyleState, span_start: &mut usize) {
    if *span_start >= out.len() {
        return;
    }
    spans.push(FormatSpan {
        range: *span_start..out.len(),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strikethrough: style.strikethrough,
        mono: style.mono,
        reverse: style.reverse,
        fg: style.fg,
        bg: style.bg,
    });
    *span_start = out.len();
}

fn read_color_number(bytes: &[u8], start: usize) -> (Option<u8>, usize) {
    let mut cursor = start;
    let mut value = 0u16;
    let mut digits = 0;
    while cursor < bytes.len() && digits < 2 && bytes[cursor].is_ascii_digit() {
        value = value * 10 + (bytes[cursor] - b'0') as u16;
        cursor += 1;
        digits += 1;
    }
    if digits == 0 {
        (None, start)
    } else {
        (Some(value.min(99) as u8), cursor)
    }
}

fn read_hex_color(bytes: &[u8], start: usize) -> (Option<Rgb>, usize) {
    if start + 6 > bytes.len() {
        return (None, start);
    }
    let mut value = 0u32;
    for offset in 0..6 {
        let c = bytes[start + offset];
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return (None, start),
        };
        value = value * 16 + digit as u32;
    }
    (
        Some(Rgb::new(
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        )),
        start + 6,
    )
}

/// Palette mIRC classique (0–15) et couleurs étendues (16–98).
pub fn mirc_color(value: u8) -> Option<Rgb> {
    let palette = [
        (255, 255, 255),
        (0, 0, 0),
        (0, 0, 127),
        (0, 147, 0),
        (255, 0, 0),
        (127, 0, 0),
        (156, 0, 156),
        (252, 127, 0),
        (255, 255, 0),
        (0, 252, 0),
        (0, 147, 147),
        (0, 255, 255),
        (0, 0, 252),
        (255, 0, 255),
        (127, 127, 127),
        (210, 210, 210),
    ];
    match value {
        0..=15 => {
            let (r, g, b) = palette[value as usize];
            Some(Rgb::new(r, g, b))
        }
        16..=98 => {
            let hue = (value - 16) as f32 * 360.0 / 82.0;
            Some(hsv_to_rgb(hue, 0.82, 0.95))
        }
        _ => None,
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let c = v * s;
    let h = (h % 360.0 + 360.0) % 360.0 / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Rgb::new(
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text() {
        let parsed = parse("bonjour à tous");
        assert_eq!(parsed.text, "bonjour à tous");
        assert_eq!(parsed.spans.len(), 1);
        assert!(parsed.spans[0].is_plain());
        assert_eq!(parsed.spans[0].range, 0..parsed.text.len());
    }

    #[test]
    fn bold_toggle() {
        let parsed = parse("a\u{02}b\u{02}c");
        assert_eq!(parsed.text, "abc");
        assert_eq!(parsed.spans.len(), 3);
        assert!(!parsed.spans[0].bold);
        assert!(parsed.spans[1].bold);
        assert!(!parsed.spans[2].bold);
        assert_eq!(parsed.spans[1].range, 1..2);
    }

    #[test]
    fn colors() {
        let parsed = parse("\u{03}4,1rouge\u{0f} normal");
        assert_eq!(parsed.text, "rouge normal");
        assert_eq!(parsed.spans[0].fg, Some(Rgb::new(255, 0, 0)));
        assert_eq!(parsed.spans[0].bg, Some(Rgb::new(0, 0, 0)));
        assert!(parsed.spans[1].fg.is_none());
        assert!(parsed.spans[1].bg.is_none());
    }

    #[test]
    fn hex_colors() {
        let parsed = parse("\u{04}ff8800chaud");
        assert_eq!(parsed.text, "chaud");
        assert_eq!(parsed.spans[0].fg, Some(Rgb::new(255, 136, 0)));
    }

    #[test]
    fn all_flags() {
        let parsed = parse("\u{02}\u{1d}\u{1f}\u{1e}\u{11}x");
        assert_eq!(parsed.text, "x");
        let span = &parsed.spans[0];
        assert!(span.bold && span.italic && span.underline && span.strikethrough && span.mono);
    }

    #[test]
    fn utf8_ranges() {
        let parsed = parse("é\u{02}à");
        assert_eq!(parsed.text, "éà");
        assert_eq!(parsed.spans[0].range, 0..2);
        assert_eq!(parsed.spans[1].range, 2..4);
        assert!(parsed.spans[1].bold);
    }

    #[test]
    fn bidi_controls_are_removed() {
        let parsed = parse("abc\u{202e}def\u{2066}ghi\u{2069}");
        assert_eq!(parsed.text, "abcdefghi");
        let parsed = parse("a\u{2028}b\u{2029}c");
        assert_eq!(parsed.text, "a b c");
    }

    #[test]
    fn bare_color_resets() {
        let parsed = parse("\u{03}4rouge\u{03}normal");
        assert_eq!(parsed.text, "rougenormal");
        assert!(parsed.spans[1].fg.is_none());
    }
}
