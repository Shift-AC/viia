use chrono::Utc;
use std::process::Command;

fn main() {
    // Re-run the build script if any of the Git files change
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    // Get the Git hash
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();

    let git_hash = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    };

    // Get the current build timestamp
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

    // Export variables to rustc
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", timestamp);

    tauri_build::build()
}
