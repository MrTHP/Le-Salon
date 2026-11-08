//! Formulaire de connexion (serveur, pseudo, SASL, salons).

use gpui::{
    Context, Div, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    Render, SharedString, Subscription, Window, div, prelude::*, px,
};

use ircv3_core::{Config, ConnectionConfig, config::parse_channels};

use crate::{
    i18n,
    text_input::{TextInput, TextInputEvent},
    theme, tooltip,
};

#[derive(Clone, Debug)]
pub enum ConnectFormEvent {
    Submit {
        connection: ConnectionConfig,
        remember_password: bool,
    },
    Cancel,
}

pub struct ConnectForm {
    server: Entity<TextInput>,
    port: Entity<TextInput>,
    nick: Entity<TextInput>,
    realname: Entity<TextInput>,
    sasl_account: Entity<TextInput>,
    sasl_password: Entity<TextInput>,
    channels: Entity<TextInput>,
    tls: bool,
    verify: bool,
    remember_password: bool,
    saved_host: String,
    saved_account: String,
    password_cleared: bool,
    allow_cancel: bool,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    focus_first: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ConnectFormEvent> for ConnectForm {}

impl ConnectForm {
    pub fn new(cx: &mut Context<Self>, config: &Config, allow_cancel: bool) -> Self {
        let server = cx.new(|cx| {
            TextInput::new(cx, "irc.libera.chat", config.server.clone(), false).with_tab_stop(1)
        });
        let port = cx
            .new(|cx| TextInput::new(cx, "6697", config.port.to_string(), false).with_tab_stop(2));
        let nick = cx.new(|cx| {
            TextInput::new(
                cx,
                i18n::t("monpseudo", "mynick"),
                config.nick.clone(),
                false,
            )
            .with_tab_stop(3)
        });
        let realname = cx.new(|cx| {
            TextInput::new(
                cx,
                i18n::t("Nom réel", "Real name"),
                config.realname.clone(),
                false,
            )
            .with_tab_stop(4)
        });
        let sasl_account = cx.new(|cx| {
            TextInput::new(
                cx,
                i18n::t("compte (optionnel)", "account (optional)"),
                config.sasl_account.clone(),
                false,
            )
            .with_tab_stop(5)
        });
        let sasl_password = cx.new(|cx| {
            TextInput::new(
                cx,
                i18n::t("mot de passe SASL", "SASL password"),
                if config.remember_sasl_password {
                    config.sasl_password.clone()
                } else {
                    String::new()
                },
                true,
            )
            .with_tab_stop(6)
        });
        let channels = cx.new(|cx| {
            TextInput::new(cx, "#fedora,#rust", config.channels.clone(), false).with_tab_stop(7)
        });
        let mut form = Self {
            server,
            port,
            nick,
            realname,
            sasl_account,
            sasl_password,
            channels,
            tls: config.tls,
            verify: config.verify_tls,
            remember_password: config.remember_sasl_password,
            saved_host: config.server.trim().to_string(),
            saved_account: config.sasl_account.trim().to_string(),
            password_cleared: !config.remember_sasl_password || config.sasl_password.is_empty(),
            allow_cancel,
            error: None,
            focus_handle: cx.focus_handle(),
            focus_first: true,
            _subs: Vec::new(),
        };
        let inputs = [
            form.server.clone(),
            form.port.clone(),
            form.nick.clone(),
            form.realname.clone(),
            form.sasl_account.clone(),
            form.sasl_password.clone(),
            form.channels.clone(),
        ];
        for input in inputs {
            let subscription = cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| {
                this.on_input_event(event, cx);
            });
            form._subs.push(subscription);
        }
        form
    }

    fn on_input_event(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Submitted(_) => self.submit(cx),
            TextInputEvent::Escape => {
                if self.allow_cancel {
                    cx.emit(ConnectFormEvent::Cancel);
                }
            }
            TextInputEvent::Changed(_) => {
                self.clear_password_if_identity_changed(cx);
                if self.error.is_some() {
                    self.error = None;
                    cx.notify();
                }
            }
            TextInputEvent::Tab => {}
        }
    }

    /// Un mot de passe mémorisé n'est valable que pour un couple
    /// (hôte, compte). Dès que l'un change, on efface le champ pour éviter
    /// d'envoyer le secret d'un réseau à un autre.
    fn clear_password_if_identity_changed(&mut self, cx: &mut Context<Self>) {
        if self.password_cleared {
            return;
        }
        let host = self.server.read(cx).content().trim().to_string();
        let account = self.sasl_account.read(cx).content().trim().to_string();
        if host != self.saved_host || account != self.saved_account {
            self.sasl_password.update(cx, |input, cx| input.clear(cx));
            self.password_cleared = true;
            cx.notify();
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let server = self.server.read(cx).content().trim().to_string();
        let port_text = self.port.read(cx).content().trim().to_string();
        let nick = self.nick.read(cx).content().trim().to_string();
        let realname = self.realname.read(cx).content().trim().to_string();
        let sasl_account = self.sasl_account.read(cx).content().trim().to_string();
        let sasl_password = self.sasl_password.read(cx).content().to_string();
        let channels = self.channels.read(cx).content().to_string();

        if server.is_empty() {
            self.set_error(
                i18n::t("Indiquez un serveur IRC.", "Enter an IRC server."),
                cx,
            );
            return;
        }
        let scheme = server.to_ascii_lowercase();
        if scheme.starts_with("irc://") && self.tls {
            self.set_error(
                i18n::t(
                    "irc:// implique une connexion en clair : décochez TLS ou utilisez ircs://.",
                    "irc:// implies a clear-text connection: uncheck TLS or use ircs://.",
                ),
                cx,
            );
            return;
        }
        if scheme.starts_with("ircs://") && !self.tls {
            self.set_error(
                i18n::t(
                    "ircs:// exige TLS : cochez « Connexion TLS ».",
                    "ircs:// requires TLS: check “TLS connection”.",
                ),
                cx,
            );
            return;
        }
        if nick.is_empty() {
            self.set_error(i18n::t("Indiquez un pseudo.", "Enter a nickname."), cx);
            return;
        }
        let port = match port_text.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                self.set_error(
                    i18n::t("Port invalide (1–65535).", "Invalid port (1–65535)."),
                    cx,
                );
                return;
            }
        };
        if !sasl_account.is_empty() && sasl_password.is_empty() {
            self.set_error(
                i18n::t(
                    "Indiquez le mot de passe SASL, ou videz le compte SASL.",
                    "Enter the SASL password, or clear the SASL account.",
                ),
                cx,
            );
            return;
        }
        let sasl_password = (!sasl_password.is_empty()).then_some(sasl_password);
        let connection = ConnectionConfig {
            host: server,
            port,
            tls: self.tls,
            verify_tls: self.verify,
            nick: nick.clone(),
            user: nick,
            realname: if realname.is_empty() {
                self.nick.read(cx).content().to_string()
            } else {
                realname
            },
            sasl_account: (!sasl_account.is_empty()).then_some(sasl_account),
            sasl_password,
            channels: parse_channels(&channels),
            auto_reconnect: true,
        }
        .normalized();
        if let Err(error) = connection.security_check() {
            self.set_error(error, cx);
            return;
        }
        cx.emit(ConnectFormEvent::Submit {
            connection,
            remember_password: self.remember_password,
        });
    }

    fn set_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn toggle_tls(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.tls = !self.tls;
        if self.tls {
            // Activer TLS réactive la vérification : on ne veut pas d'un TLS
            // « décoratif » qui accepterait n'importe quel certificat.
            self.verify = true;
        }
        let port = self.port.read(cx).content().to_string();
        if self.tls && port == "6667" {
            self.port.update(cx, |input, cx| input.set_text("6697", cx));
        } else if !self.tls && port == "6697" {
            self.port.update(cx, |input, cx| input.set_text("6667", cx));
        }
        cx.notify();
    }

    fn toggle_verify(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.verify = !self.verify;
        cx.notify();
    }

    fn toggle_remember(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.remember_password = !self.remember_password;
        cx.notify();
    }

    fn cancel(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ConnectFormEvent::Cancel);
    }

    fn connect_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    fn field(label: &'static str, hint: &'static str, input: Entity<TextInput>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::faint())
                            .child(hint),
                    ),
            )
            .child(input)
    }

    fn grow(child: impl IntoElement) -> Div {
        div().flex_1().min_w_0().child(child)
    }

    fn checkbox(
        id: &'static str,
        label: &'static str,
        tooltip_title: &'static str,
        tooltip_detail: &'static str,
        checked: bool,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let mark = if checked { "✓" } else { " " };
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .tooltip(move |_, cx| tooltip::simple(cx, tooltip_title, tooltip_detail))
            .on_mouse_down(MouseButton::Left, cx.listener(on_click))
            .child(
                div()
                    .size(px(16.))
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
                    .text_size(px(12.5))
                    .text_color(theme::text())
                    .child(label),
            )
            .into_any_element()
    }
}

impl Render for ConnectForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_first {
            self.focus_first = false;
            window.focus(&self.server.read(cx).focus_handle(cx));
        }

        let mut card =
            div()
                .w(px(560.))
                .max_w_full()
                .flex()
                .flex_col()
                .gap_4()
                .p_6()
                .rounded_lg()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .shadow_lg()
                .track_focus(&self.focus_handle)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme::text())
                                .child(if self.allow_cancel {
                                    i18n::t("Nouveau réseau", "New network")
                                } else {
                                    i18n::t("Se connecter à IRC", "Connect to IRC")
                                }),
                        )
                        .child(div().text_size(px(12.5)).text_color(theme::muted()).child(
                            i18n::t(
                                "IRCv3 · TLS · SASL PLAIN · négociation CAP 302",
                                "IRCv3 · TLS · SASL PLAIN · CAP 302 negotiation",
                            ),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .items_end()
                        .child(Self::grow(Self::field(
                            i18n::t("SERVEUR", "SERVER"),
                            i18n::t("hôte ou ircs://hôte", "host or ircs://host"),
                            self.server.clone(),
                        )))
                        .child(div().w(px(96.)).flex_none().child(Self::field(
                            i18n::t("PORT", "PORT"),
                            "TLS 6697",
                            self.port.clone(),
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(Self::grow(Self::field(
                            i18n::t("PSEUDO", "NICK"),
                            i18n::t("votre nick", "your nick"),
                            self.nick.clone(),
                        )))
                        .child(Self::grow(Self::field(
                            i18n::t("NOM RÉEL", "REAL NAME"),
                            i18n::t("affiché par /whois", "shown by /whois"),
                            self.realname.clone(),
                        ))),
                )
                .child(Self::field(
                    i18n::t("SALONS À REJOINDRE", "CHANNELS TO JOIN"),
                    i18n::t("séparés par des virgules", "comma-separated"),
                    self.channels.clone(),
                ))
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(Self::grow(Self::field(
                            i18n::t("COMPTE SASL", "SASL ACCOUNT"),
                            i18n::t("optionnel", "optional"),
                            self.sasl_account.clone(),
                        )))
                        .child(Self::grow(Self::field(
                            i18n::t("MOT DE PASSE SASL", "SASL PASSWORD"),
                            i18n::t("mémorisable (trousseau)", "can be saved (keyring)"),
                            self.sasl_password.clone(),
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_5()
                        .child(Self::checkbox(
                            "connect-tls",
                            i18n::t("Connexion TLS", "TLS connection"),
                            i18n::t("Connexion TLS", "TLS connection"),
                            i18n::t(
                                "Chiffrer la connexion (recommandé, port 6697)",
                                "Encrypt the connection (recommended, port 6697)",
                            ),
                            self.tls,
                            Self::toggle_tls,
                            cx,
                        ))
                        .child(Self::checkbox(
                            "connect-verify",
                            i18n::t("Vérifier le certificat", "Verify certificate"),
                            i18n::t("Vérification du certificat", "Certificate verification"),
                            i18n::t(
                                "Refuser un certificat invalide ou non approuvé",
                                "Reject invalid or untrusted certificates",
                            ),
                            self.verify,
                            Self::toggle_verify,
                            cx,
                        ))
                        .child(Self::checkbox(
                            "connect-remember",
                            i18n::t("Mémoriser le mot de passe SASL", "Remember SASL password"),
                            i18n::t("Trousseau système", "System keyring"),
                            i18n::t(
                                "Conserver le mot de passe dans le trousseau système",
                                "Keep the password in the system keyring",
                            ),
                            self.remember_password,
                            Self::toggle_remember,
                            cx,
                        )),
                )
                .child(
                    div()
                        .text_size(px(9.5))
                        .text_color(theme::faint())
                        .child(i18n::t(
                            "Le mot de passe mémorisé est confié au trousseau système, par réseau. Sans trousseau disponible, il est stocké en clair dans ~/.config/le-salon (0600).",
                            "A remembered password is kept in the system keyring, per network. Without one, it is stored in plain text in ~/.config/le-salon (0600).",
                        )),
                )
                .when(!self.tls, |this| {
                    this.child(
                        div()
                            .rounded_md()
                            .bg(theme::error_alpha(0.09))
                            .border_1()
                            .border_color(theme::error_alpha(0.33))
                            .px_3()
                            .py_2()
                            .text_size(px(11.5))
                            .text_color(theme::red())
                            .child(i18n::t(
                                "⚠ Connexion non chiffrée : SASL est bloqué et tes messages sont lisibles par le réseau. Active TLS si possible.",
                                "⚠ Unencrypted connection: SASL is blocked and your messages are readable by the network. Enable TLS if possible.",
                            )),
                    )
                })
                .when(self.tls && !self.verify, |this| {
                    this.child(
                        div()
                            .rounded_md()
                            .bg(theme::error_alpha(0.09))
                            .border_1()
                            .border_color(theme::error_alpha(0.33))
                            .px_3()
                            .py_2()
                            .text_size(px(11.5))
                            .text_color(theme::red())
                            .child(i18n::t(
                                "⚠ Vérification du certificat désactivée : un MITM peut intercepter la connexion et voler le mot de passe. Ne l'utilise que pour un serveur local de confiance.",
                                "⚠ Certificate verification disabled: a MITM can intercept the connection and steal the password. Only use this for a trusted local server.",
                            )),
                    )
                });

        if let Some(error) = &self.error {
            card = card.child(
                div()
                    .rounded_md()
                    .bg(theme::error_alpha(0.09))
                    .border_1()
                    .border_color(theme::error_alpha(0.33))
                    .px_3()
                    .py_2()
                    .text_size(px(12.5))
                    .text_color(theme::red())
                    .child(error.clone()),
            );
        }

        card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::faint())
                        .child(i18n::t(
                            "Entrée pour valider · mot de passe mémorisé si l'option est cochée",
                            "Enter to submit · password saved when the option is checked",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .when(self.allow_cancel, |this| {
                            this.child(
                                div()
                                    .id("connect-cancel")
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::muted())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::panel_alt()))
                                    .tooltip(|_, cx| {
                                        tooltip::simple(
                                            cx,
                                            i18n::t("Annuler", "Cancel"),
                                            i18n::t(
                                                "Fermer sans se connecter",
                                                "Close without connecting",
                                            ),
                                        )
                                    })
                                    .on_mouse_down(MouseButton::Left, cx.listener(Self::cancel))
                                    .child(i18n::t("Annuler", "Cancel")),
                            )
                        })
                        .child(
                            div()
                                .id("connect-submit")
                                .px_5()
                                .py_2()
                                .rounded_md()
                                .bg(theme::accent())
                                .text_color(theme::on_accent())
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::accent_hover()))
                                .tooltip(|_, cx| {
                                    tooltip::simple(
                                        cx,
                                        i18n::t("Se connecter", "Connect"),
                                        i18n::t(
                                            "Établir la connexion IRC (Entrée)",
                                            "Establish the IRC connection (Enter)",
                                        ),
                                    )
                                })
                                .on_mouse_down(MouseButton::Left, cx.listener(Self::connect_click))
                                .child(i18n::t("Se connecter", "Connect")),
                        ),
                ),
        )
    }
}
