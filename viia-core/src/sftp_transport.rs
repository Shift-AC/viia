use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, info, warn};
use url::Url;

use crate::MediaUrl;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status_code: i32,
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[String], stdin: &str) -> Result<CommandOutput, String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessCommandRunner;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

impl CommandRunner for ProcessCommandRunner {
    fn run(&self, program: &str, args: &[String], stdin: &str) -> Result<CommandOutput, String> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", program, e))?;

        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(stdin.as_bytes())
                .map_err(|e| format!("Failed to write batch input to {}: {}", program, e))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("Failed waiting for {}: {}", program, e))?;

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingOrder {
    ProtocolDefault,
    LexicographicFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    pub entries: Vec<String>,
    pub order: ListingOrder,
}

pub fn list_directory(url: &MediaUrl) -> Result<DirListing, String> {
    list_directory_with_runner(url, &ProcessCommandRunner)
}

pub fn download_file(url: &MediaUrl) -> Result<Vec<u8>, String> {
    download_file_with_runner(url, &ProcessCommandRunner)
}

pub fn get_metadata(url: &MediaUrl) -> Result<String, String> {
    get_metadata_with_runner(url, &ProcessCommandRunner)
}

fn get_metadata_with_runner(url: &MediaUrl, runner: &dyn CommandRunner) -> Result<String, String> {
    let parsed = url
        .to_url()
        .map_err(|e| format!("Invalid SFTP URL '{}': {}", url.as_str(), e))?;
    ensure_sftp_scheme(&parsed)?;

    let parent_url = url
        .parent()
        .ok_or_else(|| format!("SFTP URL has no parent directory: {}", url.as_str()))?;
    let parsed_parent = parent_url.to_url().unwrap();

    let file_name = url
        .file_name()
        .ok_or_else(|| format!("SFTP URL has no file name: {}", url.as_str()))?;
    let batch = format!("@ls -l {}\n", quote_sftp_arg(&file_name));

    debug!(
        "Getting metadata for SFTP file {} via parent URL {}",
        url.as_str(),
        parent_url.as_str()
    );
    let output = match run_sftp_batch(runner, &parsed_parent, &batch) {
        Ok(out) => out,
        Err(e) if e.contains("not found") => {
            return Err(format!("File not found: {}", url.as_str()));
        }
        Err(e) => return Err(e),
    };

    let line = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("sftp>"))
        .rfind(|line| !line.starts_with("Connected to "))
        .ok_or_else(|| format!("Could not get metadata for {}", url.as_str()))?;

    Ok(line.to_string())
}

fn list_directory_with_runner(
    url: &MediaUrl,
    runner: &dyn CommandRunner,
) -> Result<DirListing, String> {
    let parsed = url
        .to_url()
        .map_err(|e| format!("Invalid SFTP URL '{}': {}", url.as_str(), e))?;
    ensure_sftp_scheme(&parsed)?;

    let batch = "@ls -1\n";
    debug!("Listing SFTP directory via URL {}", url.as_str());
    let output = match run_sftp_batch(runner, &parsed, batch) {
        Ok(out) => out,
        Err(e) if e.contains("not found") => {
            return Err(format!("Directory not found: {}", url.as_str()));
        }
        Err(e) => return Err(e),
    };
    let mut entries = parse_ls_output(&output.stdout);
    let order = if entries.is_empty() {
        ListingOrder::LexicographicFallback
    } else {
        ListingOrder::ProtocolDefault
    };

    if matches!(order, ListingOrder::LexicographicFallback) {
        entries.sort_by(|a, b| natord::compare_ignore_case(a, b));
        warn!(
            "Falling back to lexicographic order for SFTP listing at {}",
            url.as_str()
        );
    } else {
        info!(
            "Using protocol-default SFTP listing order for {}",
            url.as_str()
        );
    }

    Ok(DirListing { entries, order })
}

fn download_file_with_runner(
    url: &MediaUrl,
    runner: &dyn CommandRunner,
) -> Result<Vec<u8>, String> {
    let parsed = url
        .to_url()
        .map_err(|e| format!("Invalid SFTP URL '{}': {}", url.as_str(), e))?;
    ensure_sftp_scheme(&parsed)?;

    // Use destination URL that includes the directory, letting OpenSSH sftp "cd" into it.
    // Then `get` the basename as a relative path, because many servers treat `/` differently
    // than the default login directory root.
    let parent_url = url
        .parent()
        .ok_or_else(|| format!("SFTP URL has no parent directory: {}", url.as_str()))?;
    let parsed_parent = parent_url.to_url().unwrap();

    let file_name = url
        .file_name()
        .ok_or_else(|| format!("SFTP URL has no file name: {}", url.as_str()))?;
    let temp_path = unique_temp_download_path();
    let local_arg = temp_path.to_string_lossy().replace('\\', "/");
    let batch = format!(
        "@get {} {}\n",
        quote_sftp_arg(&file_name),
        quote_sftp_arg(&local_arg)
    );

    debug!(
        "Downloading SFTP file {} via parent URL {}",
        url.as_str(),
        parent_url.as_str()
    );
    if let Err(e) = run_sftp_batch(runner, &parsed_parent, &batch) {
        if e.contains("not found") {
            return Err(format!("File not found: {}", url.as_str()));
        }
        return Err(e);
    }

    let bytes = fs::read(&temp_path).map_err(|e| {
        format!(
            "Failed to read downloaded temp file {}: {}",
            temp_path.display(),
            e
        )
    })?;
    let _ = fs::remove_file(&temp_path);
    Ok(bytes)
}

fn run_sftp_batch(
    runner: &dyn CommandRunner,
    url: &Url,
    batch: &str,
) -> Result<CommandOutput, String> {
    let mut args = vec![
        "-q".to_string(),
        "-b".to_string(),
        "-".to_string(),
        "-oBatchMode=yes".to_string(),
        "-oNumberOfPasswordPrompts=0".to_string(),
        "-oPasswordAuthentication=no".to_string(),
        "-oKbdInteractiveAuthentication=no".to_string(),
    ];

    if let Some(port) = url.port() {
        args.push("-P".to_string());
        args.push(port.to_string());
    }

    let mut dest = String::new();
    let user = url.username();
    if !user.is_empty() {
        dest.push_str(user);
        dest.push('@');
    }
    if let Some(host) = url.host_str() {
        dest.push_str(host);
    }
    dest.push(':');

    let path = url.path();
    if let Some(stripped) = path.strip_prefix("//") {
        dest.push('/'); // Keep one slash for absolute
        dest.push_str(stripped);
    } else if let Some(stripped) = path.strip_prefix("/~/") {
        // sftp treats paths without a leading slash as relative to home directory,
        // so we just append the stripped path without the "~/"
        dest.push_str(stripped);
    } else if let Some(stripped) = path.strip_prefix('/') {
        dest.push_str(stripped); // Treat as relative
    } else {
        dest.push_str(path);
    }

    args.push(dest);

    let output = runner.run("sftp", &args, batch)?;
    if output.status_code != 0 {
        return Err(format!(
            "sftp failed (status {}): {}",
            output.status_code,
            output.stderr.trim()
        ));
    }
    Ok(output)
}

fn ensure_sftp_scheme(url: &Url) -> Result<(), String> {
    if url.scheme() != "sftp" {
        return Err(format!(
            "Expected sftp:// URL, got scheme '{}'",
            url.scheme()
        ));
    }
    Ok(())
}

fn quote_sftp_arg(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':'))
    {
        return value.to_string();
    }

    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

fn parse_ls_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("sftp>"))
        .filter(|line| !line.starts_with("Connected to "))
        .map(|line| line.trim_end_matches('/').to_string())
        .collect()
}

fn unique_temp_download_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    path.push(format!("viia-sftp-{}-{}.tmp", pid, nanos));
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Invocation = (String, Vec<String>, String);

    #[derive(Clone, Default)]
    struct FakeRunner {
        outputs: Arc<Mutex<Vec<Result<CommandOutput, String>>>>,
        invocations: Arc<Mutex<Vec<Invocation>>>,
    }

    impl FakeRunner {
        fn with_output(output: Result<CommandOutput, String>) -> Self {
            Self {
                outputs: Arc::new(Mutex::new(vec![output])),
                invocations: Arc::new(Mutex::new(vec![])),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &self,
            program: &str,
            args: &[String],
            stdin: &str,
        ) -> Result<CommandOutput, String> {
            self.invocations.lock().unwrap().push((
                program.to_string(),
                args.to_vec(),
                stdin.to_string(),
            ));
            self.outputs.lock().unwrap().remove(0)
        }
    }

    #[test]
    fn test_parse_ls_output() {
        let parsed = parse_ls_output("a.png\nb.gif\n");
        assert_eq!(parsed, vec!["a.png".to_string(), "b.gif".to_string()]);
    }

    #[test]
    fn test_list_directory_protocol_order() {
        let runner = FakeRunner::with_output(Ok(CommandOutput {
            stdout: "b.png\na.jpg\n".to_string(),
            stderr: String::new(),
            status_code: 0,
        }));
        let url = MediaUrl::parse_url("sftp://example.com/path/").unwrap();
        let listing = list_directory_with_runner(&url, &runner).unwrap();
        assert_eq!(
            listing.entries,
            vec!["b.png".to_string(), "a.jpg".to_string()]
        );
        assert_eq!(listing.order, ListingOrder::ProtocolDefault);
    }

    #[test]
    fn test_quote_sftp_arg_quotes_spaces() {
        assert_eq!(
            quote_sftp_arg("C:/Users/Admin/file name.png"),
            "\"C:/Users/Admin/file name.png\""
        );
    }

    #[test]
    fn test_run_sftp_batch_args() {
        let runner = FakeRunner::with_output(Ok(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            status_code: 0,
        }));

        let url = Url::parse("sftp://user@example.com:2222/path/to/dir").unwrap();
        run_sftp_batch(&runner, &url, "@ls -1\n").unwrap();

        let invocations = runner.invocations.lock().unwrap();
        let args = &invocations[0].1;
        assert!(args.contains(&"-P".to_string()));
        assert!(args.contains(&"2222".to_string()));
        assert_eq!(args.last().unwrap(), "user@example.com:path/to/dir");

        // Test absolute path
        let runner = FakeRunner::with_output(Ok(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            status_code: 0,
        }));
        let url = Url::parse("sftp://example.com//absolute/path").unwrap();
        run_sftp_batch(&runner, &url, "").unwrap();
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(
            invocations[0].1.last().unwrap(),
            "example.com:/absolute/path"
        );

        // Test home relative path
        let runner = FakeRunner::with_output(Ok(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            status_code: 0,
        }));
        let url = Url::parse("sftp://example.com/~/relative/path").unwrap();
        run_sftp_batch(&runner, &url, "").unwrap();
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(
            invocations[0].1.last().unwrap(),
            "example.com:relative/path"
        );
    }
}
