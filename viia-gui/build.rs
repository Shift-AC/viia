use chrono::Utc;
use ico::{IconDir, IconDirEntry, IconImage, ResourceType};
use image::imageops::{FilterType, overlay};
use image::{DynamicImage, Rgba, RgbaImage};
use std::fs::File;
use std::path::Path;
use std::process::Command;

fn square_logo(source: &Path) -> Result<RgbaImage, Box<dyn std::error::Error>> {
    let image = image::open(source)?.into_rgba8();
    let size = image.width().max(image.height());
    let mut canvas = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let x = ((size - image.width()) / 2) as i64;
    let y = ((size - image.height()) / 2) as i64;
    overlay(&mut canvas, &image, x, y);
    Ok(canvas)
}

fn write_png(
    image: &RgbaImage,
    size: u32,
    target: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let resized = image::imageops::resize(image, size, size, FilterType::Lanczos3);
    DynamicImage::ImageRgba8(resized).save(target)?;
    Ok(())
}

fn generate_icons() -> Result<(), Box<dyn std::error::Error>> {
    let source = Path::new("../logo/logo.png");
    let icons_dir = Path::new("icons");
    let square = square_logo(source)?;

    println!("cargo:rerun-if-changed={}", source.display());

    for (file_name, size) in [
        ("32x32.png", 32),
        ("64x64.png", 64),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("256x256.png", 256),
        ("icon.png", 512),
    ] {
        write_png(&square, size, &icons_dir.join(file_name))?;
    }

    let mut icon_dir = IconDir::new(ResourceType::Icon);
    for size in [256, 128, 96, 64, 48, 40, 32, 24, 20, 16] {
        let resized = image::imageops::resize(&square, size, size, FilterType::Lanczos3);
        let icon_image = IconImage::from_rgba_data(size, size, resized.into_raw());
        icon_dir.add_entry(IconDirEntry::encode(&icon_image)?);
    }

    let mut icon_file = File::create(icons_dir.join("icon.ico"))?;
    icon_dir.write(&mut icon_file)?;

    Ok(())
}

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

    generate_icons().expect("failed to generate Tauri icon assets");

    tauri_build::build()
}
