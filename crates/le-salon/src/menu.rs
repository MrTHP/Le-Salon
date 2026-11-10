//! Menus contextuels et menus de la barre supérieure.
//!
//! Les entrées ne contiennent que des identifiants d'action ; la vue racine
//! les exécute (`AppRoot::run_menu_action`).

use gpui::{Pixels, SharedString};

use ircv3_core::model::{BufferId, ChatLine, NetworkId};

#[derive(Clone, Debug)]
pub enum MenuAction {
    NewNetwork,
    Preferences,
    Themes,
    SetLanguage(&'static str),
    ToggleNotifications,
    Upload,
    Schedule,
    DccPanel,
    DccSend(String),
    Emoji,
    Jitsi,
    Help,
    About,
    Quit,
    MinimizeToTray,
    Reconnect(NetworkId),
    Disconnect(NetworkId),
    JoinPrompt,
    Join(String),
    Part(String),
    CloseBuffer(BufferId),
    ClearBuffer(BufferId),
    Topic(BufferId),
    Names(Option<String>),
    Who(Option<String>),
    List,
    Query(String),
    Whois(String),
    Mention(String),
    Copy(String),
    Monitor(String),
    Op(String),
    Deop(String),
    Voice(String),
    Devoice(String),
    Kick(String),
    Ban(String),
    BanKick(String),
    Invite(String),
    ReplyMessage(ChatLine),
    ReactMessage(BufferId, ChatLine),
    SaveMessage(BufferId, ChatLine),
    ShareMessage(BufferId, ChatLine),
    ShareExcerpt(BufferId),
    SearchMessages,
    SavedMessages,
    Activity,
    Palette,
    ToggleMute(String),
    TogglePin(String, BufferId),
    ExportTheme,
    ImportTheme,
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: SharedString,
    pub action: MenuAction,
    pub danger: bool,
    pub separator: bool,
    pub checked: bool,
    pub shortcut: Option<SharedString>,
}

impl MenuItem {
    pub fn new(label: impl Into<SharedString>, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            action,
            danger: false,
            separator: false,
            checked: false,
            shortcut: None,
        }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn separator(mut self) -> Self {
        self.separator = true;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

#[derive(Clone, Debug)]
pub struct PopupMenu {
    pub x: Pixels,
    pub y: Pixels,
    pub items: Vec<MenuItem>,
}

impl PopupMenu {
    pub fn new(x: Pixels, y: Pixels, items: Vec<MenuItem>) -> Self {
        Self { x, y, items }
    }
}
