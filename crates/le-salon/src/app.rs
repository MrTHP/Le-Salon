//! Vue racine : barre supérieure, arborescence réseau/salons, journal,
//! liste des pseudos, saisie et barre d'état.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::Local;
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Div, Entity, FocusHandle, Focusable,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
    Pixels, Render, ScrollHandle, Subscription, Task, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, div, prelude::*, px, rgb, rgba, size, svg,
};

use ircv3_core::{
    APP_NAME, Config, ConnectionConfig, Lang,
    client::{self, NetworkHandle},
    formatting, irc_fold, media,
    model::{
        AppModel, BufferId, BufferKind, ChatLine, ConnectionStatus, DccStatus, LineKind, NetworkId,
        User,
    },
    upload,
};

use crate::{
    channel_list::ChannelListWindow,
    chat,
    connect_form::{ConnectForm, ConnectFormEvent},
    emoji, i18n,
    menu::{MenuAction, MenuItem, PopupMenu},
    preferences::{Preferences, PreferencesWindow},
    schedule::ScheduleStore,
    text_input::{TextInput, TextInputEvent},
    theme, tooltip,
    tray::{self, TrayCommand, TrayController, TrayReceiver},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeSide {
    Sidebar,
    NickList,
}

#[derive(Clone, Copy, Debug)]
struct ResizeDrag {
    side: ResizeSide,
    start_x: gpui::Pixels,
    start_width: gpui::Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TopMenu {
    File,
    Network,
    Channel,
    View,
    Help,
}

actions!(
    ircv3_gpui,
    [
        AppQuit,
        NewNetwork,
        ClearBuffer,
        FocusComposer,
        OpenThemes,
        OpenEmoji,
        OpenPreferences,
        TabNext,
        TabPrev,
        GoToBuffer1,
        GoToBuffer2,
        GoToBuffer3,
        GoToBuffer4,
        GoToBuffer5,
        GoToBuffer6,
        GoToBuffer7,
        GoToBuffer8,
        GoToBuffer9,
    ]
);

pub fn install_bindings(cx: &mut App) {
    let mut bindings = crate::text_input::bind_keys();
    bindings.extend([
        KeyBinding::new("secondary-q", AppQuit, None),
        KeyBinding::new("secondary-n", NewNetwork, None),
        KeyBinding::new("secondary-l", ClearBuffer, None),
        KeyBinding::new("secondary-f", FocusComposer, None),
        KeyBinding::new("secondary-t", OpenThemes, None),
        KeyBinding::new("secondary-e", OpenEmoji, None),
        KeyBinding::new("secondary-p", OpenPreferences, None),
        KeyBinding::new("tab", TabNext, Some("FormInput")),
        KeyBinding::new("shift-tab", TabPrev, Some("FormInput")),
        KeyBinding::new("alt-1", GoToBuffer1, None),
        KeyBinding::new("alt-2", GoToBuffer2, None),
        KeyBinding::new("alt-3", GoToBuffer3, None),
        KeyBinding::new("alt-4", GoToBuffer4, None),
        KeyBinding::new("alt-5", GoToBuffer5, None),
        KeyBinding::new("alt-6", GoToBuffer6, None),
        KeyBinding::new("alt-7", GoToBuffer7, None),
        KeyBinding::new("alt-8", GoToBuffer8, None),
        KeyBinding::new("alt-9", GoToBuffer9, None),
    ]);
    cx.bind_keys(bindings);
}

#[derive(Clone)]
struct SidebarBuffer {
    id: BufferId,
    kind: BufferKind,
    name: String,
    unread: Option<String>,
    highlight: bool,
    active: bool,
    joined: bool,
}

#[derive(Clone)]
struct SidebarNetwork {
    id: NetworkId,
    name: String,
    host: String,
    status: ConnectionStatus,
    buffers: Vec<SidebarBuffer>,
}

struct BufferView {
    id: BufferId,
    network_id: NetworkId,
    kind: BufferKind,
    name: String,
    topic: String,
    modes: String,
    joined: bool,
    users: Vec<User>,
    lines: Vec<ChatLine>,
    typing: Vec<String>,
    version: u64,
}

struct NetworkView {
    status: ConnectionStatus,
    status_text: String,
    tls: bool,
    away: bool,
}

struct CompletionState {
    word_start: usize,
    word_len: usize,
    matches: Vec<String>,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasswordPolicy {
    /// Mémoriser le mot de passe du réseau courant.
    Save,
    /// Oublier le mot de passe du réseau courant.
    Clear,
    /// Ne rien changer au trousseau (connexion CLI ponctuelle).
    Keep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Persist {
    /// Enregistrer aussi les réglages de sécurité (formulaire GUI).
    All,
    /// Ne pas persister TLS/vérification/mot de passe (CLI ponctuelle).
    Session,
}

struct PendingUpload {
    path: PathBuf,
    target: String,
    network_id: NetworkId,
    size: u64,
    filename: String,
}

struct Toast {
    id: u64,
    title: String,
    body: String,
    expires_at: Instant,
}

/// Nettoie le texte affiché dans une notification : retire les codes mIRC,
/// les contrôles et les caractères de direction Unicode.
fn clean_notification_text(text: &str) -> String {
    formatting::parse(text).text
}

/// Décide si un nouveau message mérite une notification.
fn notification_relevant(kind: BufferKind, line: &ChatLine, mentions_only: bool) -> bool {
    if kind == BufferKind::Server || line.self_nick {
        return false;
    }
    let is_query = kind == BufferKind::Query;
    let is_mention = line.mention;
    let is_message = matches!(line.kind, LineKind::Message | LineKind::Action);
    if mentions_only {
        is_query || is_mention
    } else {
        is_query || is_mention || is_message
    }
}

pub struct AppRoot {
    model: Arc<Mutex<AppModel>>,
    networks: HashMap<NetworkId, NetworkHandle>,
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    input: Entity<TextInput>,
    _input_sub: Option<Subscription>,
    form: Option<Entity<ConnectForm>>,
    form_sub: Option<Subscription>,
    scroll_handles: HashMap<BufferId, ScrollHandle>,
    seen_versions: HashMap<BufferId, u64>,
    last_active: Option<BufferId>,
    drafts: HashMap<BufferId, String>,
    status_message: Option<(String, Instant)>,
    focus_handle: FocusHandle,
    window_handle: gpui::AnyWindowHandle,
    config: Config,
    popup: Option<PopupMenu>,
    theme_picker_open: bool,
    emoji_open: bool,
    focus_emoji_search: bool,
    dcc_open: bool,
    schedule_open: bool,
    pending_upload: Option<PendingUpload>,
    toasts: Vec<Toast>,
    next_toast_id: u64,
    preferences_window: Option<gpui::AnyWindowHandle>,
    emoji_search: Entity<TextInput>,
    _emoji_sub: Option<Subscription>,
    about_open: bool,
    rename_open: bool,
    rename_input: Entity<TextInput>,
    rename_network: Option<NetworkId>,
    focus_rename_input: bool,
    _rename_sub: Option<Subscription>,
    schedule: ScheduleStore,
    window_active: bool,
    notifications_ready: bool,
    notify_seen: HashMap<BufferId, u64>,
    tray: Option<TrayController>,
    tray_rx: TrayReceiver,
    raise_window: bool,
    minimize_window: bool,
    wake_task: Task<()>,
    tick_task: Task<()>,
    completion: Option<CompletionState>,
    last_typing: Option<Instant>,
    should_focus_input: bool,
    sidebar_width: Pixels,
    nick_width: Pixels,
    resize_drag: Option<ResizeDrag>,
    quitting: bool,
}

impl AppRoot {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        initial_connection: Option<ConnectionConfig>,
    ) -> Self {
        let mut config = Config::load();
        let lang = Lang::from_config(&config.language);
        i18n::set_lang(lang);
        let theme_key = std::env::var("LE_SALON_THEME").unwrap_or_else(|_| config.theme.clone());
        let theme_index = theme::set_theme_key(&theme_key);
        config.theme = theme::themes()[theme_index].key.to_string();
        let model = Arc::new(Mutex::new(AppModel::new(config.clone())));
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let input = cx.new(|cx| {
            TextInput::new(
                cx,
                i18n::t(
                    "Écrivez un message… (/aide pour les commandes)",
                    "Type a message… (/help for commands)",
                ),
                "",
                false,
            )
            .with_completion()
        });
        let input_sub = cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| {
            this.on_input_event(event, cx);
        });
        let form = cx.new(|cx| ConnectForm::new(cx, &config, false));
        let form_sub = cx.subscribe(&form, |this, _, event: &ConnectFormEvent, cx| {
            this.on_form_event(event, cx);
        });
        let emoji_search = cx.new(|cx| {
            TextInput::new(
                cx,
                i18n::t("Rechercher un emoji…", "Search emoji…"),
                "",
                false,
            )
        });
        let emoji_sub =
            cx.subscribe(
                &emoji_search,
                |this, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Changed(_) => cx.notify(),
                    TextInputEvent::Escape => {
                        this.emoji_open = false;
                        this.should_focus_input = true;
                        cx.notify();
                    }
                    _ => {}
                },
            );
        let rename_input =
            cx.new(|cx| TextInput::new(cx, i18n::t("Nouveau pseudo", "New nickname"), "", false));
        let rename_sub =
            cx.subscribe(
                &rename_input,
                |this, _, event: &TextInputEvent, cx| match event {
                    TextInputEvent::Submitted(text) => this.confirm_rename(text.clone(), cx),
                    TextInputEvent::Escape => {
                        this.rename_open = false;
                        this.should_focus_input = true;
                        cx.notify();
                    }
                    TextInputEvent::Changed(_) => cx.notify(),
                    _ => {}
                },
            );
        let schedule = ScheduleStore::load();
        let (tray, tray_rx) = tray::start();
        let sidebar_width = px(config.sidebar_width.clamp(150.0, 460.0));
        let nick_width = px(config.nick_width.clamp(120.0, 360.0));

        let mut root = Self {
            model,
            networks: HashMap::new(),
            wake_tx,
            wake_rx,
            input,
            _input_sub: Some(input_sub),
            form: Some(form),
            form_sub: Some(form_sub),
            scroll_handles: HashMap::new(),
            seen_versions: HashMap::new(),
            last_active: None,
            drafts: HashMap::new(),
            status_message: None,
            focus_handle: cx.focus_handle(),
            window_handle: window.window_handle(),
            config,
            popup: None,
            theme_picker_open: false,
            emoji_open: false,
            focus_emoji_search: false,
            dcc_open: false,
            schedule_open: false,
            pending_upload: None,
            toasts: Vec::new(),
            next_toast_id: 1,
            preferences_window: None,
            emoji_search,
            _emoji_sub: Some(emoji_sub),
            about_open: false,
            rename_open: false,
            rename_input,
            rename_network: None,
            focus_rename_input: false,
            _rename_sub: Some(rename_sub),
            schedule,
            window_active: true,
            notifications_ready: false,
            notify_seen: HashMap::new(),
            tray,
            tray_rx,
            raise_window: false,
            minimize_window: false,
            wake_task: Task::ready(()),
            tick_task: Task::ready(()),
            completion: None,
            last_typing: None,
            should_focus_input: false,
            sidebar_width,
            nick_width,
            resize_drag: None,
            quitting: false,
        };
        root.start_tasks(cx);
        // Commutateurs de diagnostic pour les captures d'écran.
        if std::env::var_os("LE_SALON_THEMES").is_some() {
            root.theme_picker_open = true;
        }
        if std::env::var_os("LE_SALON_EMOJI").is_some() {
            root.emoji_open = true;
        }
        if std::env::var_os("LE_SALON_DCC").is_some() {
            root.dcc_open = true;
        }
        if std::env::var_os("LE_SALON_SCHEDULE").is_some() {
            root.schedule_open = true;
        }
        if std::env::var_os("LE_SALON_ABOUT").is_some() {
            root.about_open = true;
        }
        if std::env::var_os("LE_SALON_LIST").is_some() {
            let delayed = cx.spawn(async move |this, cx| {
                gpui::Timer::after(Duration::from_millis(700)).await;
                let _ = this.update(cx, |this, cx| this.open_channel_list(None, cx));
            });
            delayed.detach();
        }
        if std::env::var_os("LE_SALON_PREFERENCES").is_some() {
            let delayed = cx.spawn(async move |this, cx| {
                gpui::Timer::after(Duration::from_millis(700)).await;
                let _ = this.update(cx, |this, cx| this.open_preferences(cx));
            });
            delayed.detach();
        }
        if std::env::var_os("LE_SALON_MENU").is_some() {
            let items = vec![
                MenuItem::new("Ouvrir une conversation", MenuAction::Query("alice".into())),
                MenuItem::new("Whois", MenuAction::Whois("alice".into())),
                MenuItem::new("Copier le pseudo", MenuAction::Copy("alice".into())),
                MenuItem::new("Expulser du salon", MenuAction::Kick("alice".into()))
                    .danger()
                    .separator(),
                MenuItem::new(
                    "Envoyer un fichier (DCC)",
                    MenuAction::DccSend("alice".into()),
                ),
            ];
            root.popup = Some(PopupMenu::new(px(360.), px(240.), items));
        }
        if let Some(connection) = initial_connection {
            // Les flags CLI (`--no-tls`, `--insecure`, `--sasl-password`) sont
            // strictement limités à cette session : ils ne doivent jamais
            // modifier la configuration persistante.
            root.connect(connection, PasswordPolicy::Keep, Persist::Session, cx);
        }
        root
    }

    fn start_tasks(&mut self, cx: &mut Context<Self>) {
        let wake_rx = self.wake_rx.clone();
        self.wake_task = cx.spawn(async move |this, cx| {
            while wake_rx.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        self.tick_task = cx.spawn(async move |this, cx| {
            loop {
                gpui::Timer::after(Duration::from_secs(1)).await;
                if this
                    .update(cx, |this, cx| {
                        this.expire_status();
                        this.process_schedules(cx);
                        this.check_notifications(cx);
                        this.expire_toasts();
                        this.process_tray(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    fn expire_status(&mut self) {
        if self
            .status_message
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(8))
        {
            self.status_message = None;
        }
    }

    fn set_status(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.status_message = Some((message.into(), Instant::now()));
        cx.notify();
    }

    fn on_input_event(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Submitted(text) => self.send_input(text.clone(), cx),
            TextInputEvent::Tab => self.complete(cx),
            TextInputEvent::Escape => {
                self.input.update(cx, |input, cx| input.clear(cx));
                self.completion = None;
            }
            TextInputEvent::Changed(text) => self.maybe_send_typing(text),
        }
    }

    fn on_form_event(&mut self, event: &ConnectFormEvent, cx: &mut Context<Self>) {
        match event {
            ConnectFormEvent::Submit {
                connection,
                remember_password,
            } => {
                self.config.remember_sasl_password = *remember_password;
                let password = if *remember_password {
                    PasswordPolicy::Save
                } else {
                    PasswordPolicy::Clear
                };
                self.connect(connection.clone(), password, Persist::All, cx)
            }
            ConnectFormEvent::Cancel => self.close_form(cx),
        }
    }

    fn connect(
        &mut self,
        connection: ConnectionConfig,
        password_policy: PasswordPolicy,
        persist: Persist,
        cx: &mut Context<Self>,
    ) {
        let connection = connection.normalized();
        if let Err(error) = connection.security_check() {
            self.set_status(error, cx);
            self.should_focus_input = true;
            return;
        }

        // On mémorise l'ancien état de sécurité avant `remember` : une
        // connexion CLI ne doit pas laisser TLS ou la vérification dans un
        // état dangereux pour les lancements suivants.
        let previous_tls = self.config.tls;
        let previous_verify = self.config.verify_tls;
        self.config.remember(&connection);
        if persist == Persist::Session {
            self.config.tls = previous_tls;
            self.config.verify_tls = previous_verify;
        }

        match password_policy {
            PasswordPolicy::Save => {
                // Si le champ est vide, on ne doit surtout pas conserver le
                // mot de passe d'un autre réseau.
                self.config.sasl_password = connection.sasl_password.clone().unwrap_or_default();
            }
            PasswordPolicy::Clear => self.config.sasl_password.clear(),
            PasswordPolicy::Keep => {
                if persist == Persist::Session {
                    // Ne jamais garder le mot de passe CLI en mémoire dans la
                    // configuration : un changement de thème ou de langue
                    // déclenche un `save()` qui le mémoriserait. On recharge
                    // seulement le secret déjà stocké pour ce réseau, s'il
                    // existe.
                    let account = connection.sasl_account.clone().unwrap_or_default();
                    self.config.sasl_password =
                        ircv3_core::secrets::get(&connection.host, &account)
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                }
            }
        }

        let saved = if persist == Persist::Session {
            // Ne jamais écrire le mot de passe CLI, ni écraser le secret du
            // réseau dans le trousseau.
            self.config.save_without_secret()
        } else {
            self.config.save()
        };
        if let Err(error) = saved {
            log::warn!("sauvegarde de la configuration impossible : {error}");
        }
        let network_id = {
            let mut model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.config = self.config.clone();
            model.add_network(connection.clone())
        };
        let handle = client::spawn(
            network_id,
            connection.clone(),
            self.model.clone(),
            self.wake_tx.clone(),
        );
        self.networks.insert(network_id, handle);
        self.close_form(cx);
        self.should_focus_input = true;
        self.set_status(
            format!(
                "{} {}…",
                i18n::t("Connexion à", "Connecting to"),
                connection.endpoint()
            ),
            cx,
        );
    }

    fn open_preferences(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.preferences_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.preferences_window = None;
        }

        let prefs = Preferences::from_config(&self.config);
        let (tx, rx) = async_channel::bounded(4);
        let bounds = Bounds::centered(None, size(px(780.), px(760.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("{} — Le Salon", i18n::t("Préférences", "Preferences")).into()),
                ..Default::default()
            }),
            app_id: Some("le-salon-preferences".into()),
            window_min_size: Some(size(px(640.), px(560.))),
            ..Default::default()
        };
        match cx.open_window(options, move |window, cx| {
            cx.new(|cx| PreferencesWindow::new(window, cx, prefs, tx))
        }) {
            Ok(handle) => {
                self.preferences_window = Some(handle.into());
                cx.spawn(async move |this, cx| {
                    while let Ok(prefs) = rx.recv().await {
                        let _ = this.update(cx, |this, cx| {
                            this.apply_preferences(prefs, cx);
                        });
                    }
                })
                .detach();
            }
            Err(error) => {
                log::warn!("impossible d'ouvrir les préférences : {error}");
                self.set_status(
                    i18n::t(
                        "Impossible d'ouvrir la fenêtre des préférences.",
                        "Could not open the preferences window.",
                    ),
                    cx,
                );
            }
        }
    }

    fn apply_preferences(&mut self, prefs: Preferences, cx: &mut Context<Self>) {
        // La langue est appliquée en premier pour que les libellés des
        // autres réglages soient déjà dans la bonne langue.
        let lang = Lang::from_config(&prefs.language);
        self.set_language(lang, cx);
        prefs.apply_to(&mut self.config);
        theme::set_theme_key(&self.config.theme);
        if !self.config.in_app_toasts {
            self.toasts.clear();
        }
        if let Err(error) = self.config.save() {
            log::warn!("sauvegarde des préférences impossible : {error}");
        }
        {
            let mut model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.config = self.config.clone();
        }
        self.set_status(
            i18n::t("Préférences enregistrées.", "Preferences saved."),
            cx,
        );
        cx.notify();
    }

    fn open_form(&mut self, cx: &mut Context<Self>) {
        let allow_cancel = !self.networks.is_empty();
        let config = self.config.clone();
        let form = cx.new(|cx| ConnectForm::new(cx, &config, allow_cancel));
        let subscription = cx.subscribe(&form, |this, _, event: &ConnectFormEvent, cx| {
            this.on_form_event(event, cx);
        });
        self.form_sub = Some(subscription);
        self.form = Some(form);
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form_sub.take();
        self.form.take();
        self.should_focus_input = true;
        cx.notify();
    }

    fn send_input(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim_end().to_string();
        if text.is_empty() {
            return;
        }
        if self.handle_special_command(&text, cx) {
            return;
        }
        let (network_id, buffer_id) = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            let active = model.active_buffer();
            (
                active.map(|buffer| buffer.network),
                active.map(|buffer| buffer.id),
            )
        };
        if let Some(network_id) = network_id {
            if let Some(handle) = self.networks.get(&network_id) {
                handle.send_input(text);
            } else {
                self.set_status(
                    i18n::t(
                        "Ce réseau n'est plus disponible.",
                        "This network is no longer available.",
                    ),
                    cx,
                );
            }
        } else {
            self.set_status(i18n::t("Aucun réseau actif.", "No active network."), cx);
        }
        if let Some(buffer_id) = buffer_id {
            self.drafts.remove(&buffer_id);
        }
        self.last_typing = None;
        self.completion = None;
        self.input.update(cx, |input, cx| input.clear(cx));
        self.should_focus_input = true;
        cx.notify();
    }

    fn send_command(&mut self, command: impl Into<String>, cx: &mut Context<Self>) {
        let command = command.into();
        let network_id = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.active_buffer().map(|buffer| buffer.network)
        };
        if let Some(network_id) = network_id {
            if let Some(handle) = self.networks.get(&network_id) {
                handle.send_input(command);
            }
        } else {
            self.set_status(i18n::t("Aucun réseau actif.", "No active network."), cx);
        }
    }

    fn switch_buffer(&mut self, id: BufferId, cx: &mut Context<Self>) {
        if self.last_active == Some(id) {
            self.should_focus_input = true;
            cx.notify();
            return;
        }
        if let Some(old) = self.last_active {
            let text = self.input.read(cx).content().to_string();
            self.drafts.insert(old, text);
        }
        {
            let mut model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.set_active(id);
        }
        let draft = self.drafts.remove(&id).unwrap_or_default();
        self.input.update(cx, |input, cx| input.set_text(draft, cx));
        self.last_active = Some(id);
        self.completion = None;
        self.should_focus_input = true;
        cx.notify();
    }

    fn switch_to_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let id = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            ordered_buffers(&model).get(index).copied()
        };
        if let Some(id) = id {
            self.switch_buffer(id, cx);
        }
    }

    fn begin_join(&mut self, _network_id: NetworkId, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_text("/join ", cx));
        self.should_focus_input = true;
        cx.notify();
    }

    fn nick_clicked(
        &mut self,
        nick: String,
        click_count: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if click_count >= 2 {
            self.send_command(format!("/query {nick}"), cx);
        } else {
            self.input
                .update(cx, |input, cx| input.insert_text(&format!("{nick}: "), cx));
            self.should_focus_input = true;
            cx.notify();
        }
    }

    fn maybe_send_typing(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self
            .last_typing
            .is_some_and(|last| last.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        let target = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.active_buffer().and_then(|buffer| {
                (buffer.kind == BufferKind::Channel && buffer.joined)
                    .then(|| (buffer.network, buffer.name.clone()))
            })
        };
        if let Some((network_id, name)) = target
            && let Some(handle) = self.networks.get(&network_id)
        {
            handle.send_raw(format!("@+typing=active TAGMSG {name}"));
            self.last_typing = Some(Instant::now());
        }
    }

    fn complete(&mut self, cx: &mut Context<Self>) {
        let content = self.input.read(cx).content().to_string();
        if content.is_empty() {
            return;
        }
        let trimmed = content.trim_end();
        let word_start = trimmed
            .rfind(char::is_whitespace)
            .map(|index| {
                index
                    + trimmed[index..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(1)
            })
            .unwrap_or(0);
        let word = &trimmed[word_start..];
        if word.is_empty() {
            return;
        }
        let (lead, search) = if let Some(rest) = word.strip_prefix('@') {
            ("@", rest)
        } else if let Some(rest) = word.strip_prefix('+') {
            ("+", rest)
        } else {
            ("", word)
        };
        let searching_channels = search.starts_with('#');
        let candidates = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            let Some(active) = model.active_buffer() else {
                return;
            };
            let network_id = active.network;
            let mut values: Vec<String> = if searching_channels {
                model
                    .network(network_id)
                    .map(|network| {
                        network
                            .buffers
                            .iter()
                            .filter_map(|id| model.buffer(*id))
                            .filter(|buffer| buffer.kind == BufferKind::Channel)
                            .map(|buffer| buffer.name.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                active.users.iter().map(|user| user.nick.clone()).collect()
            };
            let needle = irc_fold(search);
            values.retain(|value| irc_fold(value).starts_with(&needle));
            values.sort_by_key(|a| irc_fold(a));
            values.dedup_by(|a, b| irc_fold(a) == irc_fold(b));
            values
        };
        if candidates.is_empty() {
            self.set_status(i18n::t("Aucune correspondance.", "No match."), cx);
            return;
        }

        let suffix = if word_start == 0 && !searching_channels {
            ": "
        } else {
            ""
        };
        let (index, matches) = match self.completion.take() {
            Some(state)
                if state.word_start == word_start
                    && state.word_len == word.len()
                    && state.matches == candidates =>
            {
                let index = (state.index + 1) % candidates.len();
                (index, candidates)
            }
            _ => (0, candidates),
        };
        let replacement = format!("{lead}{}{}", matches[index], suffix);
        let word_end = word_start + word.len();
        let new_content = format!(
            "{}{}{}",
            &content[..word_start],
            replacement,
            &content[word_end..]
        );
        self.input
            .update(cx, |input, cx| input.set_text(new_content, cx));
        self.completion = Some(CompletionState {
            word_start,
            word_len: replacement.len(),
            matches: matches.clone(),
            index,
        });
        if matches.len() > 1 {
            self.set_status(
                format!(
                    "{} {} — {}",
                    matches.len(),
                    i18n::t("correspondances", "matches"),
                    i18n::t("Tab pour cycler", "Tab to cycle")
                ),
                cx,
            );
        } else {
            self.set_status(
                format!("{} {}", i18n::t("Complété :", "Completed:"), matches[index]),
                cx,
            );
        }
        self.should_focus_input = true;
    }

    fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle);
        self.should_focus_input = false;
    }

    fn open_rename_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open_rename(cx);
    }

    fn open_rename(&mut self, cx: &mut Context<Self>) {
        let network = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model
                .active_network()
                .map(|network| (network.id, network.nick.clone()))
        };
        let Some((network_id, nick)) = network else {
            self.set_status(i18n::t("Aucun réseau actif.", "No active network."), cx);
            return;
        };
        self.rename_network = Some(network_id);
        self.rename_input
            .update(cx, |input, cx| input.set_text(nick, cx));
        self.rename_open = true;
        self.focus_rename_input = true;
        self.should_focus_input = false;
        self.popup = None;
        cx.notify();
    }

    fn confirm_rename(&mut self, text: String, cx: &mut Context<Self>) {
        let nick = text.trim().to_string();
        if nick.is_empty() || nick.contains(char::is_whitespace) {
            self.set_status(
                i18n::t(
                    "Pseudo invalide : pas d'espace.",
                    "Invalid nickname: no spaces.",
                ),
                cx,
            );
            self.focus_rename_input = true;
            cx.notify();
            return;
        }
        if let Some(network_id) = self.rename_network {
            if let Some(handle) = self.networks.get(&network_id) {
                handle.send_input(format!("/nick {nick}"));
                self.set_status(
                    format!(
                        "{} {nick}",
                        i18n::t(
                            "Changement de pseudo demandé :",
                            "Nickname change requested:"
                        )
                    ),
                    cx,
                );
            }
        }
        self.rename_open = false;
        self.rename_network = None;
        self.should_focus_input = true;
        cx.notify();
    }

    fn close_rename(&mut self, cx: &mut Context<Self>) {
        self.rename_open = false;
        self.rename_network = None;
        self.should_focus_input = true;
        cx.notify();
    }

    fn render_upload_confirm(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(pending) = &self.pending_upload else {
            return div();
        };
        let filename = pending.filename.clone();
        let size = upload::format_size(pending.size);
        let target = pending.target.clone();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.pending_upload = None;
                    this.should_focus_input = true;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(560.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_6()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(i18n::t(
                                "Téléverser vers un service public ?",
                                "Upload to a public service?",
                            )),
                    )
                    .child(
                        div()
                            .rounded_md()
                            .bg(theme::error_alpha(0.09))
                            .border_1()
                            .border_color(theme::error_alpha(0.33))
                            .px_3()
                            .py_2()
                            .text_size(px(12.5))
                            .text_color(theme::red())
                            .child(i18n::t(
                                "Le fichier sera envoyé à litterbox.catbox.moe, conservé 72 h et accessible à toute personne ayant le lien. N'envoie jamais de fichier sensible (clé SSH, mot de passe, document privé).",
                                "The file will be sent to litterbox.catbox.moe, kept for 72 hours and accessible to anyone with the link. Never upload sensitive files (SSH keys, passwords, private documents).",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(12.5))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(filename),
                            )
                            .child(
                                div()
                                    .text_color(theme::muted())
                                    .child(format!(
                                        "{size} · {} {target}",
                                        i18n::t("vers", "to")
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
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
                                        cx.listener(|this, _, _, cx| {
                                            this.pending_upload = None;
                                            this.should_focus_input = true;
                                            cx.notify();
                                        }),
                                    )
                                    .child(i18n::t("Annuler", "Cancel")),
                            )
                            .child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(theme::error_alpha(0.20))
                                    .text_color(theme::red())
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::error_alpha(0.30)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            if let Some(pending) = this.pending_upload.take() {
                                                this.confirm_upload(pending, cx);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(i18n::t("Téléverser", "Upload")),
                            ),
                    ),
            )
    }

    fn render_about_dialog(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.about_open = false;
                this.should_focus_input = true;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(520.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_6()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(
                                div()
                                    .size(px(56.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_lg()
                                    .bg(theme::accent())
                                    .text_size(px(28.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(theme::on_accent())
                                    .child("#"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .child(APP_NAME),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme::accent())
                                            .child(format!("version {}", ircv3_core::VERSION)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme::muted())
                            .child(i18n::t(
                                "Client IRCv3 natif écrit en Rust, interface GPU dessinée par GPUI (le framework UI de Zed).",
                                "Native IRCv3 client written in Rust, GPU interface drawn by GPUI (Zed's UI framework).",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(11.5))
                            .text_color(theme::muted())
                            .child("• IRCv3.2, CAP 302, SASL PLAIN, TLS rustls")
                            .child(if i18n::is_fr() {
                                "• DCC SEND, upload litterbox, messages planifiés"
                            } else {
                                "• DCC SEND, litterbox upload, scheduled messages"
                            })
                            .child(if i18n::is_fr() {
                                "• 19 thèmes, toasts, info-bulles FR/EN, notifications et tray Linux"
                            } else {
                                "• 19 themes, toasts, FR/EN tooltips, notifications and Linux tray"
                            })
                            .child(if i18n::is_fr() {
                                "• ircv3-core partagé, tests unitaires et bout en bout"
                            } else {
                                "• shared ircv3-core, unit and end-to-end tests"
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme::faint())
                                    .child(i18n::t(
                                        "Licence MIT ou Apache-2.0 — ircv3-asm contributors",
                                        "MIT or Apache-2.0 license — ircv3-asm contributors",
                                    )),
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
                                        cx.listener(|this, _, _, cx| {
                                            this.about_open = false;
                                            this.should_focus_input = true;
                                            cx.notify();
                                        }),
                                    )
                                    .child(i18n::t("Fermer", "Close")),
                            ),
                    ),
            )
    }

    fn render_rename_dialog(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_rename(cx)),
            )
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(i18n::t("Changer de pseudo", "Change nickname")),
                    )
                    .child(self.rename_input.clone())
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::muted())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::panel_alt()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.close_rename(cx)),
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
                                        cx.listener(|this, _, _, cx| {
                                            let nick =
                                                this.rename_input.read(cx).content().to_string();
                                            this.confirm_rename(nick, cx)
                                        }),
                                    )
                                    .child(i18n::t("Renommer", "Rename")),
                            ),
                    ),
            )
    }

    fn begin_resize(&mut self, side: ResizeSide, start_x: Pixels, cx: &mut Context<Self>) {
        self.resize_drag = Some(ResizeDrag {
            side,
            start_x,
            start_width: match side {
                ResizeSide::Sidebar => self.sidebar_width,
                ResizeSide::NickList => self.nick_width,
            },
        });
        cx.notify();
    }

    fn resize_handle(&self, side: ResizeSide, cx: &Context<Self>) -> Div {
        div()
            .w(px(5.))
            .h_full()
            .flex_none()
            .cursor(gpui::CursorStyle::ResizeColumn)
            .hover(|style| style.bg(theme::accent()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.begin_resize(side, event.position.x, cx)
                }),
            )
            .child(div().w(px(1.)).h_full().bg(theme::border()))
    }

    fn on_root_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.resize_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.resize_drag = None;
            return;
        }
        let delta = f32::from(event.position.x - drag.start_x);
        match drag.side {
            ResizeSide::Sidebar => {
                let max = (f32::from(window.viewport_size().width) * 0.45).clamp(260.0, 460.0);
                self.sidebar_width = px((f32::from(drag.start_width) + delta).clamp(150.0, max));
            }
            ResizeSide::NickList => {
                let max = (f32::from(window.viewport_size().width) * 0.40).clamp(180.0, 360.0);
                self.nick_width = px((f32::from(drag.start_width) - delta).clamp(110.0, max));
            }
        }
        cx.notify();
    }

    fn on_root_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.resize_drag.take().is_some() {
            self.config.sidebar_width = f32::from(self.sidebar_width);
            self.config.nick_width = f32::from(self.nick_width);
            if let Err(error) = self.config.save() {
                log::warn!("sauvegarde des largeurs impossible : {error}");
            }
            cx.notify();
        }
    }

    fn themes_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.theme_picker_open = true;
        self.should_focus_input = false;
        self.popup = None;
        cx.notify();
    }

    fn language_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_language(cx);
    }

    fn upload_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open_upload_picker(cx);
    }

    fn emoji_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = true;
        self.focus_emoji_search = true;
        self.should_focus_input = false;
        cx.notify();
    }

    fn schedule_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.schedule_open = true;
        self.should_focus_input = false;
        cx.notify();
    }

    fn dcc_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.dcc_open = true;
        self.should_focus_input = false;
        cx.notify();
    }

    fn jitsi_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.start_jitsi(None, cx);
    }

    fn composer_icon(
        &self,
        id: &'static str,
        content: impl IntoElement,
        title: &'static str,
        detail: &'static str,
        action: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .text_size(px(14.))
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|style| style.bg(theme::panel_alt()).text_color(theme::text()))
            .tooltip(move |_, cx| tooltip::simple(cx, title, detail))
            .on_mouse_down(MouseButton::Left, cx.listener(action))
            .child(content)
            .into_any_element()
    }

    pub fn hide_to_tray(&mut self, window: &mut Window, _cx: &mut App) -> bool {
        log::debug!("hide_to_tray called, tray={}", self.tray.is_some());
        if self.quitting {
            return true;
        }
        if self.tray.is_some() {
            window.minimize_window();
            false
        } else {
            true
        }
    }

    fn app_quit(&mut self, _: &AppQuit, _: &mut Window, cx: &mut Context<Self>) {
        self.quitting = true;
        for handle in self.networks.values() {
            handle.shutdown();
        }
        cx.quit();
    }

    fn new_network_action(&mut self, _: &NewNetwork, _: &mut Window, cx: &mut Context<Self>) {
        self.open_form(cx);
    }

    fn clear_buffer_action(&mut self, _: &ClearBuffer, _: &mut Window, cx: &mut Context<Self>) {
        let id = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.active
        };
        if let Some(id) = id {
            self.model
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear_buffer(id);
            cx.notify();
        }
    }

    fn focus_composer_action(
        &mut self,
        _: &FocusComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_input(window, cx);
    }

    fn open_themes_action(&mut self, _: &OpenThemes, _: &mut Window, cx: &mut Context<Self>) {
        self.theme_picker_open = true;
        self.should_focus_input = false;
        self.popup = None;
        cx.notify();
    }

    fn open_emoji_action(&mut self, _: &OpenEmoji, _: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = true;
        self.focus_emoji_search = true;
        self.should_focus_input = false;
        cx.notify();
    }

    fn open_preferences_action(
        &mut self,
        _: &OpenPreferences,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_preferences(cx);
    }

    fn tab_next(&mut self, _: &TabNext, window: &mut Window, _: &mut Context<Self>) {
        window.focus_next();
    }

    fn tab_prev(&mut self, _: &TabPrev, window: &mut Window, _: &mut Context<Self>) {
        window.focus_prev();
    }

    fn goto1(&mut self, _: &GoToBuffer1, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(0, cx);
    }
    fn goto2(&mut self, _: &GoToBuffer2, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(1, cx);
    }
    fn goto3(&mut self, _: &GoToBuffer3, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(2, cx);
    }
    fn goto4(&mut self, _: &GoToBuffer4, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(3, cx);
    }
    fn goto5(&mut self, _: &GoToBuffer5, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(4, cx);
    }
    fn goto6(&mut self, _: &GoToBuffer6, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(5, cx);
    }
    fn goto7(&mut self, _: &GoToBuffer7, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(6, cx);
    }
    fn goto8(&mut self, _: &GoToBuffer8, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(7, cx);
    }
    fn goto9(&mut self, _: &GoToBuffer9, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_index(8, cx);
    }

    fn help_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.send_command("/help", cx);
    }

    fn open_form_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open_form(cx);
    }

    fn send_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).content().to_string();
        self.send_input(text, cx);
    }

    fn new_network_click(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open_form(cx);
    }

    fn topbar(&mut self, cx: &mut Context<Self>) -> Div {
        let (network, status, unread) = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            let network = model.active_network();
            (
                network
                    .map(|network| network.display_name().to_string())
                    .unwrap_or_else(|| i18n::t("hors ligne", "offline").to_string()),
                network
                    .map(|network| network.status)
                    .unwrap_or(ConnectionStatus::Disconnected),
                model.total_unread(),
            )
        };
        div()
            .h(px(46.))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .id("topbar-logo")
                    .size(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(theme::accent())
                    .text_color(theme::on_accent())
                    .font_weight(gpui::FontWeight::BOLD)
                    .tooltip(|_, cx| {
                        tooltip::simple(
                            cx,
                            APP_NAME,
                            i18n::t(
                                "Client IRCv3 natif — Rust + GPUI",
                                "Native IRCv3 client — Rust + GPUI",
                            ),
                        )
                    })
                    .child("#"),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme::text())
                    .child(APP_NAME),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(theme::border()))
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(theme::status_color(status)),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme::muted())
                    .child(network),
            )
            .child(self.top_menu_button(i18n::t("Fichier", "File"), TopMenu::File, cx))
            .child(self.top_menu_button(i18n::t("Réseau", "Network"), TopMenu::Network, cx))
            .child(self.top_menu_button(i18n::t("Salon", "Channel"), TopMenu::Channel, cx))
            .child(self.top_menu_button(i18n::t("Affichage", "View"), TopMenu::View, cx))
            .child(self.top_menu_button(i18n::t("Aide", "Help"), TopMenu::Help, cx))
            .child(div().flex_1())
            .when(unread > 0, |this| {
                this.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(theme::accent())
                        .text_size(px(10.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme::on_accent())
                        .child(format!("{unread} {}", i18n::t("non lus", "unread"))),
                )
            })
            .child(self.toolbar_button(
                "toolbar-theme",
                "◐",
                i18n::t("Thème", "Theme"),
                i18n::t("Changer de thème (Ctrl+T)", "Change theme (Ctrl+T)"),
                Self::themes_click,
                cx,
            ))
            .child(self.toolbar_button(
                "toolbar-language",
                i18n::t("FR", "EN"),
                i18n::t("Langue", "Language"),
                i18n::t(
                    "Basculer entre le français et l'anglais",
                    "Switch between French and English",
                ),
                Self::language_click,
                cx,
            ))
            .child(self.toolbar_button(
                "toolbar-network",
                i18n::t("＋ Réseau", "＋ Network"),
                i18n::t("Nouvelle connexion", "New connection"),
                i18n::t(
                    "Ajouter un réseau IRC (Ctrl+N)",
                    "Add an IRC network (Ctrl+N)",
                ),
                Self::new_network_click,
                cx,
            ))
            .child(self.toolbar_button(
                "toolbar-help",
                i18n::t("Aide", "Help"),
                i18n::t("Aide", "Help"),
                i18n::t("Commandes et raccourcis", "Commands and shortcuts"),
                Self::help_click,
                cx,
            ))
    }

    fn toolbar_button(
        &self,
        id: &'static str,
        label: &'static str,
        title: &'static str,
        detail: &'static str,
        action: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme::border())
            .text_size(px(11.5))
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|style| style.bg(theme::panel_alt()).text_color(theme::text()))
            .tooltip(move |_, cx| tooltip::simple(cx, title, detail))
            .on_mouse_down(MouseButton::Left, cx.listener(action))
            .child(label)
            .into_any_element()
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let networks: Vec<SidebarNetwork> = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model
                .networks
                .iter()
                .map(|network| {
                    let buffers = ordered_buffers_for(&model, network.id)
                        .into_iter()
                        .filter_map(|id| {
                            model.buffer(id).map(|buffer| SidebarBuffer {
                                id: buffer.id,
                                kind: buffer.kind,
                                name: buffer.name.clone(),
                                unread: buffer.unread_label(),
                                highlight: buffer.highlight,
                                active: model.active == Some(buffer.id),
                                joined: buffer.joined,
                            })
                        })
                        .collect();
                    SidebarNetwork {
                        id: network.id,
                        name: network.display_name().to_string(),
                        host: network.host.clone(),
                        status: network.status,
                        buffers,
                    }
                })
                .collect()
        };

        let mut sidebar = div()
            .w(self.sidebar_width)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::border())
            .id("sidebar")
            .overflow_y_scroll();

        for network in networks {
            let network_id = network.id;
            let mut block = div().flex().flex_col().gap_1().px_2().pt_3().pb_2().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.open_network_menu(network_id, event.position, cx)
                        }),
                    )
                    .child(
                        div()
                            .size(px(7.))
                            .flex_none()
                            .rounded_full()
                            .bg(theme::status_color(network.status)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(network.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(theme::faint())
                            .child(network.host.clone()),
                    )
                    .child(
                        div()
                            .id(("sidebar-join", network_id))
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::panel_alt()))
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    i18n::t("Rejoindre un salon", "Join a channel"),
                                    i18n::t("Ouvrir la saisie du salon", "Open the channel prompt"),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| this.begin_join(network_id, cx)),
                            )
                            .child("+"),
                    )
                    .child(
                        div()
                            .id(("sidebar-reconnect", network_id))
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::panel_alt()))
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    i18n::t("Reconnecter", "Reconnect"),
                                    i18n::t(
                                        "Relancer la connexion au réseau",
                                        "Reconnect to the network",
                                    ),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(handle) = this.networks.get(&network_id) {
                                        handle.reconnect();
                                    }
                                    this.set_status(
                                        i18n::t("Reconnexion demandée.", "Reconnect requested."),
                                        cx,
                                    );
                                }),
                            )
                            .child("↻"),
                    )
                    .child(
                        div()
                            .id(("sidebar-disconnect", network_id))
                            .flex_none()
                            .px_1()
                            .rounded_sm()
                            .text_size(px(11.))
                            .text_color(theme::faint())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::panel_alt()))
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    i18n::t("Déconnecter", "Disconnect"),
                                    i18n::t("Fermer la connexion", "Close the connection"),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(handle) = this.networks.get(&network_id) {
                                        handle.disconnect();
                                    }
                                    this.set_status(
                                        i18n::t("Déconnexion demandée.", "Disconnect requested."),
                                        cx,
                                    );
                                }),
                            )
                            .child("×"),
                    ),
            );

            for buffer in network.buffers {
                let id = buffer.id;
                let kind = buffer.kind;
                let row_name = buffer.name.clone();
                let row_joined = buffer.joined;
                let muted = !buffer.joined && buffer.kind == BufferKind::Channel;
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mx_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(buffer.active, |this| {
                        this.bg(theme::accent_dark())
                            .border_l_2()
                            .border_color(theme::accent())
                    })
                    .hover(|style| {
                        if buffer.active {
                            style
                        } else {
                            style.bg(theme::panel_alt())
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.switch_buffer(id, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| match kind {
                            BufferKind::Server => {
                                this.open_network_menu(network_id, event.position, cx)
                            }
                            BufferKind::Query => {
                                this.open_query_menu(id, row_name.clone(), event.position, cx)
                            }
                            BufferKind::Channel => this.open_channel_menu(
                                id,
                                row_name.clone(),
                                row_joined,
                                event.position,
                                cx,
                            ),
                        }),
                    )
                    .child(
                        div()
                            .w(px(13.))
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(if buffer.kind == BufferKind::Channel {
                                theme::faint()
                            } else {
                                theme::purple()
                            })
                            .child(buffer.kind.icon()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(if muted { theme::faint() } else { theme::text() })
                            .child(buffer.name),
                    );
                if buffer.highlight {
                    row = row.child(div().size(px(6.)).rounded_full().bg(theme::red()));
                }
                if let Some(unread) = buffer.unread {
                    row = row.child(
                        div()
                            .px_1()
                            .rounded_full()
                            .bg(theme::accent())
                            .text_size(px(9.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::on_accent())
                            .child(unread),
                    );
                }
                block = block.child(row);
            }
            sidebar = sidebar
                .child(block)
                .child(div().mx_3().h(px(1.)).bg(theme::border()));
        }

        sidebar
            .child(
                div().p_2().child(
                    div()
                        .id("sidebar-new-network")
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::border())
                        .text_size(px(11.5))
                        .text_color(theme::muted())
                        .cursor_pointer()
                        .hover(|style| style.bg(theme::panel_alt()).text_color(theme::text()))
                        .tooltip(|_, cx| {
                            tooltip::simple(
                                cx,
                                i18n::t("Nouvelle connexion", "New connection"),
                                i18n::t(
                                    "Ouvrir le formulaire de connexion IRC",
                                    "Open the IRC connection form",
                                ),
                            )
                        })
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::new_network_click))
                        .child(i18n::t("＋ Nouveau réseau", "＋ New network")),
                ),
            )
            .into_any_element()
    }

    fn main_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let data = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            let Some(buffer) = model.active_buffer() else {
                return div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::bg())
                    .text_color(theme::muted())
                    .child(i18n::t("Aucun tampon sélectionné", "No buffer selected"));
            };
            let network = model.network(buffer.network);
            let view = BufferView {
                id: buffer.id,
                network_id: buffer.network,
                kind: buffer.kind,
                name: buffer.name.clone(),
                topic: buffer.topic.clone(),
                modes: buffer.modes.clone(),
                joined: buffer.joined,
                users: buffer.users_sorted(),
                lines: buffer
                    .messages
                    .iter()
                    .rev()
                    .take(800)
                    .rev()
                    .cloned()
                    .collect(),
                typing: buffer.typing_users(),
                version: buffer.version,
            };
            let network_view = NetworkView {
                status: network
                    .map(|network| network.status)
                    .unwrap_or(ConnectionStatus::Disconnected),
                status_text: network
                    .map(|network| network.status_text.clone())
                    .unwrap_or_default(),
                tls: network.is_some_and(|network| network.tls),
                away: network.is_some_and(|network| network.away),
            };
            (view, network_view)
        };
        let (view, network) = data;

        let header = self.buffer_header(&view, &network, cx);
        let log = self.chat_log(&view, window, cx);
        let body = if view.kind == BufferKind::Channel && !view.users.is_empty() {
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(log)
                .child(self.resize_handle(ResizeSide::NickList, cx))
                .child(self.nick_list(&view, cx))
        } else {
            div().flex().flex_1().min_h_0().child(log)
        };

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .child(header)
            .child(body)
            .child(self.composer(&view, cx))
    }

    fn buffer_header(
        &self,
        view: &BufferView,
        network: &NetworkView,
        cx: &mut Context<Self>,
    ) -> Div {
        let network_id = view.network_id;
        let name = view.name.clone();
        let mut header = div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .flex_none()
            .bg(theme::panel())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_size(px(17.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(view.name.clone()),
            )
            .when(!view.modes.is_empty(), |this| {
                this.child(
                    div()
                        .id(("header-modes", view.id))
                        .px_1()
                        .rounded_sm()
                        .bg(theme::panel_alt())
                        .text_size(px(10.))
                        .text_color(theme::muted())
                        .tooltip(|_, cx| {
                            tooltip::simple(
                                cx,
                                i18n::t("Modes du salon", "Channel modes"),
                                i18n::t("Droits et réglages actifs", "Active rights and settings"),
                            )
                        })
                        .child(format!("+{}", view.modes)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child(if view.topic.is_empty() {
                        i18n::t("Aucun sujet", "No topic").to_string()
                    } else {
                        view.topic.clone()
                    }),
            );

        if view.kind == BufferKind::Channel {
            header = header.child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(theme::faint())
                    .child(format!(
                        "{} {}",
                        view.users.len(),
                        i18n::t("pseudos", "nicks")
                    )),
            );
            let name_part = name.clone();
            if view.joined {
                header = header.child(self.small_button(
                    i18n::t("Partir", "Leave"),
                    move |this, _, _, cx| {
                        this.send_command(format!("/part {name_part}"), cx);
                    },
                    cx,
                ));
            } else {
                let name_join = name.clone();
                header = header.child(self.small_button(
                    i18n::t("Rejoindre", "Join"),
                    move |this, _, _, cx| {
                        this.send_command(format!("/join {name_join}"), cx);
                    },
                    cx,
                ));
            }
            header = header.child(self.small_button(
                i18n::t("Fermer", "Close"),
                move |this, _, _, cx| this.send_command("/close", cx),
                cx,
            ));
        } else if view.kind == BufferKind::Query {
            header = header.child(self.small_button(
                i18n::t("Fermer", "Close"),
                move |this, _, _, cx| this.send_command("/close", cx),
                cx,
            ));
        } else {
            header = header
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .bg(theme::status_color(network.status)),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme::faint())
                        .child(network.status_text.clone()),
                )
                .when(network.tls, |this| {
                    this.child(
                        div()
                            .id(("header-tls", view.id))
                            .px_2()
                            .rounded_full()
                            .bg(theme::panel_alt())
                            .text_size(px(9.5))
                            .text_color(theme::green())
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    i18n::t("Connexion chiffrée", "Encrypted connection"),
                                    i18n::t("TLS actif", "TLS active"),
                                )
                            })
                            .child("TLS"),
                    )
                });
            if network.status == ConnectionStatus::Connected {
                header = header.child(self.small_button(
                    i18n::t("Déconnecter", "Disconnect"),
                    move |this, _, _, cx| {
                        if let Some(handle) = this.networks.get(&network_id) {
                            handle.disconnect();
                        }
                        this.set_status(
                            i18n::t("Déconnexion demandée.", "Disconnect requested."),
                            cx,
                        );
                    },
                    cx,
                ));
            } else {
                header = header.child(self.small_button(
                    i18n::t("Reconnecter", "Reconnect"),
                    move |this, _, _, cx| {
                        if let Some(handle) = this.networks.get(&network_id) {
                            handle.reconnect();
                        }
                        this.set_status(
                            i18n::t("Reconnexion demandée.", "Reconnect requested."),
                            cx,
                        );
                    },
                    cx,
                ));
            }
        }
        if network.away {
            header = header.child(
                div()
                    .id(("header-away", view.id))
                    .px_2()
                    .rounded_full()
                    .bg(theme::panel_alt())
                    .text_size(px(10.))
                    .text_color(theme::orange())
                    .tooltip(|_, cx| {
                        tooltip::simple(
                            cx,
                            i18n::t("Vous êtes absent", "You are away"),
                            i18n::t("Utilisez /away pour revenir", "Use /away to come back"),
                        )
                    })
                    .child(i18n::t("absent", "away")),
            );
        }
        let menu_id = view.id;
        let menu_network = view.network_id;
        let menu_kind = view.kind;
        let menu_name = view.name.clone();
        let menu_joined = view.joined;
        header = header.child(
            div()
                .id(("buffer-menu", view.id))
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(theme::border())
                .text_size(px(10.5))
                .text_color(theme::muted())
                .cursor_pointer()
                .hover(|style| style.bg(theme::panel_alt()).text_color(theme::text()))
                .tooltip(|_, cx| {
                    tooltip::simple(
                        cx,
                        i18n::t("Options du tampon", "Buffer options"),
                        i18n::t("Sujet, modes, noms, fermer…", "Topic, modes, names, close…"),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| match menu_kind {
                        BufferKind::Server => {
                            this.open_network_menu(menu_network, event.position, cx)
                        }
                        BufferKind::Query => {
                            this.open_query_menu(menu_id, menu_name.clone(), event.position, cx)
                        }
                        BufferKind::Channel => this.open_channel_menu(
                            menu_id,
                            menu_name.clone(),
                            menu_joined,
                            event.position,
                            cx,
                        ),
                    }),
                )
                .child("⋯"),
        );
        header
    }

    fn small_button(
        &self,
        label: &'static str,
        action: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(theme::border())
            .text_size(px(10.5))
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|style| style.bg(theme::panel_alt()).text_color(theme::text()))
            .on_mouse_down(MouseButton::Left, cx.listener(action))
            .child(label)
    }

    fn chat_log(
        &mut self,
        view: &BufferView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scroll = self.scroll_handles.entry(view.id).or_default().clone();
        let switched = self.last_active != Some(view.id);
        let version_changed =
            self.seen_versions.insert(view.id, view.version) != Some(view.version);
        let at_bottom = scroll.offset().y <= -scroll.max_offset().height + px(32.);
        if version_changed && (at_bottom || switched) {
            scroll.scroll_to_bottom();
        }
        self.last_active = Some(view.id);

        let font = window.text_style().font();
        let users: HashMap<String, User> = view
            .users
            .iter()
            .map(|user| (irc_fold(&user.nick), user.clone()))
            .collect();
        let mut children: Vec<AnyElement> = Vec::new();
        let today = Local::now().date_naive();
        let mut last_date = None;
        for (index, line) in view.lines.iter().enumerate() {
            let date = line.at.date_naive();
            if last_date != Some(date) {
                let label = if date == today {
                    i18n::t("Aujourd'hui", "Today").to_string()
                } else {
                    date.format("%d/%m/%Y").to_string()
                };
                children.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .child(div().flex_1().h(px(1.)).bg(theme::border()))
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(theme::faint())
                                .child(label),
                        )
                        .child(div().flex_1().h(px(1.)).bg(theme::border()))
                        .into_any_element(),
                );
                last_date = Some(date);
            }
            children.push(chat::line_row(line, &font, &users, index));
            if let Some(link) = media::find_media(&line.text) {
                let url = link.url.clone();
                let label = match link.kind {
                    media::MediaKind::Audio => i18n::t("Écouter", "Listen"),
                    media::MediaKind::Image => i18n::t("Voir l'image", "View image"),
                    _ => i18n::t("Ouvrir la vidéo", "Open video"),
                };
                children.push(
                    div()
                        .pl(px(75.))
                        .py_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .bg(theme::panel_alt())
                                .border_1()
                                .border_color(theme::border())
                                .text_size(px(10.5))
                                .text_color(theme::accent())
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::accent_dark()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_, _, _, cx| cx.open_url(&url)),
                                )
                                .child(format!("{} {} · {}", link.kind.icon(), label, link.host)),
                        )
                        .into_any_element(),
                );
            }
        }
        if view.lines.is_empty() {
            children.push(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .py_8()
                    .text_size(px(12.))
                    .text_color(theme::faint())
                    .child(match view.kind {
                        BufferKind::Server => i18n::t("Connexion en cours…", "Connecting…"),
                        BufferKind::Channel => i18n::t(
                            "Aucun message dans ce salon.",
                            "No message in this channel.",
                        ),
                        BufferKind::Query => i18n::t(
                            "Aucun message dans cette conversation.",
                            "No message in this conversation.",
                        ),
                    })
                    .into_any_element(),
            );
        }
        if !view.typing.is_empty() {
            children.push(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(10.5))
                    .italic()
                    .text_color(theme::muted())
                    .child(format!(
                        "{} {}",
                        view.typing.join(", "),
                        i18n::t("est en train d'écrire…", "is typing…")
                    ))
                    .into_any_element(),
            );
        }

        div()
            .id(("chat-log", view.id))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.open_log_menu(event.position, cx)
                }),
            )
            .child(div().flex().flex_col().gap_1().py_2().children(children))
            .into_any_element()
    }

    fn nick_list(&self, view: &BufferView, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div()
            .w(self.nick_width)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(theme::panel())
            .border_l_1()
            .border_color(theme::border())
            .id(("nick-list", view.id))
            .overflow_y_scroll()
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .text_size(px(9.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::faint())
                    .child(format!(
                        "{} — {}",
                        i18n::t("PSEUDOS", "NICKS"),
                        view.users.len()
                    )),
            );

        for (index, user) in view.users.iter().enumerate() {
            let nick = user.nick.clone();
            let nick_right = user.nick.clone();
            let profile = user.clone();
            let prefix = user
                .best_prefix()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let display = user.nick.clone();
            let away = user.away;
            let account = user.account.clone();
            list = list.child(
                div()
                    .id(("nick", index))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .py(px(1.))
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::panel_alt()))
                    .tooltip(move |_, cx| tooltip::user_card(cx, &profile, true))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.nick_clicked(nick.clone(), event.click_count, window, cx)
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.open_nick_menu(nick_right.clone(), event.position, cx)
                        }),
                    )
                    .child(
                        div()
                            .w(px(11.))
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme::accent())
                            .child(prefix),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme::nick_color(&display))
                            .child(display),
                    )
                    .when(away, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(px(9.))
                                .text_color(theme::faint())
                                .child("◌"),
                        )
                    })
                    .when_some(account, |this, account| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(px(8.5))
                                .text_color(theme::faint())
                                .child(account),
                        )
                    }),
            );
        }
        list.into_any_element()
    }

    fn composer(&self, view: &BufferView, cx: &mut Context<Self>) -> Div {
        let nick = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model
                .active_network()
                .map(|network| network.nick.clone())
                .unwrap_or_default()
        };
        let nick = if nick.is_empty() {
            "?".to_string()
        } else {
            nick
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(theme::panel())
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .id("composer-nick")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::panel_alt()))
                    .tooltip(|_, cx| {
                        tooltip::simple(
                            cx,
                            i18n::t("Changer de pseudo", "Change nickname"),
                            i18n::t(
                                "Choisir un nouveau pseudo pour ce réseau",
                                "Choose a new nickname for this network",
                            ),
                        )
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::open_rename_click))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::accent())
                            .child(nick),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::faint())
                            .child("✎"),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(140.))
                    .truncate()
                    .text_size(px(10.5))
                    .text_color(theme::muted())
                    .child(format!("{} ›", view.name)),
            )
            .child(self.composer_icon(
                "composer-upload",
                "⇧",
                i18n::t("Téléverser un fichier", "Upload a file"),
                i18n::t(
                    "litterbox · public 72 h · 50 Mo max",
                    "litterbox · public 72 h · 50 MB max",
                ),
                Self::upload_click,
                cx,
            ))
            .child(self.composer_icon(
                "composer-emoji",
                "☺",
                i18n::t("Emoji", "Emoji"),
                i18n::t("Insérer un emoji", "Insert an emoji"),
                Self::emoji_click,
                cx,
            ))
            .child(self.composer_icon(
                "composer-schedule",
                "◷",
                i18n::t("Message planifié", "Scheduled message"),
                i18n::t("Choisir une heure d'envoi", "Choose a send time"),
                Self::schedule_click,
                cx,
            ))
            .child(
                self.composer_icon(
                    "composer-jitsi",
                    svg()
                        .path("icons/camera.svg")
                        .size(px(16.))
                        .text_color(theme::muted()),
                    i18n::t("Appel Jitsi", "Jitsi call"),
                    i18n::t(
                        "Ouvrir un appel vidéo dans le navigateur",
                        "Open a video call in the browser",
                    ),
                    Self::jitsi_click,
                    cx,
                ),
            )
            .child(self.composer_icon(
                "composer-dcc",
                "⇅",
                i18n::t("Transfert DCC", "DCC transfer"),
                i18n::t("Envoyer un fichier directement", "Send a file directly"),
                Self::dcc_click,
                cx,
            ))
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .child(
                div()
                    .id("composer-send")
                    .flex_none()
                    .px_4()
                    .py_2()
                    .rounded_md()
                    .bg(theme::accent())
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::on_accent())
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::accent_hover()))
                    .tooltip(|_, cx| {
                        tooltip::simple(
                            cx,
                            i18n::t("Envoyer", "Send"),
                            i18n::t("Envoyer le message (Entrée)", "Send the message (Enter)"),
                        )
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::send_click))
                    .child(i18n::t("Envoyer", "Send")),
            )
    }

    fn statusbar(&mut self, cx: &mut Context<Self>) -> Div {
        let (status, text, endpoint, tls, lag, caps, message) = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            let network = model.active_network();
            (
                network
                    .map(|network| network.status)
                    .unwrap_or(ConnectionStatus::Disconnected),
                network
                    .map(|network| network.status_text.clone())
                    .unwrap_or_else(|| i18n::t("Aucune connexion", "No connection").to_string()),
                network
                    .map(|network| network.endpoint())
                    .unwrap_or_default(),
                network.is_some_and(|network| network.tls),
                network.and_then(|network| network.lag_ms),
                network.map(|network| network.caps.len()).unwrap_or(0),
                self.status_message
                    .as_ref()
                    .filter(|(_, at)| at.elapsed() < Duration::from_secs(8))
                    .map(|(message, _)| message.clone()),
            )
        };
        let _ = cx;
        let left = message.unwrap_or_else(|| {
            if endpoint.is_empty() {
                text.clone()
            } else {
                format!(
                    "{} · {}{} · {} · {} caps",
                    text,
                    endpoint,
                    if tls { " (TLS)" } else { "" },
                    lag.map(|lag| format!("{lag} ms"))
                        .unwrap_or_else(|| "—".to_string()),
                    caps
                )
            }
        });
        div()
            .h(px(27.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3()
            .bg(theme::panel_alt())
            .border_t_1()
            .border_color(theme::border())
            .text_size(px(10.5))
            .text_color(theme::muted())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(theme::status_color(status)),
                    )
                    .child(div().truncate().child(left)),
            )
            .child(div().flex_none().text_color(theme::faint()).child(i18n::t(
                "Tab compléter · ↑ historique · /aide",
                "Tab complete · ↑ history · /help",
            )))
    }

    fn overlay(&self, form: Entity<ConnectForm>) -> Div {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .child(form)
    }

    fn welcome(&mut self, cx: &mut Context<Self>) -> Div {
        let mut page = div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(theme::bg())
            .text_color(theme::text())
            .child(
                div()
                    .id("welcome-logo")
                    .size(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(theme::accent())
                    .text_size(px(36.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme::on_accent())
                    .shadow_lg()
                    .tooltip(|_, cx| {
                        tooltip::simple(
                            cx,
                            APP_NAME,
                            i18n::t(
                                "Client IRCv3 natif — Rust + GPUI",
                                "Native IRCv3 client — Rust + GPUI",
                            ),
                        )
                    })
                    .child("#"),
            )
            .child(
                div()
                    .text_size(px(30.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(APP_NAME),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(i18n::t(
                        "Client IRCv3 moderne — Rust + GPUI, TLS rustls et SASL PLAIN",
                        "Modern IRCv3 client — Rust + GPUI, rustls TLS and SASL PLAIN",
                    )),
            )
            .child(
                div().flex().gap_2().children(
                    ["IRCv3.2", "CAP 302", "SASL PLAIN", "TLS rustls", "GPU"]
                        .into_iter()
                        .map(|label| {
                            div()
                                .px_2()
                                .py_1()
                                .rounded_full()
                                .bg(theme::panel_alt())
                                .border_1()
                                .border_color(theme::border())
                                .text_size(px(10.))
                                .text_color(theme::muted())
                                .child(label)
                        }),
                ),
            );
        if let Some(form) = self.form.clone() {
            page = page.child(form);
        } else {
            page = page
                .child(
                    div()
                        .id("welcome-connect")
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
                                    "Ouvrir le formulaire de connexion IRC",
                                    "Open the IRC connection form",
                                ),
                            )
                        })
                        .on_mouse_down(MouseButton::Left, cx.listener(Self::open_form_click))
                        .child(i18n::t("Se connecter", "Connect")),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme::faint())
                        .child(i18n::t(
                            "Fedora · Linux · macOS — AppImage universelle",
                            "Fedora · Linux · macOS — universal AppImage",
                        )),
                );
        }
        page
    }

    fn active_context(&self) -> Option<(NetworkId, BufferId, BufferKind, String)> {
        let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
        let buffer = model.active_buffer()?;
        Some((buffer.network, buffer.id, buffer.kind, buffer.name.clone()))
    }

    fn set_language(&mut self, lang: Lang, cx: &mut Context<Self>) {
        i18n::set_lang(lang);
        self.config.language = lang.code().to_string();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(
                i18n::t(
                    "Écrivez un message… (/aide pour les commandes)",
                    "Type a message… (/help for commands)",
                ),
                cx,
            )
        });
        self.emoji_search.update(cx, |input, cx| {
            input.set_placeholder(i18n::t("Rechercher un emoji…", "Search emoji…"), cx)
        });
        if let Err(error) = self.config.save() {
            log::warn!("sauvegarde de la langue impossible : {error}");
        }
        if let Some(tray) = &self.tray {
            tray.set_language(lang);
        }
        let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
        model.save_config();
        drop(model);
        self.popup = None;
        cx.notify();
    }

    fn toggle_language(&mut self, cx: &mut Context<Self>) {
        self.set_language(i18n::lang().other(), cx);
    }

    fn apply_theme(&mut self, index: usize, cx: &mut Context<Self>) {
        theme::set_theme(index);
        self.config.theme = theme::current().key.to_string();
        if let Err(error) = self.config.save() {
            log::warn!("sauvegarde du thème impossible : {error}");
        }
        self.theme_picker_open = false;
        self.popup = None;
        cx.notify();
    }

    fn open_popup(
        &mut self,
        x: gpui::Pixels,
        y: gpui::Pixels,
        items: Vec<MenuItem>,
        cx: &mut Context<Self>,
    ) {
        self.popup = Some(PopupMenu::new(x, y, items));
        cx.notify();
    }

    fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.popup = None;
        cx.notify();
    }

    fn top_menu_button(
        &self,
        label: &'static str,
        menu: TopMenu,
        cx: &Context<Self>,
    ) -> AnyElement {
        let detail = match menu {
            TopMenu::File => i18n::t(
                "Connexion, préférences, fichiers, planification, quitter",
                "Connection, preferences, files, scheduling, quit",
            ),
            TopMenu::Network => i18n::t(
                "Reconnecter, déconnecter, rejoindre un salon",
                "Reconnect, disconnect, join a channel",
            ),
            TopMenu::Channel => i18n::t(
                "Sujet, modes, noms, liste des salons",
                "Topic, modes, names, channel list",
            ),
            TopMenu::View => i18n::t(
                "Thèmes, emojis, appel Jitsi, langue",
                "Themes, emojis, Jitsi call, language",
            ),
            TopMenu::Help => i18n::t(
                "Commandes, raccourcis et à propos",
                "Commands, shortcuts and about",
            ),
        };
        div()
            .id(("top-menu", menu as usize))
            .px_2()
            .py_1()
            .rounded_sm()
            .text_size(px(11.5))
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|style| style.bg(theme::panel_alt()).text_color(theme::text()))
            .tooltip(move |_, cx| tooltip::simple(cx, label, detail))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_top_menu(menu, event.position, cx)
                }),
            )
            .child(label)
            .into_any_element()
    }

    fn open_top_menu(
        &mut self,
        menu: TopMenu,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let (network_id, buffer_id, kind, name) = match self.active_context() {
            Some(value) => value,
            None => (0, 0, BufferKind::Server, String::new()),
        };
        let items = match menu {
            TopMenu::File => vec![
                MenuItem::new(
                    i18n::t("Nouveau réseau…", "New network…"),
                    MenuAction::NewNetwork,
                )
                .shortcut("Ctrl+N"),
                MenuItem::new(
                    i18n::t("Préférences…", "Preferences…"),
                    MenuAction::Preferences,
                )
                .shortcut("Ctrl+P"),
                MenuItem::new(
                    i18n::t("Téléverser un fichier…", "Upload a file…"),
                    MenuAction::Upload,
                ),
                MenuItem::new(
                    i18n::t("Messages planifiés…", "Scheduled messages…"),
                    MenuAction::Schedule,
                ),
                MenuItem::new(
                    i18n::t("Transferts DCC…", "DCC transfers…"),
                    MenuAction::DccPanel,
                ),
                MenuItem::new(
                    i18n::t("Réduire dans la barre système", "Minimize to tray"),
                    MenuAction::MinimizeToTray,
                ),
                MenuItem::new(i18n::t("Quitter", "Quit"), MenuAction::Quit)
                    .danger()
                    .separator(),
            ],
            TopMenu::Network => vec![
                MenuItem::new(
                    i18n::t("Reconnecter", "Reconnect"),
                    MenuAction::Reconnect(network_id),
                ),
                MenuItem::new(
                    i18n::t("Déconnecter", "Disconnect"),
                    MenuAction::Disconnect(network_id),
                ),
                MenuItem::new(
                    i18n::t("Rejoindre un salon…", "Join a channel…"),
                    MenuAction::JoinPrompt,
                )
                .separator(),
                MenuItem::new(
                    i18n::t("Liste des salons", "Channel list"),
                    MenuAction::List,
                ),
                MenuItem::new(
                    i18n::t("Notifications", "Notifications"),
                    MenuAction::ToggleNotifications,
                )
                .checked(self.config.notifications),
            ],
            TopMenu::Channel => {
                let mut items = Vec::new();
                if kind == BufferKind::Channel {
                    let joined = {
                        let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
                        model.buffer(buffer_id).is_some_and(|buffer| buffer.joined)
                    };
                    if joined {
                        items.push(MenuItem::new(
                            i18n::t("Partir", "Leave"),
                            MenuAction::Part(name.clone()),
                        ));
                    } else {
                        items.push(MenuItem::new(
                            i18n::t("Rejoindre", "Join"),
                            MenuAction::Join(name.clone()),
                        ));
                    }
                    items.push(MenuItem::new(
                        i18n::t("Sujet…", "Topic…"),
                        MenuAction::Topic(buffer_id),
                    ));
                    items.push(MenuItem::new(
                        i18n::t("Noms", "Names"),
                        MenuAction::Names(Some(name.clone())),
                    ));
                    items.push(MenuItem::new(
                        i18n::t("Who", "Who"),
                        MenuAction::Who(Some(name.clone())),
                    ));
                }
                items.push(
                    MenuItem::new(
                        i18n::t("Liste des salons…", "Channel list…"),
                        MenuAction::List,
                    )
                    .separator(),
                );
                if kind != BufferKind::Server {
                    items.push(
                        MenuItem::new(
                            i18n::t("Effacer le tampon", "Clear buffer"),
                            MenuAction::ClearBuffer(buffer_id),
                        )
                        .separator(),
                    );
                    items.push(MenuItem::new(
                        i18n::t("Fermer", "Close"),
                        MenuAction::CloseBuffer(buffer_id),
                    ));
                } else {
                    items.push(
                        MenuItem::new(
                            i18n::t("Rejoindre un salon…", "Join a channel…"),
                            MenuAction::JoinPrompt,
                        )
                        .separator(),
                    );
                    items.push(MenuItem::new(
                        i18n::t("Nouveau réseau…", "New network…"),
                        MenuAction::NewNetwork,
                    ));
                }
                items
            }
            TopMenu::View => vec![
                MenuItem::new(i18n::t("Thèmes…", "Themes…"), MenuAction::Themes),
                MenuItem::new(i18n::t("Emojis…", "Emojis…"), MenuAction::Emoji),
                MenuItem::new(i18n::t("Appel Jitsi", "Jitsi call"), MenuAction::Jitsi),
                MenuItem::new(
                    if i18n::is_fr() {
                        "English"
                    } else {
                        "Français"
                    },
                    MenuAction::SetLanguage(i18n::lang().other().code()),
                )
                .separator(),
            ],
            TopMenu::Help => vec![
                MenuItem::new(
                    i18n::t("Commandes et raccourcis", "Commands and shortcuts"),
                    MenuAction::Help,
                ),
                MenuItem::new(i18n::t("À propos", "About"), MenuAction::About),
            ],
        };
        self.open_popup(position.x, position.y, items, cx);
    }

    fn open_nick_menu(
        &mut self,
        nick: String,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let channel = self
            .active_context()
            .filter(|(_, _, kind, _)| *kind == BufferKind::Channel)
            .map(|(_, _, _, name)| name);
        let mut items = vec![
            MenuItem::new(
                i18n::t("Ouvrir une conversation", "Open a conversation"),
                MenuAction::Query(nick.clone()),
            ),
            MenuItem::new(
                i18n::t("Mentionner", "Mention"),
                MenuAction::Mention(nick.clone()),
            ),
            MenuItem::new(i18n::t("Whois", "Whois"), MenuAction::Whois(nick.clone())),
            MenuItem::new(
                i18n::t("Copier le pseudo", "Copy nickname"),
                MenuAction::Copy(nick.clone()),
            ),
            MenuItem::new(
                i18n::t("Surveiller (MONITOR)", "Monitor"),
                MenuAction::Monitor(nick.clone()),
            )
            .separator(),
        ];
        if let Some(channel) = &channel {
            items.push(MenuItem::new(
                i18n::t("Donner le statut op", "Give op"),
                MenuAction::Op(nick.clone()),
            ));
            items.push(MenuItem::new(
                i18n::t("Retirer le statut op", "Remove op"),
                MenuAction::Deop(nick.clone()),
            ));
            items.push(MenuItem::new(
                i18n::t("Donner la voix", "Give voice"),
                MenuAction::Voice(nick.clone()),
            ));
            items.push(MenuItem::new(
                i18n::t("Retirer la voix", "Remove voice"),
                MenuAction::Devoice(nick.clone()),
            ));
            items.push(
                MenuItem::new(
                    i18n::t("Expulser du salon", "Kick from channel"),
                    MenuAction::Kick(nick.clone()),
                )
                .danger(),
            );
            items.push(
                MenuItem::new(i18n::t("Bannir", "Ban"), MenuAction::Ban(nick.clone())).danger(),
            );
            items.push(MenuItem::new(
                i18n::t("Inviter dans le salon", "Invite to channel"),
                MenuAction::Invite(nick.clone()),
            ));
            let _ = channel;
        }
        items.push(
            MenuItem::new(
                i18n::t("Envoyer un fichier (DCC)", "Send a file (DCC)"),
                MenuAction::DccSend(nick),
            )
            .separator(),
        );
        self.open_popup(position.x, position.y, items, cx);
    }

    fn open_channel_menu(
        &mut self,
        buffer_id: BufferId,
        name: String,
        joined: bool,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            if joined {
                MenuItem::new(
                    i18n::t("Partir du salon", "Leave channel"),
                    MenuAction::Part(name.clone()),
                )
            } else {
                MenuItem::new(
                    i18n::t("Rejoindre le salon", "Join channel"),
                    MenuAction::Join(name.clone()),
                )
            },
            MenuItem::new(i18n::t("Sujet…", "Topic…"), MenuAction::Topic(buffer_id)),
            MenuItem::new(
                i18n::t("Liste des noms", "Names"),
                MenuAction::Names(Some(name.clone())),
            ),
            MenuItem::new(i18n::t("Who", "Who"), MenuAction::Who(Some(name.clone()))),
            MenuItem::new(
                i18n::t("Effacer le tampon", "Clear buffer"),
                MenuAction::ClearBuffer(buffer_id),
            )
            .separator(),
            MenuItem::new(
                i18n::t("Fermer", "Close"),
                MenuAction::CloseBuffer(buffer_id),
            ),
        ];
        self.open_popup(position.x, position.y, items, cx);
    }

    fn open_query_menu(
        &mut self,
        buffer_id: BufferId,
        nick: String,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            MenuItem::new(i18n::t("Whois", "Whois"), MenuAction::Whois(nick.clone())),
            MenuItem::new(
                i18n::t("Surveiller (MONITOR)", "Monitor"),
                MenuAction::Monitor(nick),
            )
            .separator(),
            MenuItem::new(
                i18n::t("Effacer le tampon", "Clear buffer"),
                MenuAction::ClearBuffer(buffer_id),
            ),
            MenuItem::new(
                i18n::t("Fermer", "Close"),
                MenuAction::CloseBuffer(buffer_id),
            ),
        ];
        self.open_popup(position.x, position.y, items, cx);
    }

    fn open_network_menu(
        &mut self,
        network_id: NetworkId,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            MenuItem::new(
                i18n::t("Reconnecter", "Reconnect"),
                MenuAction::Reconnect(network_id),
            ),
            MenuItem::new(
                i18n::t("Déconnecter", "Disconnect"),
                MenuAction::Disconnect(network_id),
            ),
            MenuItem::new(
                i18n::t("Rejoindre un salon…", "Join a channel…"),
                MenuAction::JoinPrompt,
            )
            .separator(),
            MenuItem::new(
                i18n::t("Liste des salons", "Channel list"),
                MenuAction::List,
            ),
            MenuItem::new(
                i18n::t("Notifications", "Notifications"),
                MenuAction::ToggleNotifications,
            )
            .checked(self.config.notifications),
            MenuItem::new(
                i18n::t("Nouveau réseau…", "New network…"),
                MenuAction::NewNetwork,
            )
            .separator(),
        ];
        self.open_popup(position.x, position.y, items, cx);
    }

    fn open_log_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        if let Some((_, buffer_id, kind, name)) = self.active_context() {
            if kind == BufferKind::Channel {
                self.open_channel_menu(buffer_id, name, true, position, cx);
            } else {
                let items = vec![
                    MenuItem::new(
                        i18n::t("Effacer le tampon", "Clear buffer"),
                        MenuAction::ClearBuffer(buffer_id),
                    ),
                    MenuItem::new(
                        i18n::t("Fermer", "Close"),
                        MenuAction::CloseBuffer(buffer_id),
                    ),
                ];
                self.open_popup(position.x, position.y, items, cx);
            }
        }
    }

    fn run_menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        match action {
            MenuAction::NewNetwork => self.open_form(cx),
            MenuAction::Preferences => self.open_preferences(cx),
            MenuAction::Themes => {
                self.theme_picker_open = true;
                cx.notify();
            }
            MenuAction::SetLanguage(code) => {
                let lang = if code == "en" { Lang::En } else { Lang::Fr };
                self.set_language(lang, cx);
            }
            MenuAction::ToggleNotifications => self.toggle_notifications(cx),
            MenuAction::Upload => self.open_upload_picker(cx),
            MenuAction::Schedule => {
                self.schedule_open = true;
                cx.notify();
            }
            MenuAction::DccPanel => {
                self.dcc_open = true;
                cx.notify();
            }
            MenuAction::DccSend(nick) => self.open_dcc_picker(nick, cx),
            MenuAction::Emoji => {
                self.emoji_open = true;
                self.focus_emoji_search = true;
                self.should_focus_input = false;
                cx.notify();
            }
            MenuAction::Jitsi => self.start_jitsi(None, cx),
            MenuAction::Help => self.send_command("/help", cx),
            MenuAction::About => {
                self.about_open = true;
                self.should_focus_input = false;
                self.popup = None;
                cx.notify();
            }
            MenuAction::MinimizeToTray => {
                self.minimize_window = true;
                cx.notify();
            }
            MenuAction::Quit => {
                self.quitting = true;
                for handle in self.networks.values() {
                    handle.shutdown();
                }
                cx.quit();
            }
            MenuAction::Reconnect(network_id) => {
                if let Some(handle) = self.networks.get(&network_id) {
                    handle.reconnect();
                }
                self.set_status(i18n::t("Reconnexion demandée.", "Reconnect requested."), cx);
            }
            MenuAction::Disconnect(network_id) => {
                if let Some(handle) = self.networks.get(&network_id) {
                    handle.disconnect();
                }
                self.set_status(
                    i18n::t("Déconnexion demandée.", "Disconnect requested."),
                    cx,
                );
            }
            MenuAction::JoinPrompt => {
                self.input
                    .update(cx, |input, cx| input.set_text("/join ", cx));
                self.should_focus_input = true;
                cx.notify();
            }
            MenuAction::Join(channel) => self.send_command(format!("/join {channel}"), cx),
            MenuAction::Part(channel) => self.send_command(format!("/part {channel}"), cx),
            MenuAction::CloseBuffer(id) => {
                self.switch_buffer(id, cx);
                self.send_command("/close", cx);
            }
            MenuAction::ClearBuffer(id) => {
                self.model
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clear_buffer(id);
                cx.notify();
            }
            MenuAction::Topic(id) => {
                self.switch_buffer(id, cx);
                self.input
                    .update(cx, |input, cx| input.set_text("/topic ", cx));
                self.should_focus_input = true;
                cx.notify();
            }
            MenuAction::Names(channel) => {
                self.send_command(
                    channel
                        .map(|channel| format!("/names {channel}"))
                        .unwrap_or_else(|| "/names".to_string()),
                    cx,
                );
            }
            MenuAction::Who(mask) => {
                self.send_command(
                    mask.map(|mask| format!("/who {mask}"))
                        .unwrap_or_else(|| "/who".to_string()),
                    cx,
                );
            }
            MenuAction::List => self.open_channel_list(None, cx),
            MenuAction::Query(nick) => self.send_command(format!("/query {nick}"), cx),
            MenuAction::Whois(nick) => self.send_command(format!("/whois {nick}"), cx),
            MenuAction::Mention(nick) => {
                self.input
                    .update(cx, |input, cx| input.insert_text(&format!("{nick}: "), cx));
                self.should_focus_input = true;
                cx.notify();
            }
            MenuAction::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status(
                    i18n::t("Copié dans le presse-papiers.", "Copied to clipboard."),
                    cx,
                );
            }
            MenuAction::Monitor(nick) => self.send_command(format!("/monitor + {nick}"), cx),
            MenuAction::Op(nick) => self.send_command(format!("/op {nick}"), cx),
            MenuAction::Deop(nick) => self.send_command(format!("/deop {nick}"), cx),
            MenuAction::Voice(nick) => self.send_command(format!("/voice {nick}"), cx),
            MenuAction::Devoice(nick) => self.send_command(format!("/devoice {nick}"), cx),
            MenuAction::Kick(nick) => self.send_command(format!("/kick {nick}"), cx),
            MenuAction::Ban(nick) => self.send_command(format!("/ban {nick}!*@*"), cx),
            MenuAction::Invite(nick) => self.send_command(format!("/invite {nick}"), cx),
        }
    }

    fn open_channel_list(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        let Some((network_id, _, _, _)) = self.active_context() else {
            self.set_status(i18n::t("Aucun réseau actif.", "No active network."), cx);
            return;
        };
        let Some(handle) = self.networks.get(&network_id) else {
            self.set_status(i18n::t("Réseau indisponible.", "Network unavailable."), cx);
            return;
        };
        let sender = handle.line_sender();
        let network_name = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model
                .network(network_id)
                .map(|network| network.display_name().to_string())
                .unwrap_or_default()
        };
        let model = self.model.clone();
        let bounds = Bounds::centered(None, size(px(860.), px(620.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(
                    format!(
                        "{} — {network_name}",
                        i18n::t("Liste des salons", "Channel list")
                    )
                    .into(),
                ),
                ..Default::default()
            }),
            app_id: Some("le-salon-channel-list".into()),
            window_min_size: Some(size(px(640.), px(420.))),
            ..Default::default()
        };
        let filter = filter.unwrap_or_default();
        if let Err(error) = cx.open_window(options, move |window, cx| {
            cx.new(|cx| {
                ChannelListWindow::new(window, cx, model, sender, network_id, network_name, filter)
            })
        }) {
            log::warn!("impossible d'ouvrir la liste des salons : {error}");
            self.set_status(
                i18n::t(
                    "Impossible d'ouvrir la liste des salons.",
                    "Could not open the channel list.",
                ),
                cx,
            );
        } else {
            self.set_status(
                i18n::t("Liste des salons demandée.", "Channel list requested."),
                cx,
            );
        }
    }

    fn open_upload_picker(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choisir un fichier à envoyer", "Choose a file to upload").into()),
        });
        cx.spawn(async move |this, cx| match receiver.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.start_upload(path, cx));
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.set_status(
                        format!(
                            "{} {error}",
                            i18n::t(
                                "Sélecteur de fichiers indisponible :",
                                "File picker unavailable:"
                            )
                        ),
                        cx,
                    )
                });
            }
            _ => {}
        })
        .detach();
    }

    fn start_upload(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some((network_id, _, kind, _)) = self.active_context() else {
            self.set_status(
                i18n::t(
                    "Aucun salon actif pour envoyer un fichier.",
                    "No active channel to upload to.",
                ),
                cx,
            );
            return;
        };
        if kind == BufferKind::Server {
            self.set_status(
                i18n::t(
                    "Ouvrez un salon ou un privé pour envoyer le lien.",
                    "Open a channel or query to send the link.",
                ),
                cx,
            );
            return;
        }
        let target = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.active_buffer().map(|buffer| buffer.name.clone())
        };
        let Some(target) = target else {
            return;
        };
        let size = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if size > upload::UPLOAD_MAX {
            self.set_status(
                i18n::t(
                    "Fichier trop lourd (maximum 50 Mo).",
                    "File too large (50 MB maximum).",
                ),
                cx,
            );
            return;
        }
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fichier")
            .to_string();
        let pending = PendingUpload {
            path,
            target,
            network_id,
            size,
            filename,
        };
        if self.config.confirm_upload {
            // Le fichier n'est pas encore envoyé : on demande une
            // confirmation explicite, car litterbox est un service public.
            self.pending_upload = Some(pending);
            self.should_focus_input = false;
            cx.notify();
        } else {
            self.confirm_upload(pending, cx);
        }
    }

    fn confirm_upload(&mut self, pending: PendingUpload, cx: &mut Context<Self>) {
        let Some(handle) = self.networks.get(&pending.network_id) else {
            self.set_status(i18n::t("Réseau indisponible.", "Network unavailable."), cx);
            return;
        };
        let sender = handle.line_sender();
        let french = i18n::is_fr();
        let PendingUpload {
            path,
            target,
            size,
            filename,
            ..
        } = pending;
        self.set_status(
            format!(
                "⏳ {} {filename} ({}…)",
                i18n::t("Téléversement de", "Uploading"),
                upload::format_size(size)
            ),
            cx,
        );
        std::thread::spawn(move || match upload::upload_file(&path) {
            Ok(url) => {
                sender.send_message(target, url.clone());
                sender.send_local(
                    LineKind::Info,
                    if french {
                        format!("⇧ {filename} envoyé : {url}")
                    } else {
                        format!("⇧ {filename} uploaded: {url}")
                    },
                );
            }
            Err(error) => {
                sender.send_local(
                    LineKind::Error,
                    if french {
                        format!("Échec du téléversement de {filename} : {error}")
                    } else {
                        format!("Upload of {filename} failed: {error}")
                    },
                );
            }
        });
    }

    fn open_dcc_picker(&mut self, nick: String, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("DCC SEND → {nick}").into()),
        });
        cx.spawn(async move |this, cx| match receiver.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.start_dcc(nick.clone(), path, cx));
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.set_status(
                        format!(
                            "{} {error}",
                            i18n::t(
                                "Sélecteur de fichiers indisponible :",
                                "File picker unavailable:"
                            )
                        ),
                        cx,
                    )
                });
            }
            _ => {}
        })
        .detach();
    }

    fn start_dcc(&mut self, nick: String, path: PathBuf, cx: &mut Context<Self>) {
        let Some(network_id) = self.active_context().map(|(id, _, _, _)| id) else {
            self.set_status(i18n::t("Aucun réseau actif.", "No active network."), cx);
            return;
        };
        let Some(handle) = self.networks.get(&network_id) else {
            self.set_status(i18n::t("Réseau indisponible.", "Network unavailable."), cx);
            return;
        };
        handle.dcc_send(nick.clone(), path.clone());
        self.dcc_open = true;
        self.set_status(
            format!(
                "{} {nick} — {}",
                i18n::t("Offre DCC envoyée à", "DCC offer sent to"),
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("fichier")
            ),
            cx,
        );
    }

    fn start_jitsi(&mut self, room: Option<String>, cx: &mut Context<Self>) {
        let context = self.active_context();
        let base = context
            .as_ref()
            .map(|(_, _, _, name)| {
                name.trim_start_matches('#')
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>()
            })
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| "irc".to_string());
        let room = room.unwrap_or_else(|| format!("ircv3-{base}-{}", Local::now().timestamp()));
        let room: String = room
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let url = format!("https://meet.jit.si/{room}");
        cx.open_url(&url);
        if let Some((network_id, _, kind, name)) = context {
            if kind != BufferKind::Server {
                if let Some(handle) = self.networks.get(&network_id) {
                    let text = if i18n::is_fr() {
                        format!("▶ Appel Jitsi : {url}")
                    } else {
                        format!("▶ Jitsi call: {url}")
                    };
                    handle.send_message(name, text);
                }
            }
        }
        self.set_status(format!("▶ Jitsi : {url}"), cx);
    }

    fn add_schedule(&mut self, rest: &str, cx: &mut Context<Self>) {
        let Some((network_id, _, kind, target)) = self.active_context() else {
            self.set_status(i18n::t("Aucun salon actif.", "No active channel."), cx);
            return;
        };
        if kind == BufferKind::Server {
            self.set_status(
                i18n::t(
                    "Ouvrez un salon ou un privé pour planifier.",
                    "Open a channel or query to schedule.",
                ),
                cx,
            );
            return;
        }
        let host = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model
                .network(network_id)
                .map(|network| network.host.clone())
                .unwrap_or_default()
        };
        match crate::schedule::parse_command(rest) {
            Ok((text, due)) => {
                let label = due.format("%d/%m %H:%M").to_string();
                self.schedule.add(host, target, text, due);
                self.set_status(
                    format!(
                        "🕒 {} {label}",
                        i18n::t("Message planifié pour", "Message scheduled for")
                    ),
                    cx,
                );
                self.schedule_open = true;
            }
            Err(error) => self.set_status(error, cx),
        }
    }

    fn process_schedules(&mut self, cx: &mut Context<Self>) {
        if self.schedule.items.is_empty() || self.networks.is_empty() {
            return;
        }
        {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            if !model
                .networks
                .iter()
                .any(|network| network.status == ConnectionStatus::Connected)
            {
                return;
            }
        }
        let due = self.schedule.take_due(Local::now());
        for item in due {
            let network_id = {
                let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
                model
                    .networks
                    .iter()
                    .find(|network| {
                        network.host == item.network
                            && network.status == ConnectionStatus::Connected
                    })
                    .or_else(|| {
                        model
                            .networks
                            .iter()
                            .find(|network| network.status == ConnectionStatus::Connected)
                    })
                    .map(|network| network.id)
            };
            let Some(network_id) = network_id else {
                continue;
            };
            let Some(handle) = self.networks.get(&network_id) else {
                continue;
            };
            handle.send_message(item.target.clone(), item.text.clone());
            handle.send_local(
                LineKind::Info,
                format!(
                    "◷ {} {} : {}",
                    i18n::t("Message planifié envoyé à", "Scheduled message sent to"),
                    item.target,
                    item.text
                ),
            );
            let _ = cx;
        }
    }

    fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        self.config.notifications = !self.config.notifications;
        if let Err(error) = self.config.save() {
            log::warn!("sauvegarde des notifications impossible : {error}");
        }
        self.set_status(
            if self.config.notifications {
                i18n::t("Notifications activées.", "Notifications enabled.")
            } else {
                i18n::t("Notifications désactivées.", "Notifications disabled.")
            },
            cx,
        );
    }

    fn process_tray(&mut self, cx: &mut Context<Self>) {
        while let Ok(command) = self.tray_rx.try_recv() {
            match command {
                TrayCommand::Show => {
                    self.raise_window = true;
                    cx.activate(true);
                    let _ = self.window_handle.update(cx, |_, window, _| {
                        window.activate_window();
                    });
                }
                TrayCommand::Minimize => {
                    self.minimize_window = true;
                    let _ = self.window_handle.update(cx, |_, window, _| {
                        window.minimize_window();
                    });
                }
                TrayCommand::Preferences => self.open_preferences(cx),
                TrayCommand::ToggleNotifications => self.toggle_notifications(cx),
                TrayCommand::DisconnectAll => {
                    for handle in self.networks.values() {
                        handle.disconnect();
                    }
                    self.set_status(
                        i18n::t(
                            "Tous les réseaux sont déconnectés.",
                            "All networks disconnected.",
                        ),
                        cx,
                    );
                }
                TrayCommand::Quit => {
                    self.quitting = true;
                    for handle in self.networks.values() {
                        handle.shutdown();
                    }
                    cx.quit();
                }
            }
        }
    }

    fn check_notifications(&mut self, cx: &mut Context<Self>) {
        if !self.notifications_ready {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            for buffer in model.buffers.values() {
                self.notify_seen.insert(buffer.id, buffer.version);
            }
            self.notifications_ready = true;
            return;
        }

        let mut updates = Vec::new();
        let mut desktop = Vec::new();
        let mut toasts = Vec::new();
        {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            let active = model.active;
            for buffer in model.buffers.values() {
                let previous = self.notify_seen.get(&buffer.id).copied().unwrap_or(0);
                if buffer.version <= previous {
                    continue;
                }
                updates.push((buffer.id, buffer.version));
                if buffer.kind == BufferKind::Server {
                    continue;
                }
                let Some(line) = buffer.messages.back() else {
                    continue;
                };
                if line.self_nick {
                    continue;
                }
                // Ne pas afficher de toast pour le message qu'on est en
                // train de lire dans la fenêtre active.
                if self.window_active && active == Some(buffer.id) {
                    continue;
                }

                // `line.mention` est propre au message ; ne pas se fier à
                // `buffer.highlight`, qui reste vrai jusqu'à l'ouverture du
                // salon et renotifierait chaque message suivant.
                if !notification_relevant(buffer.kind, line, self.config.notify_mentions_only) {
                    continue;
                }
                let is_query = buffer.kind == BufferKind::Query;

                let nick = line.nick.clone().unwrap_or_default();
                let (title, body) = if is_query {
                    (
                        format!("{} {nick}", i18n::t("Message de", "Message from")),
                        line.text.clone(),
                    )
                } else {
                    (format!("{} — {nick}", buffer.name), line.text.clone())
                };
                let title = clean_notification_text(&title);
                let body: String = clean_notification_text(&body).chars().take(180).collect();

                if self.config.in_app_toasts {
                    toasts.push((title.clone(), body.clone()));
                }
                if self.config.notifications && !self.window_active {
                    desktop.push((title, body));
                }
            }
        }

        for (id, version) in updates {
            self.notify_seen.insert(id, version);
        }
        for (title, body) in toasts {
            self.push_toast(title, body);
        }
        for (title, body) in desktop {
            self.notify_desktop(&title, &body);
        }
        if !self.toasts.is_empty() {
            cx.notify();
        }
    }

    fn push_toast(&mut self, title: String, body: String) {
        let now = Instant::now();
        self.toasts.retain(|toast| toast.expires_at > now);
        if self.toasts.len() >= 4 {
            self.toasts.remove(0);
        }
        let seconds = self.config.toast_seconds.clamp(1, 10);
        self.toasts.push(Toast {
            id: self.next_toast_id,
            title,
            body,
            expires_at: now + Duration::from_secs(seconds),
        });
        self.next_toast_id = self.next_toast_id.wrapping_add(1).max(1);
    }

    fn expire_toasts(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|toast| toast.expires_at > now);
    }

    fn render_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.toasts.is_empty() {
            return div();
        }
        let viewport = window.viewport_size();
        let width = px(350.);
        let left = (viewport.width - width - px(16.)).max(px(12.));
        let mut rows: Vec<AnyElement> = Vec::new();
        for toast in &self.toasts {
            let id = toast.id;
            rows.push(
                div()
                    .id(("toast", id as usize))
                    .w_full()
                    .flex()
                    .items_start()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::accent())
                    .shadow_lg()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.toasts.retain(|toast| toast.id != id);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(24.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme::accent_dark())
                            .text_color(theme::accent())
                            .text_size(px(12.))
                            .child("●"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme::accent())
                                    .truncate()
                                    .child(toast.title.clone()),
                            )
                            .child(
                                div()
                                    .max_w_full()
                                    .text_size(px(11.5))
                                    .text_color(theme::text())
                                    .line_clamp(3)
                                    .child(toast.body.clone()),
                            ),
                    )
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .left(left)
            .top(px(12.))
            .w(width)
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
    }

    fn notify_desktop(&self, title: &str, body: &str) {
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("notify-send")
                .arg("--app-name=Le Salon")
                .arg("--icon=le-salon")
                .arg("--")
                .arg(title)
                .arg(body)
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let escape = |value: &str| {
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace(['\n', '\r'], " ")
            };
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                escape(body),
                escape(title)
            );
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .spawn();
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (title, body);
        }
    }

    fn insert_emoji(&mut self, emoji: &str, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.insert_text(emoji, cx));
        self.emoji_open = false;
        self.should_focus_input = true;
        cx.notify();
    }

    fn handle_special_command(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let trimmed = text.trim();
        if trimmed == "/emoji" {
            self.emoji_open = true;
            self.input.update(cx, |input, cx| input.clear(cx));
            self.should_focus_input = true;
            cx.notify();
            return true;
        }
        if trimmed == "/list" || trimmed.starts_with("/list ") {
            let filter = trimmed
                .strip_prefix("/list")
                .unwrap_or("")
                .trim()
                .to_string();
            self.open_channel_list((!filter.is_empty()).then_some(filter), cx);
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if trimmed == "/upload" {
            self.open_upload_picker(cx);
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if let Some(path) = trimmed.strip_prefix("/upload ") {
            let path = path.trim().trim_matches('"').trim_matches('\'');
            self.start_upload(PathBuf::from(path), cx);
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if trimmed == "/jitsi" || trimmed == "/call" {
            self.start_jitsi(None, cx);
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if let Some(room) = trimmed
            .strip_prefix("/jitsi ")
            .or_else(|| trimmed.strip_prefix("/call "))
        {
            self.start_jitsi(Some(room.trim().to_string()), cx);
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if trimmed == "/dcc" {
            self.dcc_open = true;
            self.input.update(cx, |input, cx| input.clear(cx));
            cx.notify();
            return true;
        }
        if trimmed == "/dcc cancel" {
            self.model
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cancel_dcc(None);
            self.set_status(
                i18n::t("Transferts DCC annulés.", "DCC transfers canceled."),
                cx,
            );
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if let Some(rest) = trimmed.strip_prefix("/dcc send ") {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let nick = parts.next().unwrap_or("").trim();
            let path = parts
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            if nick.is_empty() || path.is_empty() {
                self.set_status(
                    i18n::t(
                        "Usage : /dcc send pseudo /chemin/fichier",
                        "Usage: /dcc send nick /path/file",
                    ),
                    cx,
                );
            } else {
                self.start_dcc(nick.to_string(), PathBuf::from(path), cx);
            }
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        if trimmed == "/schedule" || trimmed == "/sched" {
            self.schedule_open = true;
            self.input.update(cx, |input, cx| input.clear(cx));
            cx.notify();
            return true;
        }
        if let Some(rest) = trimmed
            .strip_prefix("/schedule ")
            .or_else(|| trimmed.strip_prefix("/planifie "))
        {
            self.add_schedule(rest, cx);
            self.input.update(cx, |input, cx| input.clear(cx));
            return true;
        }
        false
    }

    fn render_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(popup) = self.popup.clone() else {
            return div();
        };
        let viewport = window.viewport_size();
        let estimate = popup.items.len() as f32 * 31.0 + 18.0;
        let max_x = (viewport.width - px(270.)).max(px(8.));
        let max_y = (viewport.height - px(estimate)).max(px(8.));
        let x = popup.x.max(px(8.)).min(max_x);
        let y = popup.y.max(px(8.)).min(max_y);
        let mut rows: Vec<AnyElement> = Vec::new();
        for item in &popup.items {
            if item.separator {
                rows.push(
                    div()
                        .h(px(1.))
                        .mx_2()
                        .my_1()
                        .bg(theme::border())
                        .into_any_element(),
                );
            }
            let action = item.action.clone();
            let danger = item.danger;
            let checked = item.checked;
            let label = item.label.clone();
            let shortcut = item.shortcut.clone();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(danger, |this| this.text_color(theme::red()))
                    .hover(|style| style.bg(theme::panel_alt()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.popup = None;
                            this.run_menu_action(action.clone(), cx);
                        }),
                    )
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_color(theme::accent())
                            .child(if checked { "✓" } else { "" }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when_some(shortcut, |this, shortcut| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(px(9.5))
                                .text_color(theme::faint())
                                .child(shortcut),
                        )
                    })
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_popup(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.close_popup(cx)),
            )
            .child(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(px(260.))
                    .flex()
                    .flex_col()
                    .p_1()
                    .rounded_md()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .text_size(px(12.))
                    .text_color(theme::text())
                    .children(rows),
            )
    }

    fn render_theme_picker(&mut self, cx: &mut Context<Self>) -> Div {
        let current = theme::current_index();
        let mut cards: Vec<AnyElement> = Vec::new();
        for (index, entry) in theme::themes().iter().enumerate() {
            let name = i18n::t(entry.name_fr, entry.name_en);
            cards.push(
                div()
                    .w(px(168.))
                    .h(px(92.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(entry.bg))
                    .border_2()
                    .border_color(if index == current {
                        rgb(entry.accent)
                    } else {
                        rgb(entry.border)
                    })
                    .cursor_pointer()
                    .hover(|style| style.border_color(rgb(entry.accent)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.apply_theme(index, cx)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(rgb(entry.text))
                            .child(name),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(div().size(px(14.)).rounded_sm().bg(rgb(entry.accent)))
                            .child(div().size(px(14.)).rounded_sm().bg(rgb(entry.panel_alt)))
                            .child(div().size(px(14.)).rounded_sm().bg(rgb(entry.green)))
                            .child(div().size(px(14.)).rounded_sm().bg(rgb(entry.red))),
                    )
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.theme_picker_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(760.))
                    .h(gpui::relative(0.88))
                    .max_h(px(620.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(i18n::t("Thèmes", "Themes")),
                            )
                            .child(
                                div()
                                    .id("theme-close")
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::panel_alt()))
                                    .tooltip(|_, cx| {
                                        tooltip::simple(
                                            cx,
                                            i18n::t("Fermer", "Close"),
                                            i18n::t(
                                                "Fermer la galerie de thèmes",
                                                "Close the theme gallery",
                                            ),
                                        )
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.theme_picker_open = false;
                                            cx.notify();
                                        }),
                                    )
                                    .child("✕"),
                            ),
                    )
                    .child(
                        div()
                            .id("theme-grid")
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .overflow_y_scroll()
                            .children(cards),
                    ),
            )
    }

    fn render_emoji_picker(&mut self, cx: &mut Context<Self>) -> Div {
        let query = self.emoji_search.read(cx).content().to_string();
        let french = i18n::is_fr();
        let mut buttons: Vec<AnyElement> = Vec::new();
        let push_symbol =
            |symbol: &'static str, buttons: &mut Vec<AnyElement>, cx: &mut Context<Self>| {
                buttons.push(
                    div()
                        .size(px(34.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .font_family(theme::EMOJI_FONT)
                        .text_size(px(22.))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme::panel_alt()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.insert_emoji(symbol, cx)),
                        )
                        .child(symbol)
                        .into_any_element(),
                );
            };
        if query.trim().is_empty() {
            for group in emoji::GROUPS {
                buttons.push(
                    div()
                        .w_full()
                        .pt_2()
                        .pb_1()
                        .text_size(px(10.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme::faint())
                        .child(i18n::t(group.name_fr, group.name_en))
                        .into_any_element(),
                );
                for (symbol, _, _) in group.items {
                    push_symbol(symbol, &mut buttons, cx);
                }
            }
        } else {
            for (symbol, _, _) in emoji::filtered(&query, french) {
                push_symbol(symbol, &mut buttons, cx);
            }
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.emoji_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(660.))
                    .h(gpui::relative(0.82))
                    .max_h(px(560.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(i18n::t("Emojis", "Emojis")),
                            )
                            .child(
                                div()
                                    .id("emoji-close")
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme::panel_alt()))
                                    .tooltip(|_, cx| {
                                        tooltip::simple(
                                            cx,
                                            i18n::t("Fermer", "Close"),
                                            i18n::t(
                                                "Fermer le sélecteur d'emojis",
                                                "Close the emoji picker",
                                            ),
                                        )
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.emoji_open = false;
                                            cx.notify();
                                        }),
                                    )
                                    .child("✕"),
                            ),
                    )
                    .child(self.emoji_search.clone())
                    .child(
                        div()
                            .id("emoji-grid")
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .overflow_y_scroll()
                            .children(buttons),
                    ),
            )
    }

    fn render_dcc_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let transfers: Vec<_> = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.dcc.clone()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for transfer in &transfers {
            let status = match transfer.status {
                DccStatus::Waiting => i18n::t("en attente", "waiting"),
                DccStatus::Connected => i18n::t("connecté", "connected"),
                DccStatus::Sending => i18n::t("envoi…", "sending…"),
                DccStatus::Done => i18n::t("terminé", "done"),
                DccStatus::Failed => i18n::t("échec", "failed"),
                DccStatus::Canceled => i18n::t("annulé", "canceled"),
            };
            let percent = transfer.percent();
            let id = transfer.id;
            let active = transfer.is_active();
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(transfer.filename.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.5))
                                    .text_color(theme::muted())
                                    .child(format!("{} → {}", transfer.nick, status)),
                            )
                            .when(active, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .bg(theme::error_alpha(0.15))
                                        .text_color(theme::red())
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.model
                                                    .lock()
                                                    .unwrap_or_else(|e| e.into_inner())
                                                    .cancel_dcc(Some(id));
                                                cx.notify();
                                            }),
                                        )
                                        .child(i18n::t("Annuler", "Cancel")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded_full()
                            .bg(theme::border())
                            .child(
                                div()
                                    .h_full()
                                    .w(px(360. * percent / 100.))
                                    .rounded_full()
                                    .bg(if transfer.status == DccStatus::Failed {
                                        theme::red()
                                    } else {
                                        theme::accent()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(9.5))
                            .text_color(theme::faint())
                            .child(format!(
                                "{} / {}",
                                upload::format_size(transfer.sent),
                                upload::format_size(transfer.size)
                            ))
                            .child(format!("{percent:.0} %")),
                    )
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.dcc_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(700.))
                    .max_h(px(520.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(i18n::t("Transferts DCC", "DCC transfers")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .hover(|style| style.bg(theme::panel_alt()))
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                this.model
                                                    .lock()
                                                    .unwrap_or_else(|e| e.into_inner())
                                                    .clear_finished_dcc();
                                                cx.notify();
                                            }))
                                            .child(i18n::t("Nettoyer", "Clear finished")),
                                    )
                                    .child(
                                        div()
                                            .id("dcc-close")
                                            .px_2()
                                            .py_1()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .hover(|style| style.bg(theme::panel_alt()))
                                            .tooltip(|_, cx| {
                                                tooltip::simple(
                                                    cx,
                                                    i18n::t("Fermer", "Close"),
                                                    i18n::t(
                                                        "Fermer le panneau DCC",
                                                        "Close the DCC panel",
                                                    ),
                                                )
                                            })
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                this.dcc_open = false;
                                                cx.notify();
                                            }))
                                            .child("✕"),
                                    ),
                            ),
                    )
                    .when(rows.is_empty(), |this| {
                        this.child(
                            div()
                                .py_6()
                                .text_color(theme::muted())
                                .child(i18n::t("Aucun transfert DCC.", "No DCC transfer.")),
                        )
                    })
                    .child(
                        div()
                            .id("dcc-list")
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::faint())
                            .child(i18n::t(
                                "Le port DCC doit être joignable ; derrière un CGNAT, le transfert direct ne fonctionne pas. Le transfert n'est pas chiffré et le serveur IRC voit l'offre (IP/port) : n'envoie pas de fichier sensible.",
                                "The DCC port must be reachable; behind CGNAT, direct transfer does not work. The transfer is unencrypted and the IRC server sees the offer (IP/port): do not send sensitive files.",
                            )),
                    ),
            )
    }

    fn render_schedule_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let items = self.schedule.items.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for item in &items {
            let id = item.id;
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme::accent())
                            .child(item.due_label()),
                    )
                    .child(
                        div()
                            .w(px(120.))
                            .flex_none()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(item.target.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .child(item.text.clone()),
                    )
                    .child(
                        div()
                            .id(("schedule-remove", id))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .text_color(theme::red())
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::panel_alt()))
                            .tooltip(|_, cx| {
                                tooltip::simple(
                                    cx,
                                    i18n::t("Retirer", "Remove"),
                                    i18n::t(
                                        "Supprimer ce message planifié",
                                        "Delete this scheduled message",
                                    ),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.schedule.remove(id);
                                    cx.notify();
                                }),
                            )
                            .child("✕"),
                    )
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x000000a8))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.schedule_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(720.))
                    .max_h(px(520.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::border())
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(i18n::t("Messages planifiés", "Scheduled messages")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("schedule-new")
                                            .px_3()
                                            .py_1()
                                            .rounded_sm()
                                            .bg(theme::accent())
                                            .text_color(theme::on_accent())
                                            .cursor_pointer()
                                            .tooltip(|_, cx| {
                                                tooltip::simple(
                                                    cx,
                                                    i18n::t("Nouveau message planifié", "New scheduled message"),
                                                    i18n::t(
                                                        "Prépare la commande /schedule",
                                                        "Prepares the /schedule command",
                                                    ),
                                                )
                                            })
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                this.schedule_open = false;
                                                this.input.update(cx, |input, cx| input.set_text("/schedule ", cx));
                                                this.should_focus_input = true;
                                                cx.notify();
                                            }))
                                            .child(i18n::t("Nouveau", "New")),
                                    )
                                    .child(
                                        div()
                                            .id("schedule-close")
                                            .px_2()
                                            .py_1()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .hover(|style| style.bg(theme::panel_alt()))
                                            .tooltip(|_, cx| {
                                                tooltip::simple(
                                                    cx,
                                                    i18n::t("Fermer", "Close"),
                                                    i18n::t(
                                                        "Fermer les messages planifiés",
                                                        "Close scheduled messages",
                                                    ),
                                                )
                                            })
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                this.schedule_open = false;
                                                cx.notify();
                                            }))
                                            .child("✕"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::faint())
                            .child(i18n::t(
                                "Ajoutez avec /schedule +10m texte, /schedule 18:30 texte ou /schedule 2026-11-08 09:00 texte.",
                                "Add with /schedule +10m text, /schedule 18:30 text or /schedule 2026-11-08 09:00 text.",
                            )),
                    )
                    .when(rows.is_empty(), |this| {
                        this.child(
                            div()
                                .py_6()
                                .text_color(theme::muted())
                                .child(i18n::t("Aucun message planifié.", "No scheduled message.")),
                        )
                    })
                    .child(
                        div()
                            .id("schedule-list")
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_active = window.is_window_active();
        if self.raise_window {
            self.raise_window = false;
            window.activate_window();
        }
        if self.minimize_window {
            self.minimize_window = false;
            window.minimize_window();
        }
        let has_networks = !self.networks.is_empty();
        let title = {
            let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
            model.window_title()
        };
        window.set_window_title(&title);

        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .font_family(theme::UI_FONT)
            .text_size(px(14.))
            .text_color(theme::text())
            .track_focus(&self.focus_handle)
            .key_context("Workspace")
            .on_action(cx.listener(Self::app_quit))
            .on_action(cx.listener(Self::new_network_action))
            .on_action(cx.listener(Self::clear_buffer_action))
            .on_action(cx.listener(Self::focus_composer_action))
            .on_action(cx.listener(Self::open_themes_action))
            .on_action(cx.listener(Self::open_emoji_action))
            .on_action(cx.listener(Self::open_preferences_action))
            .on_mouse_move(cx.listener(Self::on_root_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_root_mouse_up))
            .on_action(cx.listener(Self::tab_next))
            .on_action(cx.listener(Self::tab_prev))
            .on_action(cx.listener(Self::goto1))
            .on_action(cx.listener(Self::goto2))
            .on_action(cx.listener(Self::goto3))
            .on_action(cx.listener(Self::goto4))
            .on_action(cx.listener(Self::goto5))
            .on_action(cx.listener(Self::goto6))
            .on_action(cx.listener(Self::goto7))
            .on_action(cx.listener(Self::goto8))
            .on_action(cx.listener(Self::goto9));

        if has_networks {
            root = root
                .child(self.topbar(cx))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(self.sidebar(cx))
                        .child(self.resize_handle(ResizeSide::Sidebar, cx))
                        .child(self.main_view(window, cx)),
                )
                .child(self.statusbar(cx));
            if let Some(form) = self.form.clone() {
                root = root.child(self.overlay(form));
            }
        } else {
            root = root.child(self.welcome(cx));
        }

        if self.theme_picker_open {
            root = root.child(self.render_theme_picker(cx));
        }
        if self.emoji_open {
            if self.focus_emoji_search {
                self.focus_emoji_search = false;
                let handle = self.emoji_search.read(cx).focus_handle(cx);
                window.focus(&handle);
            }
            root = root.child(self.render_emoji_picker(cx));
        }
        if self.dcc_open {
            root = root.child(self.render_dcc_panel(cx));
        }
        if self.schedule_open {
            root = root.child(self.render_schedule_panel(cx));
        }
        if self.about_open {
            root = root.child(self.render_about_dialog(cx));
        }
        if self.pending_upload.is_some() {
            root = root.child(self.render_upload_confirm(cx));
        }
        if self.rename_open {
            if self.focus_rename_input {
                self.focus_rename_input = false;
                let handle = self.rename_input.read(cx).focus_handle(cx);
                window.focus(&handle);
            }
            root = root.child(self.render_rename_dialog(cx));
        }
        if self.popup.is_some() {
            root = root.child(self.render_popup(window, cx));
        }
        if !self.toasts.is_empty() {
            root = root.child(self.render_toasts(window, cx));
        }

        if self.should_focus_input
            && self.form.is_none()
            && !self.theme_picker_open
            && !self.emoji_open
            && !self.dcc_open
            && !self.schedule_open
            && !self.rename_open
            && !self.about_open
            && self.pending_upload.is_none()
        {
            self.should_focus_input = false;
            let handle = self.input.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
        root
    }
}

impl Focusable for AppRoot {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for AppRoot {
    fn drop(&mut self) {
        for handle in self.networks.values() {
            handle.shutdown();
        }
    }
}

fn ordered_buffers(model: &AppModel) -> Vec<BufferId> {
    let mut ids = Vec::new();
    for network in &model.networks {
        ids.extend(ordered_buffers_for(model, network.id));
    }
    ids
}

fn ordered_buffers_for(model: &AppModel, network_id: NetworkId) -> Vec<BufferId> {
    let Some(network) = model.network(network_id) else {
        return Vec::new();
    };
    let mut ids = network.buffers.clone();
    ids.retain(|id| model.buffers.contains_key(id));
    ids.sort_by(|a, b| {
        let left = model.buffers.get(a);
        let right = model.buffers.get(b);
        let rank = |kind: BufferKind| match kind {
            BufferKind::Server => 0,
            BufferKind::Channel => 1,
            BufferKind::Query => 2,
        };
        let left_rank = left.map(|buffer| rank(buffer.kind)).unwrap_or(9);
        let right_rank = right.map(|buffer| rank(buffer.kind)).unwrap_or(9);
        left_rank.cmp(&right_rank).then_with(|| {
            irc_fold(left.map(|buffer| buffer.name.as_str()).unwrap_or("")).cmp(&irc_fold(
                right.map(|buffer| buffer.name.as_str()).unwrap_or(""),
            ))
        })
    });
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_text_is_sanitized() {
        assert_eq!(
            clean_notification_text("salut\u{202e} toi\u{01}"),
            "salut toi"
        );
        assert_eq!(clean_notification_text("a\u{02}b\u{02}c"), "abc");
    }

    #[test]
    fn notification_scope_is_respected() {
        let mut line = ChatLine::new(LineKind::Message, Some("alice".into()), "salut");
        assert!(!notification_relevant(BufferKind::Channel, &line, true));
        assert!(notification_relevant(BufferKind::Channel, &line, false));
        assert!(notification_relevant(BufferKind::Query, &line, true));

        line.mention = true;
        assert!(notification_relevant(BufferKind::Channel, &line, true));

        line.kind = LineKind::Join;
        line.mention = false;
        assert!(!notification_relevant(BufferKind::Channel, &line, false));

        line.kind = LineKind::Message;
        line.self_nick = true;
        assert!(!notification_relevant(BufferKind::Query, &line, false));
    }
}

// Petit utilitaire pour les tests visuels éventuels.
#[allow(dead_code)]
fn _line_kind_name(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Message => "message",
        LineKind::Action => "action",
        LineKind::Notice => "notice",
        LineKind::Error => "error",
        LineKind::Join => "join",
        LineKind::Part => "part",
        LineKind::Quit => "quit",
        LineKind::Nick => "nick",
        LineKind::Mode => "mode",
        LineKind::Topic => "topic",
        LineKind::Kick => "kick",
        LineKind::Invite => "invite",
        LineKind::Away => "away",
        LineKind::Account => "account",
        LineKind::Server => "server",
        LineKind::Motd => "motd",
        LineKind::Info => "info",
        LineKind::History => "history",
        LineKind::Other => "other",
    }
}
