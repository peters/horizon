//! Desktop pictures as egui images: session frames without a copy, a local
//! crop and scale of the desktop the viewer holds, and the empty-desktop check.
use egui::{Color32, ColorImage};
use horizon_vnc::{Frame, ViewOptions};

use super::session::ViewError;

/// A session frame as an egui image. Both keep four RGBA bytes per pixel with
/// the same alignment, and a session pixel is opaque or all zero, which is the
/// same color premultiplied or not; so the pixels move without a copy.
pub(super) fn color_image(frame: Frame) -> ColorImage {
    let size = frame.size();
    match bytemuck::allocation::try_cast_vec::<u32, Color32>(frame.into_pixels()) {
        Ok(pixels) => ColorImage::new(size, pixels),
        Err((_, pixels)) => ColorImage::from_rgba_unmultiplied(size, bytemuck::must_cast_slice(&pixels)),
    }
}

/// An egui image as a session frame, for fixtures that stand in for a server.
#[cfg(test)]
pub(super) fn frame(image: ColorImage) -> Frame {
    let pixels = bytemuck::allocation::try_cast_vec::<Color32, u32>(image.pixels)
        .map_err(|(error, _)| error)
        .expect("Color32 and u32 have the same size and alignment");
    Frame::new(image.size, pixels).expect("a ColorImage holds width × height pixels")
}

/// Crop and scale a full desktop image for local view controls.
pub(super) fn present_image(source: &ColorImage, options: ViewOptions) -> Result<ColorImage, ViewError> {
    let (size, pixels) = horizon_vnc::present(source.size, &source.pixels, options)?;
    Ok(ColorImage::new(size, pixels))
}

/// Whether a picture is one flat colour apart from a stray pointer or a clock: a desktop with
/// nothing open on it. It samples a fixed grid, so the cost does not grow with the desktop.
pub(super) fn looks_empty(image: &ColorImage) -> bool {
    const GRID: usize = 64;
    // A pointer or a small clock may differ; a window or a wallpaper would differ a lot more.
    const OTHER_PER_HUNDRED: usize = 2;
    let [width, height] = image.size;
    if width == 0 || height == 0 || image.pixels.len() != width * height {
        return false;
    }
    let (columns, rows) = (width.min(GRID), height.min(GRID));
    let samples = || {
        (0..rows).flat_map(move |row| {
            (0..columns).map(move |column| image.pixels[(row * height / rows) * width + column * width / columns])
        })
    };
    // The background is the colour most samples share (a majority vote in fixed memory), not
    // whatever sits in a corner, where a pointer or a clock may be.
    let (mut background, mut votes) = (image.pixels[0], 0_usize);
    for pixel in samples() {
        if votes == 0 {
            background = pixel;
        }
        votes = if pixel == background { votes + 1 } else { votes - 1 };
    }
    let other = samples().filter(|pixel| *pixel != background).count();
    other * 100 <= columns * rows * OTHER_PER_HUNDRED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_pixels_become_the_same_egui_colors_without_conversion() -> Result<(), ViewError> {
        // Drawn pixels are opaque; a pixel not drawn since a resize is all zero.
        let rgba = [20, 180, 40, 255, 0, 0, 0, 0];
        let image = color_image(Frame::from_rgba([2, 1], &rgba)?);
        assert_eq!(image, ColorImage::from_rgba_unmultiplied([2, 1], &rgba));
        assert_eq!(image.pixels, [Color32::from_rgb(20, 180, 40), Color32::TRANSPARENT]);
        assert_eq!(color_image(frame(image.clone())), image, "and back");
        Ok(())
    }

    #[test]
    fn a_held_desktop_is_cropped_and_scaled_in_egui_colors() -> Result<(), ViewError> {
        let options = ViewOptions {
            viewport: Some(horizon_vnc::Region {
                x: 2,
                y: 0,
                width: 2,
                height: 2,
            }),
            max_width: 1,
            max_height: 1,
            ..Default::default()
        };
        let presented = present_image(
            &ColorImage::new([4, 2], (0..8).map(|value| Color32::from_rgb(value, 0, 0)).collect()),
            options,
        )?;
        assert_eq!(presented.size, [1, 1]);
        assert_eq!(presented.pixels[0].r(), 7);
        Ok(())
    }
}
