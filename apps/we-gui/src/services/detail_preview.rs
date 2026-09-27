use std::{fs, io::BufReader, path::Path};

use iced::widget::image;
use image_rs::{
    imageops::{self, FilterType},
    ImageReader, Limits, Rgba, RgbaImage,
};
use we_core::{
    config::ScaleMode,
    presentation::compute_presentation_geometry,
    wallpaper::settings::{
        RenderResolution, Rotation, VisualAdjustments, WallpaperFillMode, WallpaperSettings,
    },
};

const PREVIEW_MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const PREVIEW_MAX_SOURCE_DIMENSION: u32 = 4_096;
const PREVIEW_MAX_ALLOC: u64 = 96 * 1024 * 1024;
const PREVIEW_CANVAS_MAX_WIDTH: u32 = 480;
const PREVIEW_CANVAS_MAX_HEIGHT: u32 = 270;
const DRAG_PREVIEW_CANVAS_MAX_WIDTH: u32 = 240;
const DRAG_PREVIEW_CANVAS_MAX_HEIGHT: u32 = 135;
const DEFAULT_TARGET_WIDTH: u32 = 1920;
const DEFAULT_TARGET_HEIGHT: u32 = 1080;

#[derive(Debug, Clone)]
pub(crate) struct DetailPreviewSource {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

pub(crate) fn load(path: &Path) -> Result<DetailPreviewSource, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect preview {}: {error}", path.display()))?;

    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("preview is not a regular file: {}", path.display()));
    }
    if metadata.len() == 0 || metadata.len() > PREVIEW_MAX_INPUT_BYTES {
        return Err("preview is empty or exceeds the bounded preview input limit".to_string());
    }

    let file = fs::File::open(path)
        .map_err(|error| format!("cannot open preview {}: {error}", path.display()))?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| format!("cannot determine preview format: {error}"))?;

    let mut limits = Limits::default();
    limits.max_image_width = Some(PREVIEW_MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(PREVIEW_MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(PREVIEW_MAX_ALLOC);
    reader.limits(limits);

    let decoded =
        reader.decode().map_err(|error| format!("cannot decode imported preview: {error}"))?;
    let bounded = decoded.thumbnail(PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT);
    let rgba = bounded.to_rgba8();

    Ok(DetailPreviewSource { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}

pub(crate) fn render(
    source: &DetailPreviewSource,
    settings: &WallpaperSettings,
) -> Result<image::Handle, String> {
    render_with_extent(source, settings, PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT)
}

pub(crate) fn render_drag(
    source: &DetailPreviewSource,
    settings: &WallpaperSettings,
) -> Result<image::Handle, String> {
    render_with_extent(
        source,
        settings,
        DRAG_PREVIEW_CANVAS_MAX_WIDTH,
        DRAG_PREVIEW_CANVAS_MAX_HEIGHT,
    )
}

fn render_with_extent(
    source: &DetailPreviewSource,
    settings: &WallpaperSettings,
    max_width: u32,
    max_height: u32,
) -> Result<image::Handle, String> {
    let (width, height, rgba) = compose(source, settings, max_width, max_height)?;
    Ok(image::Handle::from_rgba(width, height, rgba))
}

fn compose(
    source: &DetailPreviewSource,
    settings: &WallpaperSettings,
    max_width: u32,
    max_height: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    let source_image = RgbaImage::from_raw(source.width, source.height, source.rgba.clone())
        .ok_or_else(|| "cached imported preview pixels are invalid".to_string())?;

    let rotated = match settings.rotation_degrees {
        Rotation::Deg0 => source_image,
        Rotation::Deg90 => imageops::rotate90(&source_image),
        Rotation::Deg180 => imageops::rotate180(&source_image),
        Rotation::Deg270 => imageops::rotate270(&source_image),
    };

    let (target_width, target_height) = match settings.render_resolution {
        RenderResolution::Automatic => (DEFAULT_TARGET_WIDTH, DEFAULT_TARGET_HEIGHT),
        RenderResolution::Fixed { width, height } => (width.max(1), height.max(1)),
    };

    let mode = match settings.fill_mode {
        WallpaperFillMode::Cover => ScaleMode::Cover,
        WallpaperFillMode::Fit | WallpaperFillMode::Center => ScaleMode::Fit,
        WallpaperFillMode::Stretch => ScaleMode::Stretch,
    };

    let geometry = compute_presentation_geometry(
        mode,
        rotated.width(),
        rotated.height(),
        target_width,
        target_height,
        settings.zoom as f64,
        settings.position_x as f64,
        settings.position_y as f64,
    );

    let (canvas_width, canvas_height) =
        preview_canvas_extent(target_width, target_height, max_width, max_height);

    let (crop_x, crop_width) = match geometry.viewport_source {
        Some(viewport) => {
            preview_crop_axis(viewport.x, viewport.width, geometry.render_width, rotated.width())
        }
        None => (0, rotated.width()),
    };
    let (crop_y, crop_height) = match geometry.viewport_source {
        Some(viewport) => {
            preview_crop_axis(viewport.y, viewport.height, geometry.render_height, rotated.height())
        }
        None => (0, rotated.height()),
    };

    let cropped = imageops::crop_imm(&rotated, crop_x, crop_y, crop_width, crop_height).to_image();

    let destination_width =
        preview_destination_extent(geometry.viewport_width, target_width, canvas_width);
    let destination_height =
        preview_destination_extent(geometry.viewport_height, target_height, canvas_height);
    let mut composed =
        imageops::resize(&cropped, destination_width, destination_height, FilterType::Triangle);
    apply_visual_adjustments(&mut composed, settings.visual_adjustments);

    let mut canvas = RgbaImage::from_pixel(canvas_width, canvas_height, Rgba([0, 0, 0, 255]));
    let destination_x =
        normalized_destination_offset(canvas_width, destination_width, settings.position_x);
    let destination_y =
        normalized_destination_offset(canvas_height, destination_height, settings.position_y);

    imageops::overlay(&mut canvas, &composed, i64::from(destination_x), i64::from(destination_y));

    Ok((canvas_width, canvas_height, canvas.into_raw()))
}

pub(crate) fn apply_visual_adjustments(image: &mut RgbaImage, visual: VisualAdjustments) {
    let visual = visual.normalized();
    if visual.is_neutral() {
        return;
    }

    let angle = visual.hue_degrees.to_radians();
    let cos_hue = angle.cos();
    let sin_hue = angle.sin();

    let matrix = [
        [
            0.213 + cos_hue * 0.787 - sin_hue * 0.213,
            0.715 - cos_hue * 0.715 - sin_hue * 0.715,
            0.072 - cos_hue * 0.072 + sin_hue * 0.928,
        ],
        [
            0.213 - cos_hue * 0.213 + sin_hue * 0.143,
            0.715 + cos_hue * 0.285 + sin_hue * 0.140,
            0.072 - cos_hue * 0.072 - sin_hue * 0.283,
        ],
        [
            0.213 - cos_hue * 0.213 - sin_hue * 0.787,
            0.715 - cos_hue * 0.715 + sin_hue * 0.715,
            0.072 + cos_hue * 0.928 + sin_hue * 0.072,
        ],
    ];

    for pixel in image.pixels_mut() {
        let alpha = pixel[3];
        let mut r = f32::from(pixel[0]) / 255.0;
        let mut g = f32::from(pixel[1]) / 255.0;
        let mut b = f32::from(pixel[2]) / 255.0;

        r = (r + visual.brightness).clamp(0.0, 1.0);
        g = (g + visual.brightness).clamp(0.0, 1.0);
        b = (b + visual.brightness).clamp(0.0, 1.0);

        r = ((r - 0.5) * visual.contrast + 0.5).clamp(0.0, 1.0);
        g = ((g - 0.5) * visual.contrast + 0.5).clamp(0.0, 1.0);
        b = ((b - 0.5) * visual.contrast + 0.5).clamp(0.0, 1.0);

        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        r = (luma + (r - luma) * visual.saturation).clamp(0.0, 1.0);
        g = (luma + (g - luma) * visual.saturation).clamp(0.0, 1.0);
        b = (luma + (b - luma) * visual.saturation).clamp(0.0, 1.0);

        let rr = matrix[0][0] * r + matrix[0][1] * g + matrix[0][2] * b;
        let gg = matrix[1][0] * r + matrix[1][1] * g + matrix[1][2] * b;
        let bb = matrix[2][0] * r + matrix[2][1] * g + matrix[2][2] * b;

        pixel[0] = (rr.clamp(0.0, 1.0) * 255.0).round() as u8;
        pixel[1] = (gg.clamp(0.0, 1.0) * 255.0).round() as u8;
        pixel[2] = (bb.clamp(0.0, 1.0) * 255.0).round() as u8;
        pixel[3] = alpha;
    }
}

fn preview_canvas_extent(
    target_width: u32,
    target_height: u32,
    max_width: u32,
    max_height: u32,
) -> (u32, u32) {
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let max_width = max_width.max(1);
    let max_height = max_height.max(1);
    let scale =
        (max_width as f64 / target_width as f64).min(max_height as f64 / target_height as f64);

    let width = (target_width as f64 * scale).round().clamp(1.0, max_width as f64) as u32;
    let height = (target_height as f64 * scale).round().clamp(1.0, max_height as f64) as u32;

    (width, height)
}

fn preview_crop_axis(
    start: f64,
    length: f64,
    render_extent: u32,
    preview_extent: u32,
) -> (u32, u32) {
    let render_extent = render_extent.max(1) as f64;
    let preview_extent = preview_extent.max(1);
    let preview_extent_f64 = preview_extent as f64;

    let first = (start / render_extent * preview_extent_f64)
        .floor()
        .clamp(0.0, (preview_extent - 1) as f64) as u32;
    let end = ((start + length) / render_extent * preview_extent_f64)
        .ceil()
        .clamp((first + 1) as f64, preview_extent_f64) as u32;

    (first, end.saturating_sub(first).max(1))
}

fn preview_destination_extent(viewport_extent: u32, target_extent: u32, canvas_extent: u32) -> u32 {
    (viewport_extent as f64 / target_extent.max(1) as f64 * canvas_extent as f64)
        .round()
        .clamp(1.0, canvas_extent.max(1) as f64) as u32
}

fn normalized_destination_offset(canvas: u32, destination: u32, position: f32) -> u32 {
    let slack = canvas.saturating_sub(destination);
    if slack == 0 {
        return 0;
    }

    let position = if position.is_finite() { position.clamp(-1.0, 1.0) } else { 0.0 };
    let fraction = (f64::from(position) + 1.0) / 2.0;

    (slack as f64 * fraction).round().clamp(0.0, slack as f64) as u32
}

#[cfg(test)]
mod tests {
    use image_rs::{Rgba, RgbaImage};
    use we_core::wallpaper::settings::{
        RenderResolution, VisualAdjustments, WallpaperFillMode, WallpaperSettings,
    };

    use super::{
        apply_visual_adjustments, compose, DetailPreviewSource, DRAG_PREVIEW_CANVAS_MAX_HEIGHT,
        DRAG_PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT, PREVIEW_CANVAS_MAX_WIDTH,
    };

    fn source() -> DetailPreviewSource {
        let image = RgbaImage::from_pixel(160, 90, Rgba([200, 20, 20, 255]));
        DetailPreviewSource { width: image.width(), height: image.height(), rgba: image.into_raw() }
    }

    #[test]
    fn neutral_visual_adjustments_preserve_pixels() {
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([64, 128, 192, 77]));
        let before = image.clone();

        apply_visual_adjustments(&mut image, VisualAdjustments::default());

        assert_eq!(image, before);
    }

    #[test]
    fn visual_adjustments_change_rgb_without_changing_alpha() {
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([64, 128, 192, 77]));

        apply_visual_adjustments(
            &mut image,
            VisualAdjustments {
                brightness: 0.1,
                contrast: 1.2,
                saturation: 1.5,
                hue_degrees: 30.0,
            },
        );

        assert_ne!(&image.get_pixel(0, 0).0[..3], &[64, 128, 192]);
        assert_eq!(image.get_pixel(0, 0)[3], 77);
    }

    #[test]
    fn fit_preview_tracks_saved_position() {
        let source = source();
        let mut settings = WallpaperSettings {
            fill_mode: WallpaperFillMode::Fit,
            render_resolution: RenderResolution::Fixed { width: 1080, height: 1920 },
            zoom: 0.5,
            ..WallpaperSettings::default()
        };

        let centered =
            compose(&source, &settings, PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT)
                .expect("centered preview");
        settings.position_y = 1.0;
        let shifted =
            compose(&source, &settings, PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT)
                .expect("shifted preview");

        assert_eq!((centered.0, centered.1), (152, 270));
        assert_ne!(centered.2, shifted.2);
    }

    #[test]
    fn drag_preview_uses_lower_resolution_than_resting_preview() {
        let source = source();
        let settings = WallpaperSettings {
            fill_mode: WallpaperFillMode::Fit,
            render_resolution: RenderResolution::Fixed { width: 1920, height: 1080 },
            ..WallpaperSettings::default()
        };

        let full = compose(&source, &settings, PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT)
            .expect("full preview");
        let drag = compose(
            &source,
            &settings,
            DRAG_PREVIEW_CANVAS_MAX_WIDTH,
            DRAG_PREVIEW_CANVAS_MAX_HEIGHT,
        )
        .expect("drag preview");

        assert_eq!((full.0, full.1), (480, 270));
        assert_eq!((drag.0, drag.1), (240, 135));
        assert!(drag.2.len() < full.2.len());
    }

    #[test]
    fn invalid_transform_values_remain_renderable() {
        let source = source();
        let settings = WallpaperSettings {
            fill_mode: WallpaperFillMode::Fit,
            zoom: f32::NAN,
            position_x: f32::INFINITY,
            position_y: f32::NEG_INFINITY,
            ..WallpaperSettings::default()
        };

        let rendered =
            compose(&source, &settings, PREVIEW_CANVAS_MAX_WIDTH, PREVIEW_CANVAS_MAX_HEIGHT)
                .expect("bounded preview");
        assert!(!rendered.2.is_empty());
    }
}
