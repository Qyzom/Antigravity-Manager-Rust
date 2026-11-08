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
    pub fn new<F>(
        show_text: &str,
        exit_text: &str,
        tooltip: &str,
        on_action: F,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        F: Fn(TrayAction) + Send + Sync + 'static,
    {
        let show_title = show_text.to_string();
        let exit_title = exit_text.to_string();
        let tip_title = tooltip.to_string();

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let tray_menu = Menu::new();
            let show_item = MenuItem::new(&show_title, true, None);
            let show_item_id = show_item.id().clone();
            let exit_item = MenuItem::new(&exit_title, true, None);
            let exit_item_id = exit_item.id().clone();

            tray_menu.append(&show_item).map_err(|e| e.to_string())?;
            tray_menu.append(&exit_item).map_err(|e| e.to_string())?;

            let icon = create_default_icon().map_err(|e| e.to_string())?;

            let tray_icon = TrayIconBuilder::new()
                .with_menu(Box::new(tray_menu))
                .with_tooltip(&tip_title)
                .with_icon(icon)
                .build()
                .map_err(|e| e.to_string())?;

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
            tray_icon::TrayIconEvent::set_event_handler(Some(
                move |event: tray_icon::TrayIconEvent| match event {
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
                },
            ));

            Ok::<SystemTray, String>(Self {
                _tray_icon: tray_icon,
                show_item,
                exit_item,
            })
        }));

        match res {
            Ok(Ok(st)) => Ok(st),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Err("System tray service not available".into()),
        }
    }

    pub fn update_labels(&self, show_text: &str, exit_text: &str, tooltip: &str) {
        self.show_item.set_text(show_text);
        self.exit_item.set_text(exit_text);
        let _ = self._tray_icon.set_tooltip(Some(tooltip));
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
