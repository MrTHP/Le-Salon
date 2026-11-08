//! Fenêtre « Liste des salons » : navigateur de salons du serveur, avec
//! recherche, nombre d'utilisateurs, sujet et double-clic pour rejoindre.

use std::sync::{Arc, Mutex};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent, Render,
    ScrollHandle, Subscription, Task, Window, div, prelude::*, px,
};

use ircv3_core::{
    client::LineSender,
    i18n::t,
    model::{AppModel, NetworkId},
};

use crate::{
    text_input::{TextInput, TextInputEvent},
    theme, tooltip,
};

pub struct ChannelListWindow {
    model: Arc<Mutex<AppModel>>,
    sender: LineSender,
    network_id: NetworkId,
    network_name: String,
    search: Entity<TextInput>,
    selected: Option<String>,
    focus_handle: FocusHandle,
    focus_search: bool,
    scroll: ScrollHandle,
    _search_sub: Option<Subscription>,
    _tick: Task<()>,
}

impl ChannelListWindow {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        model: Arc<Mutex<AppModel>>,
        sender: LineSender,
        network_id: NetworkId,
        network_name: String,
        initial_filter: String,
    ) -> Self {
        window.set_window_title(&format!(
            "{} — {network_name}",
            t("Liste des salons", "Channel list")
        ));
        let search = cx.new(|cx| {
            TextInput::new(
                cx,
                t(
                    "Rechercher un salon ou un sujet…",
                    "Search a channel or topic…",
                ),
                initial_filter,
                false,
            )
        });
        let subscription =
            cx.subscribe(&search, |this, _, event: &TextInputEvent, cx| match event {
                TextInputEvent::Changed(_) => cx.notify(),
                TextInputEvent::Submitted(_) => this.join_first(cx),
                TextInputEvent::Escape => {}
                TextInputEvent::Tab => {}
            });
        let tick = cx.spawn(async move |this, cx| {
            loop {
                gpui::Timer::after(std::time::Duration::from_millis(500)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let mut window_view = Self {
            model,
            sender,
            network_id,
            network_name,
            search,
            selected: None,
            focus_handle: cx.focus_handle(),
            focus_search: true,
            scroll: ScrollHandle::new(),
            _search_sub: Some(subscription),
            _tick: tick,
        };
        window_view.refresh(cx);
        window_view
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        {
            let mut model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.start_channel_list(self.network_id);
        }
        self.sender.send_input("/list");
        self.selected = None;
        cx.notify();
    }

    fn join(&mut self, channel: String, window: &mut Window, _cx: &mut Context<Self>) {
        self.sender.send_input(format!("/join {channel}"));
        window.remove_window();
    }

    fn join_first(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).content().to_lowercase();
        let channel = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.channel_list(self.network_id).and_then(|state| {
                if let Some(selected) = &self.selected {
                    return Some(selected.clone());
                }
                state
                    .entries
                    .iter()
                    .find(|entry| {
                        query.is_empty()
                            || entry.channel.to_lowercase().contains(&query)
                            || entry.topic.to_lowercase().contains(&query)
                    })
                    .map(|entry| entry.channel.clone())
            })
        };
        if let Some(channel) = channel {
            self.sender.send_input(format!("/join {channel}"));
            self.selected = Some(channel);
            cx.notify();
        }
    }

    fn close(&mut self, window: &mut Window) {
        window.remove_window();
    }
}

impl Focusable for ChannelListWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChannelListWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_search {
            self.focus_search = false;
            let handle = self.search.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
        let query = self.search.read(cx).content().to_lowercase();
        let (entries, loading, complete) = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            match model.channel_list(self.network_id) {
                Some(state) => (state.entries.clone(), state.loading, state.complete),
                None => (Vec::new(), true, false),
            }
        };
        let filtered: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.channel.to_lowercase().contains(&query)
                    || entry.topic.to_lowercase().contains(&query)
            })
            .collect();
        let total_users: usize = filtered.iter().map(|entry| entry.users).sum();
        let mut rows: Vec<gpui::AnyElement> = Vec::with_capacity(filtered.len());
        for entry in &filtered {
            let channel = entry.channel.clone();
            let selected = self.selected.as_deref() == Some(entry.channel.as_str());
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(28.))
                    .px_3()
                    .border_b_1()
                    .border_color(theme::border())
                    .cursor_pointer()
                    .when(selected, |this| this.bg(theme::accent_dark()))
                    .hover(|style| style.bg(theme::panel_alt()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.selected = Some(channel.clone());
                            if event.click_count >= 2 {
                                this.join(channel.clone(), window, cx);
                            } else {
                                cx.notify();
                            }
                        }),
                    )
                    .child(
                        div()
                            .w(px(210.))
                            .flex_none()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::accent())
                            .child(entry.channel.clone()),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(entry.users.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(entry.topic.clone()),
                    )
                    .into_any_element(),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_color(theme::text())
            .font_family(theme::UI_FONT)
            .text_size(px(13.))
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .bg(theme::panel())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(t("Liste des salons", "Channel list")),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(self.network_name.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("channel-list-refresh")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(theme::border())
                            .text_size(px(11.))
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::panel_alt()))
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    t("Actualiser", "Refresh"),
                                    t(
                                        "Redemander la liste des salons au serveur",
                                        "Request the channel list from the server again",
                                    ),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.refresh(cx)),
                            )
                            .child(t("Actualiser", "Refresh")),
                    )
                    .child(
                        div()
                            .id("channel-list-close")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::panel_alt()))
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    t("Fermer", "Close"),
                                    t("Fermer la liste des salons", "Close the channel list"),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, _| this.close(window)),
                            )
                            .child("✕"),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .bg(theme::panel())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(self.search.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .bg(theme::panel_alt())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(px(9.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::faint())
                    .child(div().w(px(210.)).flex_none().child(t("SALON", "CHANNEL")))
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .child(t("UTILISATEURS", "USERS")),
                    )
                    .child(div().flex_1().child(t("SUJET", "TOPIC"))),
            )
            .child(
                div()
                    .id("channel-list-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .when(filtered.is_empty(), |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .py_10()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(if loading {
                                    t("Interrogation du serveur…", "Querying server…")
                                } else if !complete {
                                    t("En attente de la liste…", "Waiting for the list…")
                                } else if query.is_empty() {
                                    t("Aucun salon.", "No channel.")
                                } else {
                                    t("Aucun salon ne correspond.", "No matching channel.")
                                }),
                        )
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .bg(theme::panel())
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(10.))
                    .text_color(theme::muted())
                    .child(if loading {
                        t("Chargement…", "Loading…").to_string()
                    } else {
                        format!(
                            "{} {} · {} {}",
                            filtered.len(),
                            t("salons", "channels"),
                            total_users,
                            t("utilisateurs", "users")
                        )
                    })
                    .child(t(
                        "Double-clic pour rejoindre · Entrée pour le premier résultat",
                        "Double-click to join · Enter for the first result",
                    )),
            )
    }
}
