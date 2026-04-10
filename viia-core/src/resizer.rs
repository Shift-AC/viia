use fast_image_resize as fr;
use image::RgbaImage;
use std::num::NonZeroU32;

/// Resizes an image to the target dimensions while maintaining aspect ratio
pub fn resize_image(
    source: &RgbaImage,
    target_width: u32,
    target_height: u32,
) -> Option<RgbaImage> {
    let src_width = NonZeroU32::new(source.width())?;
    let src_height = NonZeroU32::new(source.height())?;

    // Calculate aspect ratio preserving dimensions
    let aspect_ratio = src_width.get() as f32 / src_height.get() as f32;
    let target_aspect_ratio = target_width as f32 / target_height as f32;

    let (final_width, final_height) = if aspect_ratio > target_aspect_ratio {
        (target_width, (target_width as f32 / aspect_ratio) as u32)
    } else {
        ((target_height as f32 * aspect_ratio) as u32, target_height)
    };

    let dst_width = NonZeroU32::new(final_width)?;
    let dst_height = NonZeroU32::new(final_height)?;

    let mut src_image = fr::images::Image::from_vec_u8(
        src_width.into(),
        src_height.into(),
        source.clone().into_raw(),
        fr::PixelType::U8x4,
    )
    .ok()?;

    let mut dst_image =
        fr::images::Image::new(dst_width.into(), dst_height.into(), fr::PixelType::U8x4);

    // Multiple images from the same image using the same alpha channel
    let alpha_mul_div = fr::MulDiv::default();
    alpha_mul_div.multiply_alpha_inplace(&mut src_image).ok()?;

    let mut resizer = fr::Resizer::new();

    // Explicitly enable rayon multithreading in resize options
    let options = fr::ResizeOptions::new()
        .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear))
        .use_alpha(false); // We handle alpha multiplication manually

    // fast_image_resize automatically uses rayon if the feature is enabled,
    // but we can configure thread pool / chunks if we want. By default,
    // enabling the feature makes the standard `.resize` method multithreaded.
    resizer.resize(&src_image, &mut dst_image, &options).ok()?;

    alpha_mul_div.divide_alpha_inplace(&mut dst_image).ok()?;

    let buffer = dst_image.into_vec();
    RgbaImage::from_raw(dst_width.get(), dst_height.get(), buffer)
}

#[cfg(test)]
#[path = "resizer_tests.rs"]
mod tests;
