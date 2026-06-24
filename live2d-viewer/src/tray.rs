//! System tray icon for background operation.
//!
//! Uses [`ksni`](https://crates.io/crates/ksni) on Linux (pure Rust DBus
//! StatusNotifierItem, no GTK), and [`tray-icon`](https://crates.io/crates/tray-icon)
//! on macOS/Windows (native platform APIs).

use std::sync::mpsc;

/// Load the embedded app icon (from `res/icon.png`) as raw RGBA8 pixels.
///
/// Shared by both the Linux (`ksni`) and non-Linux (`tray-icon`) backends.
/// The PNG is compiled into the binary via [`include_bytes!`].
fn embedded_icon_rgba() -> (Vec<u8>, u32, u32) {
    let png = include_bytes!("../res/icon.png");
    let img = image::load_from_memory(png)
        .expect("embedded tray icon (res/icon.png)")
        .into_rgba8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

/// Custom events sent from the tray to the winit event loop.
#[derive(Debug, Clone)]
pub enum AppEvent {
    ShowWindow,
    Quit,
    ToggleClickThrough,
}

/// Wraps an [`mpsc::Receiver`] and exposes a non-blocking [`poll`](Self::poll).
pub struct MenuEventReceiver {
    rx: mpsc::Receiver<String>,
}

impl MenuEventReceiver {
    /// Poll for pending menu events. Returns the IDs of all clicked items
    /// since the last call (typically 0 or 1).
    pub fn poll(&self) -> Vec<String> {
        let mut events = Vec::new();
        while let Ok(id) = self.rx.try_recv() {
            events.push(id);
        }
        events
    }
}

// ── Linux: ksni (pure Rust DBus StatusNotifierItem) ────────────────

#[cfg(target_os = "linux")]
mod tray_imp {
    use super::*;
    use ksni::blocking::TrayMethods;

    pub struct Live2dTray {
        pub tx: mpsc::Sender<String>,
    }

    impl ksni::Tray for Live2dTray {
        fn id(&self) -> String {
            "live2d-viewer".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let (rgba, w, h) = embedded_icon_rgba();
            // ksni expects ARGB byte order
            let data: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            vec![ksni::Icon {
                width: w as i32,
                height: h as i32,
                data,
            }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Live2D Pet".into(),
                ..Default::default()
            }
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            vec![
                StandardItem {
                    label: "Show Window".into(),
                    activate: Box::new(|this: &mut Live2dTray| {
                        let _ = this.tx.send("show".into());
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Toggle Click-Through".into(),
                    activate: Box::new(|this: &mut Live2dTray| {
                        let _ = this.tx.send("clickthrough".into());
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|this: &mut Live2dTray| {
                        let _ = this.tx.send("quit".into());
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub fn create_tray() -> (ksni::blocking::Handle<Live2dTray>, MenuEventReceiver) {
        let (tx, rx) = mpsc::channel();
        let tray = Live2dTray { tx };
        let handle = tray
            .spawn()
            .expect("ksni tray: failed to register StatusNotifierItem");
        (handle, MenuEventReceiver { rx })
    }
}

#[cfg(target_os = "linux")]
pub use tray_imp::create_tray;

// ── Non-Linux: tray-icon (macOS NSStatusBar / Windows Shell) ───────

#[cfg(not(target_os = "linux"))]
mod tray_imp {
    use super::*;
    use tray_icon;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIconBuilder};

    pub fn create_tray() -> (tray_icon::TrayIcon, MenuEventReceiver) {
        let menu = Menu::new();
        let show_item = MenuItem::with_id("show", "Show Window", true, None);
        let clickthrough_item =
            MenuItem::with_id("clickthrough", "Toggle Click-Through", true, None);
        let quit_item = MenuItem::with_id("quit", "Quit", true, None);
        menu.append_items(&[&show_item, &clickthrough_item, &quit_item])
            .ok();

        let (rgba, w, h) = embedded_icon_rgba();
        let icon = Icon::from_rgba(rgba, w, h).expect("create tray icon");
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Live2D Pet")
            .with_icon(icon)
            .build()
            .expect("build tray icon");

        let (tx, rx) = mpsc::channel();
        MenuEvent::set_event_handler(Some(Box::new(move |event: MenuEvent| {
            let id = event.id.0.clone();
            let _ = tx.send(id);
        })));

        (tray, MenuEventReceiver { rx })
    }
}

#[cfg(not(target_os = "linux"))]
pub use tray_imp::create_tray;
