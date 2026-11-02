use muda::{Menu, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Toggle,
    Exit,
}

#[allow(dead_code)]
pub struct SystemTray {
    _tray_icon: TrayIcon,
    show_item: MenuItem,
    exit_item: MenuItem,
}

impl SystemTray {
    pub fn new<F>(on_action: F) -> Result<Self, Box<dyn std::error::Error>>
    where
        F: Fn(TrayAction) + Send + Sync + 'static,
    {
        let tray_menu = Menu::new();
        let show_item = MenuItem::new("Show Aroxy", true, None);
        let show_item_id = show_item.id().clone();
        let exit_item = MenuItem::new("Exit", true, None);
        let exit_item_id = exit_item.id().clone();

        tray_menu.append(&show_item)?;
        tray_menu.append(&exit_item)?;

        let icon = create_default_icon()?;

        let tray_icon_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            TrayIconBuilder::new()
                .with_menu(Box::new(tray_menu))
                .with_tooltip("Aroxy — AI Proxy")
                .with_icon(icon)
                .build()
        }));

        let tray_icon = match tray_icon_res {
            Ok(Ok(ti)) => ti,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err("System tray service not available".into()),
        };

        let action_cb = std::sync::Arc::new(on_action);

        let cb_menu = std::sync::Arc::clone(&action_cb);
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            if event.id == show_item_id {
                cb_menu(TrayAction::Show);
            } else if event.id == exit_item_id {
                cb_menu(TrayAction::Exit);
            }
        }));

        let cb_tray = std::sync::Arc::clone(&action_cb);
        tray_icon::TrayIconEvent::set_event_handler(Some(move |event: tray_icon::TrayIconEvent| {
            match event {
                tray_icon::TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    cb_tray(TrayAction::Toggle);
                }
                tray_icon::TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    cb_tray(TrayAction::Show);
                }
                _ => {}
            }
        }));

        Ok(Self {
            _tray_icon: tray_icon,
            show_item,
            exit_item,
        })
    }
}

fn create_default_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        if let Ok(icon) = Icon::from_resource(1, Some((32, 32))) {
            return Ok(icon);
        }
    }

    let rgba_bytes = include_bytes!("../../assets/icons/tray_32x32.rgba");
    let icon = Icon::from_rgba(rgba_bytes.to_vec(), 32, 32)?;
    Ok(icon)
}
