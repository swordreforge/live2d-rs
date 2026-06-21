//! System tray icon for background operation

use tray_icon::menu::{Menu, MenuItem, MenuEvent};
use tray_icon::{TrayIcon, TrayIconBuilder, Icon};

/// Custom events sent from tray to main event loop
#[derive(Debug, Clone)]
pub enum AppEvent {
    ShowWindow,
    Quit,
}

/// Create a TrayIcon and return it along with the menu event receiver.
pub fn create_tray() -> (TrayIcon, MenuEventReceiver) {
    let menu = Menu::new();
    let show_item = MenuItem::with_id("show", "Show Window", true, None);
    let quit_item = MenuItem::with_id("quit", "Quit", true, None);
    menu.append_items(&[&show_item, &quit_item]).ok();

    let icon = Icon::from_rgba(
        vec![0x33, 0x99, 0xff, 0xff],
        1, 1,
    ).expect("create icon");
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Live2D Pet")
        .with_icon(icon)
        .build()
        .expect("build tray icon");

    (tray, MenuEventReceiver::new())
}

/// Wraps the menu event channel for polling from the winit event loop.
pub struct MenuEventReceiver {
    rx: std::sync::mpsc::Receiver<String>,
}

impl MenuEventReceiver {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        MenuEvent::set_event_handler(Some(Box::new(move |event: MenuEvent| {
            let id = event.id.0.clone();
            let _ = tx.send(id);
        })));
        Self { rx }
    }

    /// Poll for pending menu events. Returns the IDs of clicked items.
    pub fn poll(&self) -> Vec<String> {
        let mut events = Vec::new();
        while let Ok(id) = self.rx.try_recv() {
            events.push(id);
        }
        events
    }
}

/// Dummy receiver that returns nothing (used when GTK init fails)
pub fn dummy_receiver() -> MenuEventReceiver {
    let (_, rx) = std::sync::mpsc::channel();
    MenuEventReceiver { rx }
}
