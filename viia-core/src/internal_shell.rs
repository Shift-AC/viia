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

    /// Go to an image by file-list index. Omit index to show the current image.
    #[command(name = "g")]
    Goto { index: Option<usize> },

    /// Print file names in the current file list that match a regex pattern.
    #[command(name = "m")]
    Match {
        #[arg(allow_hyphen_values = true)]
        pattern: String,
    },

    /// Show the previous image
    #[command(name = "l")]
    ShowPrevious,

    /// Open a new set of files, directories, or URLs
    #[command(name = "o")]
    Open {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        targets: Vec<String>,
    },

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

pub fn zero_based_to_shell_index(index: usize) -> usize {
    index
        .checked_add(1)
        .expect("shell display index overflowed usize")
}

pub fn shell_index_to_zero_based(index: usize) -> Result<usize, String> {
    index
        .checked_sub(1)
        .ok_or_else(|| "File index must be at least 1".to_string())
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
            InternalCommand::parse_line("g").unwrap().action,
            RuntimeAction::Goto { index: None }
        );
        assert_eq!(
            InternalCommand::parse_line("g 3").unwrap().action,
            RuntimeAction::Goto { index: Some(3) }
        );
        assert_eq!(
            InternalCommand::parse_line("g 1").unwrap().action,
            RuntimeAction::Goto { index: Some(1) }
        );
        assert_eq!(
            InternalCommand::parse_line("g 0").unwrap().action,
            RuntimeAction::Goto { index: Some(0) }
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
    fn test_parse_match() {
        let cmd = InternalCommand::parse_line(r#"m "^cat.*\.png$""#).unwrap();
        assert_eq!(
            cmd.action,
            RuntimeAction::Match {
                pattern: "^cat.*\\.png$".to_string()
            }
        );
    }

    #[test]
    fn test_shell_index_conversion_helpers_use_one_based_indices() {
        assert_eq!(zero_based_to_shell_index(0), 1);
        assert_eq!(zero_based_to_shell_index(4), 5);
        assert_eq!(shell_index_to_zero_based(1).unwrap(), 0);
        assert_eq!(shell_index_to_zero_based(5).unwrap(), 4);
        assert_eq!(
            shell_index_to_zero_based(0).unwrap_err(),
            "File index must be at least 1"
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
