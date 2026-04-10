use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ParserError {
    #[error("Invalid command format: {0}")]
    InvalidCommand(String),
    #[error("Failed to read file: {0}")]
    FileReadError(String),
}

/// Timing limits applied to a single image in a slideshow
#[derive(Debug, Clone, PartialEq)]
pub struct TimingCommand {
    /// Number of times to loop the animation (or static image duration multiplier)
    pub loops: Option<u32>,
    /// Minimum time in seconds to display the image
    pub time_secs: Option<f32>,
    /// Whether to loop eternally
    pub infinite: bool,
}

impl TimingCommand {
    /// Parses a single command like "L1T2" or "L2" or "T0.5" or "INF"
    fn parse_single(cmd: &str) -> Result<Self, ParserError> {
        let cmd = cmd.trim().to_lowercase();
        if cmd.is_empty() {
            return Err(ParserError::InvalidCommand("Empty command".to_string()));
        }

        if cmd == "inf" {
            return Ok(Self {
                loops: None,
                time_secs: None,
                infinite: true,
            });
        }

        let mut loops = None;
        let mut time_secs = None;

        // Simple parsing logic assuming format L[num]T[num] or variations
        let mut chars = cmd.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                'l' => {
                    let mut num_str = String::new();
                    while let Some(&next_c) = chars.peek() {
                        if next_c.is_ascii_digit() {
                            num_str.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    if num_str.is_empty() {
                        return Err(ParserError::InvalidCommand(format!(
                            "Missing number after 'L' in '{}'",
                            cmd
                        )));
                    }
                    loops = Some(num_str.parse::<u32>().map_err(|_| {
                        ParserError::InvalidCommand(format!(
                            "Invalid number for loops: {}",
                            num_str
                        ))
                    })?);
                }
                't' => {
                    let mut num_str = String::new();
                    while let Some(&next_c) = chars.peek() {
                        if next_c.is_ascii_digit() || next_c == '.' {
                            num_str.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    if num_str.is_empty() {
                        return Err(ParserError::InvalidCommand(format!(
                            "Missing number after 'T' in '{}'",
                            cmd
                        )));
                    }
                    time_secs = Some(num_str.parse::<f32>().map_err(|_| {
                        ParserError::InvalidCommand(format!("Invalid number for time: {}", num_str))
                    })?);
                }
                _ => {
                    return Err(ParserError::InvalidCommand(format!(
                        "Unexpected character '{}' in '{}'",
                        c, cmd
                    )));
                }
            }
        }

        if loops.is_none() && time_secs.is_none() {
            return Err(ParserError::InvalidCommand(format!(
                "No valid timing specified in '{}'",
                cmd
            )));
        }

        Ok(Self {
            loops,
            time_secs,
            infinite: false,
        })
    }

    /// Computes the effective duration for an image given its total animation loop duration.
    /// It picks the longest required duration based on `loops` or `time_secs`.
    pub fn calculate_effective_duration(&self, loop_duration: Duration) -> Duration {
        if self.infinite {
            return Duration::MAX;
        }

        let loop_req = match self.loops {
            Some(l) => loop_duration * l,
            None => Duration::ZERO,
        };

        let time_req = match self.time_secs {
            Some(t) => Duration::from_secs_f32(t),
            None => Duration::ZERO,
        };

        std::cmp::max(loop_req, time_req)
    }
}

/// Parses a full slideshow specification string, including file imports and comma-separated lists.
pub fn parse_slideshow_spec(
    spec: &str,
    base_dir: &std::path::Path,
) -> Result<Vec<TimingCommand>, ParserError> {
    // First, remove all whitespaces from the string as per the specification
    let clean_spec: String = spec.chars().filter(|c| !c.is_whitespace()).collect();

    let mut commands = Vec::new();

    for part in clean_spec.split(',') {
        if part.is_empty() {
            continue;
        }

        if let Some(filename) = part.strip_prefix('@') {
            // Read from file
            let mut file_path = PathBuf::from(base_dir);
            file_path.push(filename);

            let content = fs::read_to_string(&file_path).map_err(|e| {
                ParserError::FileReadError(format!("Failed to read {}: {}", file_path.display(), e))
            })?;

            // Recursively parse the file content
            let file_commands = parse_slideshow_spec(&content, base_dir)?;
            commands.extend(file_commands);
        } else {
            // Parse single command
            commands.push(TimingCommand::parse_single(part)?);
        }
    }

    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_single() {
        assert_eq!(
            TimingCommand::parse_single("L1T2").unwrap(),
            TimingCommand {
                loops: Some(1),
                time_secs: Some(2.0),
                infinite: false
            }
        );
        assert_eq!(
            TimingCommand::parse_single("l2t3.5").unwrap(),
            TimingCommand {
                loops: Some(2),
                time_secs: Some(3.5),
                infinite: false
            }
        );
        assert_eq!(
            TimingCommand::parse_single("T5").unwrap(),
            TimingCommand {
                loops: None,
                time_secs: Some(5.0),
                infinite: false
            }
        );
        assert_eq!(
            TimingCommand::parse_single("L3").unwrap(),
            TimingCommand {
                loops: Some(3),
                time_secs: None,
                infinite: false
            }
        );
        assert_eq!(
            TimingCommand::parse_single("t2L1").unwrap(),
            TimingCommand {
                loops: Some(1),
                time_secs: Some(2.0),
                infinite: false
            }
        );
        assert_eq!(
            TimingCommand::parse_single("inf").unwrap(),
            TimingCommand {
                loops: None,
                time_secs: None,
                infinite: true
            }
        );
    }

    #[test]
    fn test_parse_invalid() {
        assert!(TimingCommand::parse_single("L").is_err());
        assert!(TimingCommand::parse_single("TX").is_err());
        assert!(TimingCommand::parse_single("X1").is_err());
    }

    #[test]
    fn test_calculate_effective_duration() {
        let loop_dur = Duration::from_millis(500); // 0.5 seconds

        // L2 = 1.0s, T2.0 = 2.0s. Max is 2.0s
        let cmd = TimingCommand {
            loops: Some(2),
            time_secs: Some(2.0),
            infinite: false,
        };
        assert_eq!(
            cmd.calculate_effective_duration(loop_dur),
            Duration::from_secs(2)
        );

        // L10 = 5.0s, T2.0 = 2.0s. Max is 5.0s
        let cmd = TimingCommand {
            loops: Some(10),
            time_secs: Some(2.0),
            infinite: false,
        };
        assert_eq!(
            cmd.calculate_effective_duration(loop_dur),
            Duration::from_secs(5)
        );

        // T3.0 only
        let cmd = TimingCommand {
            loops: None,
            time_secs: Some(3.0),
            infinite: false,
        };
        assert_eq!(
            cmd.calculate_effective_duration(loop_dur),
            Duration::from_secs(3)
        );

        // INF
        let cmd = TimingCommand {
            loops: None,
            time_secs: None,
            infinite: true,
        };
        assert_eq!(cmd.calculate_effective_duration(loop_dur), Duration::MAX);
    }

    #[test]
    fn test_parse_slideshow_spec() {
        let dir = tempfile::tempdir().unwrap();

        let spec = "L1T2,  L2T3,L3 "; // spaces should be ignored
        let cmds = parse_slideshow_spec(spec, dir.path()).unwrap();

        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0],
            TimingCommand {
                loops: Some(1),
                time_secs: Some(2.0),
                infinite: false
            }
        );
        assert_eq!(
            cmds[1],
            TimingCommand {
                loops: Some(2),
                time_secs: Some(3.0),
                infinite: false
            }
        );
        assert_eq!(
            cmds[2],
            TimingCommand {
                loops: Some(3),
                time_secs: None,
                infinite: false
            }
        );
    }

    #[test]
    fn test_parse_slideshow_spec_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("spec.txt");

        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "L5, T10").unwrap();

        let spec = format!(
            "L1, @{}, L2",
            file_path.file_name().unwrap().to_str().unwrap()
        );
        let cmds = parse_slideshow_spec(&spec, dir.path()).unwrap();

        assert_eq!(cmds.len(), 4);
        assert_eq!(
            cmds[0],
            TimingCommand {
                loops: Some(1),
                time_secs: None,
                infinite: false
            }
        );
        assert_eq!(
            cmds[1],
            TimingCommand {
                loops: Some(5),
                time_secs: None,
                infinite: false
            }
        );
        assert_eq!(
            cmds[2],
            TimingCommand {
                loops: None,
                time_secs: Some(10.0),
                infinite: false
            }
        );
        assert_eq!(
            cmds[3],
            TimingCommand {
                loops: Some(2),
                time_secs: None,
                infinite: false
            }
        );
    }
}
