use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "aroxy",
    version = "0.1.0",
    about = "Ultra-lightweight AI API proxy & protocol emulator"
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
}
