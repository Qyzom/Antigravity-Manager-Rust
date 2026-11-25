use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

fn port_file_path() -> PathBuf {
    let base = dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("amr");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("instance.port")
}

pub struct SingleInstanceGuard {
    listener: Option<TcpListener>,
    port_file: PathBuf,
    #[cfg(windows)]
    mutex_handle: isize,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.port_file);
        #[cfg(windows)]
        if self.mutex_handle != 0 {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.mutex_handle as _);
            }
        }
    }
}

pub enum InstanceAcquireResult {
    Primary(SingleInstanceGuard),
    AlreadyRunning,
}

fn try_signal_existing(port_file: &PathBuf, show_window: bool) -> bool {
    let Ok(content) = std::fs::read_to_string(port_file) else {
        return false;
    };
    let Ok(port) = content.trim().parse::<u16>() else {
        return false;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
            AllowSetForegroundWindow(ASFW_ANY);
        }
        let cmd = if show_window { b"SHOW\n" } else { b"PING\n" };
        let _ = stream.write_all(cmd);
        let mut buf = [0u8; 16];
        if let Ok(n) = stream.read(&mut buf) {
            return &buf[..n] == b"AMR_OK\n";
        }
        return true;
    }
    false
}

/// Acquire single-instance ownership. If another instance is already running,
/// optionally ask it to show/focus its window and return `AlreadyRunning`.
pub fn acquire_single_instance(show_if_running: bool) -> InstanceAcquireResult {
    let port_file = port_file_path();

    #[cfg(windows)]
    let mutex_handle: isize = {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name: Vec<u16> = "Local\\AMR_SingleInstance_Mutex\0".encode_utf16().collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if !handle.is_null() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // Another instance holds the mutex; signal it to raise its window
            for _ in 0..5 {
                if try_signal_existing(&port_file, show_if_running) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return InstanceAcquireResult::AlreadyRunning;
        }
        handle as isize
    };

    #[cfg(not(windows))]
    {
        if try_signal_existing(&port_file, show_if_running) {
            return InstanceAcquireResult::AlreadyRunning;
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").ok();
    if let Some(ref l) = listener {
        if let Ok(addr) = l.local_addr() {
            let _ = std::fs::write(&port_file, addr.port().to_string());
        }
    }

    InstanceAcquireResult::Primary(SingleInstanceGuard {
        listener,
        port_file,
        #[cfg(windows)]
        mutex_handle,
    })
}

impl SingleInstanceGuard {
    /// Spawn a background listener thread that invokes `on_show` whenever
    /// a secondary launch requests the existing instance to come to the foreground.
    pub fn start_activation_listener<F>(&mut self, on_show: F)
    where
        F: Fn() + Send + 'static,
    {
        let Some(listener) = self.listener.take() else {
            return;
        };
        std::thread::Builder::new()
            .name("amr-instance-ipc".to_string())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut stream = stream;
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                    let mut buf = [0u8; 32];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let msg = std::str::from_utf8(&buf[..n]).unwrap_or("").trim();
                    let _ = stream.write_all(b"AMR_OK\n");
                    if msg == "SHOW" {
                        on_show();
                    }
                }
            })
            .ok();
    }
}
