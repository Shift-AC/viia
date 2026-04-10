use clap::{Parser, Subcommand};

use std::str::FromStr;

#[derive(Clone, Debug, PartialEq)]
pub enum ZoomMode {
    Fit,
    Shrink,
    Fixed(f32),
}

impl FromStr for ZoomMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "fit" => Ok(ZoomMode::Fit),
            "shrink" => Ok(ZoomMode::Shrink),
            _ => {
                if let Ok(pct) = s.parse::<f32>() {
                    Ok(ZoomMode::Fixed(pct))
                } else {
                    Err(format!(
                        "Invalid zoom mode: {}. Expected 'fit', 'shrink', or a number.",
                        s
                    ))
                }
            }
        }
    }
}

/// The internal command processor for viia's UI/backend communication.
/// It uses a simple shell-like syntax to issue commands during runtime.
#[derive(Parser, Debug, PartialEq)]
#[command(no_binary_name = true)] // Internal shell commands don't have "viia" prefix
pub struct InternalCommand {
    #[command(subcommand)]
    pub action: RuntimeAction,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum RuntimeAction {
    /// Set window dimension (e.g., "d 800x600"). Omit value to use screen size.
    #[command(name = "d")]
    Dimension { dim: Option<String> },

    /// Show current image
    #[command(name = "e")]
    ShowCurrent,

    /// Show the previous image
    #[command(name = "l")]
    ShowPrevious,

    /// Pause/Resume the current slideshow
    #[command(name = "p")]
    TogglePause,

    /// Quit the program
    #[command(name = "q")]
    Quit,

    /// Show the next image
    #[command(name = "r")]
    ShowNext,

    /// Start a slideshow with a command string
    #[command(name = "s")]
    StartSlideshow {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },

    /// Set the zoom mode. Options: 'fit' (resize to fit window), 'shrink' (shrink if too large), or a fixed scale percentage (e.g., '150').
    #[command(name = "z")]
    Zoom {
        #[arg(default_value = "shrink")]
        mode: ZoomMode,
    },

    /// Print help information
    #[command(name = "h")]
    Help,
}

impl InternalCommand {
    /// Parses a string input into a structured internal command.
    /// Handles splitting by whitespace.
    pub fn parse_line(line: &str) -> Result<Self, clap::Error> {
        let mut args = shlex::split(line).unwrap_or_default();
        if let Some(first) = args.first_mut()
            && first == "h"
        {
            *first = "help".to_owned();
        }

        Self::try_parse_from(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_help() {
        let err = InternalCommand::parse_line("help").unwrap_err();
        println!("HELP OUTPUT:\n{}", err);
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);

        let err_h = InternalCommand::parse_line("h").unwrap_err();
        assert_eq!(err_h.kind(), clap::error::ErrorKind::DisplayHelp);
        assert_eq!(err.to_string(), err_h.to_string());

        let err_d = InternalCommand::parse_line("help d").unwrap_err();
        let err_hd = InternalCommand::parse_line("h d").unwrap_err();
        assert_eq!(err_d.to_string(), err_hd.to_string());

        let err_z = InternalCommand::parse_line("help z").unwrap_err();
        println!("HELP Z OUTPUT:\n{}", err_z);
    }

    #[test]
    fn test_parse_dimension() {
        let cmd = InternalCommand::parse_line("d 800x600").unwrap();
        assert_eq!(
            cmd.action,
            RuntimeAction::Dimension {
                dim: Some("800x600".to_string())
            }
        );

        let cmd = InternalCommand::parse_line("d").unwrap();
        assert_eq!(cmd.action, RuntimeAction::Dimension { dim: None });
    }

    #[test]
    fn test_parse_basic_actions() {
        assert_eq!(
            InternalCommand::parse_line("e").unwrap().action,
            RuntimeAction::ShowCurrent
        );
        assert_eq!(
            InternalCommand::parse_line("l").unwrap().action,
            RuntimeAction::ShowPrevious
        );
        assert_eq!(
            InternalCommand::parse_line("p").unwrap().action,
            RuntimeAction::TogglePause
        );
        assert_eq!(
            InternalCommand::parse_line("q").unwrap().action,
            RuntimeAction::Quit
        );
        assert_eq!(
            InternalCommand::parse_line("r").unwrap().action,
            RuntimeAction::ShowNext
        );
    }

    #[test]
    fn test_parse_slideshow() {
        let cmd = InternalCommand::parse_line("s L2T3, @file.txt").unwrap();
        if let RuntimeAction::StartSlideshow { cmd: args } = cmd.action {
            assert_eq!(args.join(" "), "L2T3, @file.txt");
        } else {
            panic!("Expected StartSlideshow");
        }
    }

    #[test]
    fn test_parse_scale() {
        let cmd = InternalCommand::parse_line("z 150").unwrap();
        assert_eq!(
            cmd.action,
            RuntimeAction::Zoom {
                mode: ZoomMode::Fixed(150.0)
            }
        );

        let cmd = InternalCommand::parse_line("z fit").unwrap();
        assert_eq!(
            cmd.action,
            RuntimeAction::Zoom {
                mode: ZoomMode::Fit
            }
        );

        let cmd = InternalCommand::parse_line("z shrink").unwrap();
        assert_eq!(
            cmd.action,
            RuntimeAction::Zoom {
                mode: ZoomMode::Shrink
            }
        );

        let cmd = InternalCommand::parse_line("z").unwrap();
        assert_eq!(
            cmd.action,
            RuntimeAction::Zoom {
                mode: ZoomMode::Shrink
            }
        );
    }
}
