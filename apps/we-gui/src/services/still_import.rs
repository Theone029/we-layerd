use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Cursor, Write},
    path::{Path, PathBuf},
};

use iced::widget::image;
use image_rs::{
    imageops::{self, FilterType},
    DynamicImage, GenericImageView, ImageFormat, ImageReader, Limits, Rgba, RgbaImage,
};
use serde_json::json;
use we_core::wallpaper::settings::Rotation;

use crate::domain::still_editor::StillEditorState;

const STILL_MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const STILL_MAX_SOURCE_DIMENSION: u32 = 8_192;
const STILL_DECODER_MAX_ALLOC: u64 = 384 * 1024 * 1024;

const STILL_PREVIEW_MAX_WIDTH: u32 = 480;
const STILL_PREVIEW_MAX_HEIGHT: u32 = 270;
const STILL_EDITOR_CANVAS_MAX_WIDTH: u32 = 480;
const STILL_EDITOR_CANVAS_MAX_HEIGHT: u32 = 270;

const SOURCE_FILE_NAME: &str = "source.png";
const PREVIEW_FILE_NAME: &str = "preview.png";
const PROJECT_FILE_NAME: &str = "project.json";

#[derive(Debug, Clone)]
pub(crate) struct StillImageDraft {
    pub(crate) source_path: PathBuf,
    pub(crate) title: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    preview_width: u32,
    preview_height: u32,
    preview_rgba: Vec<u8>,
    pub(crate) preview: image::Handle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StillImportResult {
    pub(crate) id: String,
    pub(crate) directory: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) preview_path: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) created: bool,
}

pub(crate) async fn inspect(path: PathBuf) -> Result<StillImageDraft, String> {
    inspect_sync(&path)
}

pub(crate) async fn import(
    workshop_root: PathBuf,
    source_path: PathBuf,
    title: String,
) -> Result<StillImportResult, String> {
    import_sync(&workshop_root, &source_path, &title)
}

fn inspect_sync(path: &Path) -> Result<StillImageDraft, String> {
    let decoded = decode_still(path)?;
    let (width, height) = decoded.dimensions();

    let preview = decoded.thumbnail(STILL_PREVIEW_MAX_WIDTH, STILL_PREVIEW_MAX_HEIGHT);
    let rgba = preview.to_rgba8();
    let preview_width = rgba.width();
    let preview_height = rgba.height();
    let preview_rgba = rgba.into_raw();
    let preview_handle =
        image::Handle::from_rgba(preview_width, preview_height, preview_rgba.clone());

    Ok(StillImageDraft {
        source_path: path.to_path_buf(),
        title: normalized_title("", path),
        width,
        height,
        preview_width,
        preview_height,
        preview_rgba,
        preview: preview_handle,
    })
}

pub(crate) fn refresh_editor_preview(
    draft: &mut StillImageDraft,
    state: &StillEditorState,
) -> Result<(), String> {
    let (width, height, rgba) = compose_editor_preview(draft, state)?;
    draft.preview = image::Handle::from_rgba(width, height, rgba);
    Ok(())
}

fn compose_editor_preview(
    draft: &StillImageDraft,
    state: &StillEditorState,
) -> Result<(u32, u32, Vec<u8>), String> {
    let source =
        RgbaImage::from_raw(draft.preview_width, draft.preview_height, draft.preview_rgba.clone())
            .ok_or_else(|| "stored still-image preview pixels are invalid".to_string())?;

    // The runtime maps Deg90/Deg180/Deg270 to the corresponding Wayland
    // buffer transform. Because the compositor applies the inverse transform
    // to unrotated buffer contents, the visible positive rotation is clockwise.
    let rotated = match state.rotation {
        Rotation::Deg0 => source,
        Rotation::Deg90 => imageops::rotate90(&source),
        Rotation::Deg180 => imageops::rotate180(&source),
        Rotation::Deg270 => imageops::rotate270(&source),
    };

    let geometry = state.preview_geometry_for((draft.width, draft.height));
    let (target_width, target_height) = state.target_extent();
    let (canvas_width, canvas_height) = preview_canvas_extent(target_width, target_height);

    let (crop_x, crop_width) = match geometry.viewport_source {
        Some(source) => {
            preview_crop_axis(source.x, source.width, geometry.render_width, rotated.width())
        }
        None => (0, rotated.width()),
    };
    let (crop_y, crop_height) = match geometry.viewport_source {
        Some(source) => {
            preview_crop_axis(source.y, source.height, geometry.render_height, rotated.height())
        }
        None => (0, rotated.height()),
    };

    let cropped = imageops::crop_imm(&rotated, crop_x, crop_y, crop_width, crop_height).to_image();

    let destination_width =
        preview_destination_extent(geometry.viewport_width, target_width, canvas_width);
    let destination_height =
        preview_destination_extent(geometry.viewport_height, target_height, canvas_height);

    let composed =
        imageops::resize(&cropped, destination_width, destination_height, FilterType::Triangle);

    let mut canvas = RgbaImage::from_pixel(canvas_width, canvas_height, Rgba([0, 0, 0, 255]));
    let destination_x =
        normalized_destination_offset(canvas_width, destination_width, state.position_x);
    let destination_y =
        normalized_destination_offset(canvas_height, destination_height, state.position_y);

    imageops::overlay(&mut canvas, &composed, i64::from(destination_x), i64::from(destination_y));

    Ok((canvas_width, canvas_height, canvas.into_raw()))
}

fn preview_canvas_extent(target_width: u32, target_height: u32) -> (u32, u32) {
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let scale = (STILL_EDITOR_CANVAS_MAX_WIDTH as f64 / target_width as f64)
        .min(STILL_EDITOR_CANVAS_MAX_HEIGHT as f64 / target_height as f64);

    let width = (target_width as f64 * scale)
        .round()
        .clamp(1.0, STILL_EDITOR_CANVAS_MAX_WIDTH as f64) as u32;
    let height = (target_height as f64 * scale)
        .round()
        .clamp(1.0, STILL_EDITOR_CANVAS_MAX_HEIGHT as f64) as u32;

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

fn import_sync(
    workshop_root: &Path,
    source_path: &Path,
    requested_title: &str,
) -> Result<StillImportResult, String> {
    let decoded = decode_still(source_path)?;
    let (width, height) = decoded.dimensions();

    let source_png = encode_png(&decoded)?;
    let preview = decoded.thumbnail(STILL_PREVIEW_MAX_WIDTH, STILL_PREVIEW_MAX_HEIGHT);
    let preview_png = encode_png(&preview)?;

    let title = normalized_title(requested_title, source_path);
    let base_id = format!("local-image-{:016x}", fnv1a64(&source_png));

    fs::create_dir_all(workshop_root).map_err(|error| {
        format!("failed to create wallpaper library {}: {error}", workshop_root.display())
    })?;
    private_dir_permissions(workshop_root)?;

    let (id, destination) = choose_destination(workshop_root, &base_id, &source_png)?;

    if destination.exists() {
        return existing_result(id, destination, width, height);
    }

    let temp = workshop_root.join(format!(".{id}.tmp-{}", std::process::id()));

    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| {
            format!("failed to clear stale import staging {}: {error}", temp.display())
        })?;
    }

    fs::create_dir(&temp)
        .map_err(|error| format!("failed to create import staging {}: {error}", temp.display()))?;
    private_dir_permissions(&temp)?;

    let source_out = temp.join(SOURCE_FILE_NAME);
    let preview_out = temp.join(PREVIEW_FILE_NAME);
    let project_out = temp.join(PROJECT_FILE_NAME);

    let install_result = (|| -> Result<(), String> {
        write_private_file(&source_out, &source_png)?;
        write_private_file(&preview_out, &preview_png)?;

        let mut project_json = serde_json::to_vec_pretty(&json!({
            "title": title,
            "type": "video",
            "file": SOURCE_FILE_NAME,
            "we_layerd": {
                "kind": "image",
                "version": 1
            }
        }))
        .map_err(|error| format!("failed to serialize imported wallpaper metadata: {error}"))?;
        project_json.push(b'\n');

        write_private_file(&project_out, &project_json)?;

        fs::rename(&temp, &destination).map_err(|error| {
            format!("failed to publish imported wallpaper {}: {error}", destination.display())
        })?;

        Ok(())
    })();

    if let Err(error) = install_result {
        let _ = fs::remove_dir_all(&temp);
        return Err(error);
    }

    sync_directory(workshop_root)?;

    Ok(StillImportResult {
        id,
        source_path: destination.join(SOURCE_FILE_NAME),
        preview_path: destination.join(PREVIEW_FILE_NAME),
        directory: destination,
        width,
        height,
        created: true,
    })
}

fn decode_still(path: &Path) -> Result<DynamicImage, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect image {}: {error}", path.display()))?;

    if metadata.file_type().is_symlink() {
        return Err(format!(
            "image source must be a regular file, not a symlink: {}",
            path.display()
        ));
    }

    if !metadata.is_file() {
        return Err(format!("image source is not a regular file: {}", path.display()));
    }

    if metadata.len() > STILL_MAX_INPUT_BYTES {
        return Err(format!(
            "image source exceeds the {} MiB input limit",
            STILL_MAX_INPUT_BYTES / (1024 * 1024)
        ));
    }

    let file = File::open(path)
        .map_err(|error| format!("cannot open image {}: {error}", path.display()))?;

    let mut reader =
        ImageReader::new(BufReader::new(file)).with_guessed_format().map_err(|error| {
            format!("cannot determine image format for {}: {error}", path.display())
        })?;

    let format = reader
        .format()
        .ok_or_else(|| format!("cannot determine image format for {}", path.display()))?;

    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err(format!(
            "unsupported still-image format for {}; use PNG or JPEG",
            path.display()
        ));
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(STILL_MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(STILL_MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(STILL_DECODER_MAX_ALLOC);
    reader.limits(limits);

    reader.decode().map_err(|error| format!("cannot decode image {}: {error}", path.display()))
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();

    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|error| format!("failed to encode normalized PNG: {error}"))?;

    Ok(bytes)
}

fn normalized_title(requested: &str, source: &Path) -> String {
    let requested = requested.trim();

    let base = if requested.is_empty() {
        source
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Custom wallpaper")
    } else {
        requested
    };

    base.chars().take(160).collect()
}

fn choose_destination(
    root: &Path,
    base_id: &str,
    source_png: &[u8],
) -> Result<(String, PathBuf), String> {
    for suffix in 0..10_000_u32 {
        let id = if suffix == 0 { base_id.to_string() } else { format!("{base_id}-{suffix}") };

        let destination = root.join(&id);

        if !destination.exists() {
            return Ok((id, destination));
        }

        let existing_source = destination.join(SOURCE_FILE_NAME);

        if existing_source.is_file() {
            let existing = fs::read(&existing_source).map_err(|error| {
                format!("failed to compare existing import {}: {error}", existing_source.display())
            })?;

            if existing == source_png {
                if !destination.join(PROJECT_FILE_NAME).is_file()
                    || !destination.join(PREVIEW_FILE_NAME).is_file()
                {
                    return Err(format!("existing import {} is incomplete", destination.display()));
                }

                return Ok((id, destination));
            }
        }
    }

    Err("could not allocate a unique local wallpaper id".to_string())
}

fn existing_result(
    id: String,
    directory: PathBuf,
    width: u32,
    height: u32,
) -> Result<StillImportResult, String> {
    let source_path = directory.join(SOURCE_FILE_NAME);
    let preview_path = directory.join(PREVIEW_FILE_NAME);

    if !source_path.is_file()
        || !preview_path.is_file()
        || !directory.join(PROJECT_FILE_NAME).is_file()
    {
        return Err(format!("existing import {} is incomplete", directory.display()));
    }

    Ok(StillImportResult {
        id,
        directory,
        source_path,
        preview_path,
        width,
        height,
        created: false,
    })
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;

    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("failed to create {}: {error}", path.display()))?;

    file.write_all(bytes)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;

    file.sync_all().map_err(|error| format!("failed to sync {}: {error}", path.display()))?;

    private_file_permissions(path)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    let directory = File::open(path).map_err(|error| {
        format!("failed to open directory {} for sync: {error}", path.display())
    })?;

    directory
        .sync_all()
        .map_err(|error| format!("failed to sync directory {}: {error}", path.display()))
}

#[cfg(unix)]
fn private_dir_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        format!("failed to set private directory permissions on {}: {error}", path.display())
    })
}

#[cfg(not(unix))]
fn private_dir_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!("failed to set private file permissions on {}: {error}", path.display())
    })
}

#[cfg(not(unix))]
fn private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use image_rs::{DynamicImage, ImageFormat, Rgb, RgbImage, RgbaImage};
    use we_core::{
        config::ScaleMode,
        wallpaper::{scan_workshop_wallpapers, WallpaperType},
    };

    use crate::domain::still_editor::StillEditorState;

    use super::{
        compose_editor_preview, import_sync, inspect_sync, PREVIEW_FILE_NAME, PROJECT_FILE_NAME,
        SOURCE_FILE_NAME,
    };

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock").as_nanos();

        let path = std::env::temp_dir()
            .join(format!("we-gui-still-import-{label}-{}-{nonce}", std::process::id()));

        fs::create_dir_all(&path).expect("create temporary test directory");
        path
    }

    fn fixture_image(path: &Path, format: ImageFormat) {
        let image = RgbImage::from_fn(64, 48, |x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                Rgb([20, 80, 220])
            } else {
                Rgb([240, 240, 240])
            }
        });

        let dynamic = DynamicImage::ImageRgb8(image);
        let mut file = File::create(path).expect("create fixture");

        dynamic.write_to(&mut file, format).expect("encode fixture");
    }

    #[test]
    fn inspect_png_builds_bounded_editor_preview() {
        let root = temp_root("inspect");
        let source = root.join("My Wallpaper.png");
        fixture_image(&source, ImageFormat::Png);

        let draft = inspect_sync(&source).expect("inspect PNG");

        assert_eq!(draft.source_path, source);
        assert_eq!(draft.title, "My Wallpaper");
        assert_eq!((draft.width, draft.height), (64, 48));

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn editor_preview_applies_shared_destination_geometry() {
        let root = temp_root("editor-preview");
        let source = root.join("source.png");
        fixture_image(&source, ImageFormat::Png);
        let draft = inspect_sync(&source).expect("inspect PNG");

        let mut state = StillEditorState::default();
        state.target_width = "400".to_string();
        state.target_height = "400".to_string();
        state.scale_mode = ScaleMode::Fit;
        state.zoom = 0.5;
        state.position_x = 1.0;

        let (width, height, rgba) =
            compose_editor_preview(&draft, &state).expect("compose editor preview");
        assert_eq!((width, height), (270, 270));

        let image = RgbaImage::from_raw(width, height, rgba).expect("preview pixels");
        assert_eq!(image.get_pixel(0, height / 2).0, [0, 0, 0, 255]);
        assert_ne!(image.get_pixel(width - 1, height / 2).0, [0, 0, 0, 255]);

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn import_png_creates_renderer_compatible_private_project() {
        let root = temp_root("png");
        let library = root.join("library");
        let source = root.join("source.png");
        fixture_image(&source, ImageFormat::Png);

        let imported = import_sync(&library, &source, "Local Test").expect("import PNG");

        assert!(imported.created);
        assert!(imported.id.starts_with("local-image-"));
        assert!(imported.directory.join(PROJECT_FILE_NAME).is_file());
        assert!(imported.directory.join(SOURCE_FILE_NAME).is_file());
        assert!(imported.directory.join(PREVIEW_FILE_NAME).is_file());

        let project: serde_json::Value = serde_json::from_slice(
            &fs::read(imported.directory.join(PROJECT_FILE_NAME)).expect("read project.json"),
        )
        .expect("parse project.json");

        assert_eq!(project["title"], "Local Test");
        assert_eq!(project["type"], "video");
        assert_eq!(project["file"], SOURCE_FILE_NAME);
        assert_eq!(project["we_layerd"]["kind"], "image");
        assert_eq!(project["we_layerd"]["version"], 1);

        let entries = scan_workshop_wallpapers(&library).expect("scan imported library");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, imported.id);
        assert_eq!(entries[0].ty, WallpaperType::Video);
        assert_eq!(entries[0].source_file.as_deref(), Some(imported.source_path.as_path()));
        assert_eq!(entries[0].preview.as_deref(), Some(imported.preview_path.as_path()));

        let duplicate =
            import_sync(&library, &source, "Different title").expect("repeat identical import");

        assert!(!duplicate.created);
        assert_eq!(duplicate.id, imported.id);
        assert_eq!(duplicate.directory, imported.directory);

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn jpeg_input_is_normalized_to_png_transport() {
        let root = temp_root("jpeg");
        let library = root.join("library");
        let source = root.join("photo.jpg");
        fixture_image(&source, ImageFormat::Jpeg);

        let imported = import_sync(&library, &source, "").expect("import JPEG");

        let normalized = fs::read(&imported.source_path).expect("read normalized PNG");

        assert!(normalized.starts_with(b"\x89PNG\r\n\x1a\n"));

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn animated_gif_is_rejected_by_still_import_path() {
        let root = temp_root("gif");
        let source = root.join("animated.gif");

        fs::write(&source, b"GIF89a\x01\x00\x01\x00\x00\x00\x00").expect("write GIF signature");

        let error = match inspect_sync(&source) {
            Ok(_) => panic!("GIF must not enter still-image import"),
            Err(error) => error,
        };

        assert!(error.contains("PNG or JPEG"), "unexpected error: {error}");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn imported_project_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("permissions");
        let library = root.join("library");
        let source = root.join("source.png");
        fixture_image(&source, ImageFormat::Png);

        let imported = import_sync(&library, &source, "Private").expect("import PNG");

        assert_eq!(
            fs::metadata(&imported.directory).expect("directory metadata").permissions().mode()
                & 0o777,
            0o700
        );

        for path in [
            imported.source_path,
            imported.preview_path,
            imported.directory.join(PROJECT_FILE_NAME),
        ] {
            assert_eq!(
                fs::metadata(&path).expect("file metadata").permissions().mode() & 0o777,
                0o600,
                "{}",
                path.display()
            );
        }

        fs::remove_dir_all(root).expect("remove fixture");
    }
}
