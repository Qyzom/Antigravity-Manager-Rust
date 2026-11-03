// Aroxy Autostart Management (Linux XDG & Windows Registry)

use std::path::PathBuf;

pub fn set_autostart(enable: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(config_dir) = dirs::config_dir() {
            let autostart_dir = config_dir.join("autostart");
            let desktop_file = autostart_dir.join("aroxy.desktop");

            if enable {
                let _ = std::fs::create_dir_all(&autostart_dir);
                let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aroxy"));
                let exe_str = current_exe.to_string_lossy();
                let entry_content = format!(
                    "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Aroxy\nComment=Aroxy AI Protocol Proxy Daemon\nExec=\"{}\" --start-minimized\nTerminal=false\nCategories=Utility;Development;\n",
                    exe_str
                );
                std::fs::write(&desktop_file, entry_content)
                    .map_err(|e| format!("Failed to create autostart entry: {}", e))?;
            } else if desktop_file.exists() {
                let _ = std::fs::remove_file(&desktop_file);
            }
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aroxy.exe"));
        let exe_str = current_exe.to_str().unwrap_or("aroxy.exe");
        let auto = auto_launch::AutoLaunchBuilder::new()
            .set_app_name("Aroxy")
            .set_app_path(exe_str)
            .set_args(&["--start-minimized"])
            .build();

        if let Ok(auto) = auto {
            if enable {
                let _ = auto.enable();
            } else {
                let _ = auto.disable();
            }
        }
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = enable;
        Ok(())
    }
}
