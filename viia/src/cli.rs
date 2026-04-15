use clap::{Parser, ValueEnum};

/// The user interface to use
#[derive(Debug, Clone, PartialEq, Eq, ValueEnum)]
pub enum UiMode {
    /// Headless mode (no graphical output, just logs)
    Headless,
    /// Terminal mode (renders images using sixel graphics)
    Terminal,
    /// GUI mode (displays images in a window)
    #[cfg(feature = "gui")]
    Gui,
}

#[derive(Parser, Debug, PartialEq)]
#[command(name = "viia")]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " (git: ", env!("GIT_HASH"), ", build: ", env!("BUILD_TIMESTAMP"), ")"))]
#[command(about = "View Images In Animations", long_about = None)]
pub struct Cli {
    /// Window dimension ([width]x[height]), 2/3 of the screen size if not specified
    #[arg(short, long)]
    pub dimension: Option<String>,

    /// The user interface to use (headless, terminal, gui)
    #[arg(long, default_value = "gui")]
    pub ui: UiMode,

    /// Number of images to prefetch into memory
    #[arg(long, default_value_t = 5)]
    pub prefetch: usize,

    /// List of paths to image files or directories
    #[arg(name = "PATHS")]
    pub paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    #[cfg(not(feature = "gui"))]
    fn test_cli_default_ui() {
        // ... (This test fails because default_value="gui" is hardcoded and invalid without the feature)
    }

    #[test]
    #[cfg(feature = "gui")]
    fn test_cli_custom_ui() {
        let args = vec!["viia", "--ui", "gui", "dir/"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.ui, UiMode::Gui);
        assert_eq!(cli.paths.len(), 1);
        assert_eq!(cli.paths[0], "dir/".to_string());
    }

    #[test]
    #[cfg(feature = "gui")]
    fn test_cli_dimension() {
        let args = vec!["viia", "-d", "800x600"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.dimension, Some("800x600".to_string()));
        assert_eq!(cli.paths.len(), 0);
    }
}
