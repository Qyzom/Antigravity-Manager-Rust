use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "amr",
    version = env!("CARGO_PKG_VERSION"),
    about = "AMR (Antigravity Manager Rust) — Ultra-lightweight AI API Proxy & Emulator"
)]
pub struct CliArgs {
    /// Run proxy in headless mode without graphical UI
    #[arg(long)]
    pub headless: bool,

    /// Override local listening port
    #[arg(short, long)]
    pub port: Option<u16>,

    /// Start minimized directly to system tray
    #[arg(long)]
    pub start_minimized: bool,

    /// Custom config file path
    #[arg(short, long)]
    pub config: Option<String>,

    /// Capture UI screenshots in English to target directory and exit
    #[arg(long)]
    pub take_screenshots: Option<String>,
}
