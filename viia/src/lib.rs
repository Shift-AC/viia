pub mod cli;
pub mod headless;

use clap::Parser;
use cli::Cli;
use tracing::{Level, info};
use tracing_subscriber::{filter::EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn run() {
    // Parse command line arguments first to determine UI mode
    let cli = Cli::parse();

    #[cfg(all(windows, feature = "gui"))]
    if cli.ui == cli::UiMode::Gui {
        unsafe {
            use windows_sys::Win32::System::Console::{GetConsoleProcessList, FreeConsole};
            let mut pids = [0; 2];
            let num = GetConsoleProcessList(pids.as_mut_ptr(), 2);
            // If num is 1, this process is the only one attached to the console.
            // This happens when started from outside a terminal (e.g. Explorer).
            if num == 1 {
                FreeConsole();
            }
        }
    }

    let env_filter = EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env_lossy();

    if cli.ui == cli::UiMode::Terminal {
        // Set up tracing for TUI mode
        tui_logger::init_logger(log::LevelFilter::Trace).unwrap();
        tui_logger::set_default_level(log::LevelFilter::Trace);

        let tui_layer = tui_logger::TuiTracingSubscriberLayer;
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tui_layer)
            .init();
    } else {
        // Set up tracing for other modes
        let fmt_layer = tracing_subscriber::fmt::layer();
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
    }

    info!("Starting viia...");
    info!("Selected UI mode: {:?}", cli.ui);

    if let Some(dim) = &cli.dimension {
        info!("Window dimension specified: {}", dim);
    } else {
        info!("No dimension specified, using default (2/3 of screen size)");
    }

    info!("Input paths: {:?}", cli.paths);

    // Run the selected UI
    match cli.ui {
        cli::UiMode::Terminal => {
            if let Err(e) = viia_tui::run_tui(cli.paths, cli.prefetch) {
                tracing::error!("TUI encountered an error: {}", e);
            }
        }
        cli::UiMode::Headless => {
            if let Err(e) = headless::run_headless(cli.paths, cli.prefetch) {
                tracing::error!("Headless mode encountered an error: {}", e);
            }
        }
        #[cfg(feature = "gui")]
        cli::UiMode::Gui => {
            if let Err(e) = viia_gui::run_gui(cli.paths, cli.dimension, cli.prefetch) {
                tracing::error!("GUI mode encountered an error: {}", e);
            }
        }
    }
}
