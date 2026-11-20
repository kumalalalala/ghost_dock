//! System tray icon, context menu, and global summon hotkey for GhostDock.

use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::icon::render_ghost_dock_rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ToggleDashboard,
    ShowDashboard,
    RestoreAllWindows,
    Quit,
}

pub struct TrayController {
    _tray: TrayIcon,
    _hotkey_manager: Option<GlobalHotKeyManager>,
    pub tx: Sender<TrayCommand>,
    pub rx: Receiver<TrayCommand>,
}

fn build_minimal_icon() -> Icon {
    let size = 64u32;
    let rgba = render_ghost_dock_rgba(size);
    Icon::from_rgba(rgba, size, size).expect("valid 64x64 RGBA icon")
}

impl TrayController {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx): (Sender<TrayCommand>, Receiver<TrayCommand>) = mpsc::channel();

        let id_show = MenuId::new("ghost_dock_show");
        let id_restore = MenuId::new("ghost_dock_restore_all");
        let id_quit = MenuId::new("ghost_dock_quit");

        let menu = Menu::new();
        let item_show = MenuItem::with_id(id_show.clone(), "Show / Hide GhostDock", true, None);
        let item_restore =
            MenuItem::with_id(id_restore.clone(), "Restore All Hidden Windows", true, None);
        let item_quit = MenuItem::with_id(id_quit.clone(), "Quit (Restore All)", true, None);
        let _ = menu.append(&item_show);
        let _ = menu.append(&item_restore);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_quit);

        // Register handlers BEFORE touching receivers (OnceCell initialization).
        let tx_tray = tx.clone();
        let ctx_tray = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = tx_tray.send(TrayCommand::ToggleDashboard);
                ctx_tray.request_repaint();
            }
        }));

        let tx_menu = tx.clone();
        let ctx_menu = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let cmd = if event.id == id_show {
                Some(TrayCommand::ShowDashboard)
            } else if event.id == id_restore {
                Some(TrayCommand::RestoreAllWindows)
            } else if event.id == id_quit {
                Some(TrayCommand::Quit)
            } else {
                None
            };
            if let Some(c) = cmd {
                let _ = tx_menu.send(c);
                ctx_menu.request_repaint();
            }
        }));

        let hotkey_manager = GlobalHotKeyManager::new().ok();
        let summon_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
        let summon_id = summon_hotkey.id();
        if let Some(mgr) = &hotkey_manager {
            let _ = mgr.register(summon_hotkey);
        }

        let tx_hk = tx.clone();
        let ctx_hk = ctx;
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id == summon_id && event.state == HotKeyState::Pressed {
                let _ = tx_hk.send(TrayCommand::ToggleDashboard);
                ctx_hk.request_repaint();
            }
        }));

        let tray = TrayIconBuilder::new()
            .with_tooltip("GhostDock — Ctrl+Shift+Space to toggle")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(build_minimal_icon())
            .build()
            .expect("failed to create Windows tray icon");

        Self {
            _tray: tray,
            _hotkey_manager: hotkey_manager,
            tx,
            rx,
        }
    }
}
