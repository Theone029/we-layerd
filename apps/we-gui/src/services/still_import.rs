use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Cursor, Read, Write},
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

use super::media_backend::{self, MotionProbe};

const STILL_MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const STILL_MAX_SOURCE_DIMENSION: u32 = 8_192;
const STILL_DECODER_MAX_ALLOC: u64 = 384 * 1024 * 1024;

const STILL_PREVIEW_MAX_WIDTH: u32 = 480;
const STILL_PREVIEW_MAX_HEIGHT: u32 = 270;
const STILL_EDITOR_CANVAS_MAX_WIDTH: u32 = 480;
const STILL_EDITOR_CANVAS_MAX_HEIGHT: u32 = 270;

const RENDERED_FILE_NAME: &str = "rendered.png";
const PREVIEW_FILE_NAME: &str = "preview.png";
const PROJECT_FILE_NAME: &str = "project.json";

#[derive(Debug, Clone)]
pub(crate) struct StillImageDraft {
    pub(crate) source_path: PathBuf,
    pub(crate) original_name: String,
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
    pub(crate) original_path: PathBuf,
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
    original_name: String,
    title: String,
) -> Result<StillImportResult, String> {
    import_sync(&workshop_root, &source_path, &original_name, &title)
}

fn inspect_sync(path: &Path) -> Result<StillImageDraft, String> {
    match decode_still(path) {
        Ok((decoded, _)) => {
            let dimensions = decoded.dimensions();
            build_editor_draft(path, dimensions, decoded)
        }
        Err(still_error) => {
            let probe = media_backend::probe_motion(path).map_err(|media_error| {
                format!(
                    "selected media is neither a supported still nor supported motion media:                      {still_error}; {media_error}"
                )
            })?;
            let preview_png = media_backend::extract_preview_png(path)?;
            let preview = image_rs::load_from_memory_with_format(&preview_png, ImageFormat::Png)
                .map_err(|error| format!("cannot decode bounded FFmpeg preview: {error}"))?;

            build_editor_draft(path, (probe.width, probe.height), preview)
        }
    }
}

fn build_editor_draft(
    path: &Path,
    source_dimensions: (u32, u32),
    preview_source: DynamicImage,
) -> Result<StillImageDraft, String> {
    let preview = preview_source.thumbnail(STILL_PREVIEW_MAX_WIDTH, STILL_PREVIEW_MAX_HEIGHT);
    let rgba = preview.to_rgba8();
    let preview_width = rgba.width();
    let preview_height = rgba.height();
    let preview_rgba = rgba.into_raw();
    let preview_handle =
        image::Handle::from_rgba(preview_width, preview_height, preview_rgba.clone());

    Ok(StillImageDraft {
        source_path: path.to_path_buf(),
        original_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wallpaper")
            .chars()
            .take(240)
            .collect(),
        title: normalized_title("", path),
        width: source_dimensions.0,
        height: source_dimensions.1,
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
    original_name: &str,
    requested_title: &str,
) -> Result<StillImportResult, String> {
    match decode_still(source_path) {
        Ok((decoded, source_format)) => import_still_sync(
            workshop_root,
            source_path,
            original_name,
            requested_title,
            decoded,
            source_format,
        ),
        Err(_) => {
            let probe = media_backend::probe_motion(source_path)?;
            import_motion_sync(workshop_root, source_path, original_name, requested_title, &probe)
        }
    }
}

fn import_still_sync(
    workshop_root: &Path,
    source_path: &Path,
    original_name: &str,
    requested_title: &str,
    decoded: DynamicImage,
    source_format: ImageFormat,
) -> Result<StillImportResult, String> {
    let (width, height) = decoded.dimensions();

    let rendered_png = encode_png(&decoded)?;
    let preview = decoded.thumbnail(STILL_PREVIEW_MAX_WIDTH, STILL_PREVIEW_MAX_HEIGHT);
    let preview_png = encode_png(&preview)?;

    let original_extension = original_extension(source_format)?;
    let original_file_name = format!("original.{original_extension}");
    let original_name = normalized_original_name(original_name, original_extension);
    let title = normalized_title(requested_title, Path::new(&original_name));

    let exact_hash = fnv1a64_file(source_path)?;
    let base_id = format!("local-media-{exact_hash:016x}");

    fs::create_dir_all(workshop_root).map_err(|error| {
        format!("failed to create wallpaper library {}: {error}", workshop_root.display())
    })?;
    private_dir_permissions(workshop_root)?;

    let (id, destination) = choose_destination(
        workshop_root,
        &base_id,
        source_path,
        &original_file_name,
        RENDERED_FILE_NAME,
    )?;

    if destination.exists() {
        return existing_result(
            id,
            destination,
            width,
            height,
            &original_file_name,
            RENDERED_FILE_NAME,
        );
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

    let original_out = temp.join(&original_file_name);
    let rendered_out = temp.join(RENDERED_FILE_NAME);
    let preview_out = temp.join(PREVIEW_FILE_NAME);
    let project_out = temp.join(PROJECT_FILE_NAME);

    let install_result = (|| -> Result<(), String> {
        copy_private_file(source_path, &original_out)?;
        write_private_file(&rendered_out, &rendered_png)?;
        write_private_file(&preview_out, &preview_png)?;

        let mut project_json = serde_json::to_vec_pretty(&json!({
            "title": title,
            "type": "video",
            "file": RENDERED_FILE_NAME,
            "we_layerd": {
                "kind": "image",
                "version": 2,
                "original_name": original_name,
                "original_file": original_file_name,
                "rendered_file": RENDERED_FILE_NAME
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
        original_path: destination.join(&original_file_name),
        source_path: destination.join(RENDERED_FILE_NAME),
        preview_path: destination.join(PREVIEW_FILE_NAME),
        directory: destination,
        width,
        height,
        created: true,
    })
}

fn import_motion_sync(
    workshop_root: &Path,
    source_path: &Path,
    original_name: &str,
    requested_title: &str,
    probe: &MotionProbe,
) -> Result<StillImportResult, String> {
    let width = probe.width;
    let height = probe.height;
    let original_file_name = format!("original.{}", probe.kind.original_extension());
    let rendered_file_name = format!("rendered.{}", probe.kind.rendered_extension());
    let original_name = normalized_original_name(original_name, probe.kind.original_extension());
    let title = normalized_title(requested_title, Path::new(&original_name));

    let preview_png = media_backend::extract_preview_png(source_path)?;
    let exact_hash = fnv1a64_file(source_path)?;
    let base_id = format!("local-media-{exact_hash:016x}");

    fs::create_dir_all(workshop_root).map_err(|error| {
        format!("failed to create wallpaper library {}: {error}", workshop_root.display())
    })?;
    private_dir_permissions(workshop_root)?;

    let (id, destination) = choose_destination(
        workshop_root,
        &base_id,
        source_path,
        &original_file_name,
        &rendered_file_name,
    )?;

    if destination.exists() {
        return existing_result(
            id,
            destination,
            width,
            height,
            &original_file_name,
            &rendered_file_name,
        );
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

    let original_out = temp.join(&original_file_name);
    let rendered_out = temp.join(&rendered_file_name);
    let preview_out = temp.join(PREVIEW_FILE_NAME);
    let project_out = temp.join(PROJECT_FILE_NAME);

    let install_result = (|| -> Result<(), String> {
        copy_private_file(source_path, &original_out)?;

        if probe.kind.needs_transcode() {
            media_backend::transcode_gif_to_webm(&original_out, &rendered_out)?;
            private_file_permissions(&rendered_out)?;
        } else {
            copy_private_file(&original_out, &rendered_out)?;
        }

        write_private_file(&preview_out, &preview_png)?;

        let mut project_json = serde_json::to_vec_pretty(&json!({
            "title": title,
            "type": "video",
            "file": rendered_file_name,
            "we_layerd": {
                "kind": probe.kind.metadata_kind(),
                "version": 3,
                "original_name": original_name,
                "original_file": original_file_name,
                "rendered_file": rendered_file_name,
                "source_format": probe.kind.format_name(),
                "source_codec": probe.codec_name,
                "media_backend": "ffmpeg"
            }
        }))
        .map_err(|error| format!("failed to serialize imported media metadata: {error}"))?;
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
        original_path: destination.join(&original_file_name),
        source_path: destination.join(&rendered_file_name),
        preview_path: destination.join(PREVIEW_FILE_NAME),
        directory: destination,
        width,
        height,
        created: true,
    })
}

fn decode_still(path: &Path) -> Result<(DynamicImage, ImageFormat), String> {
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
            "still image exceeds the {} MiB decode limit",
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

    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) {
        return Err(format!(
            "unsupported still-image format for {}; use PNG, JPEG, or WebP",
            path.display()
        ));
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(STILL_MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(STILL_MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(STILL_DECODER_MAX_ALLOC);
    reader.limits(limits);

    let decoded = reader
        .decode()
        .map_err(|error| format!("cannot decode image {}: {error}", path.display()))?;

    Ok((decoded, format))
}

fn original_extension(format: ImageFormat) -> Result<&'static str, String> {
    match format {
        ImageFormat::Png => Ok("png"),
        ImageFormat::Jpeg => Ok("jpg"),
        ImageFormat::WebP => Ok("webp"),
        other => Err(format!("unsupported exact-original format: {other:?}")),
    }
}

fn normalized_original_name(name: &str, fallback_extension: &str) -> String {
    let clean = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().filter(|ch| !ch.is_control()).take(240).collect::<String>())
        .filter(|value| !value.is_empty());

    clean.unwrap_or_else(|| format!("wallpaper.{fallback_extension}"))
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
    source_path: &Path,
    original_file_name: &str,
    rendered_file_name: &str,
) -> Result<(String, PathBuf), String> {
    for suffix in 0..10_000_u32 {
        let id = if suffix == 0 { base_id.to_string() } else { format!("{base_id}-{suffix}") };
        let destination = root.join(&id);

        if !destination.exists() {
            return Ok((id, destination));
        }

        let existing_original = destination.join(original_file_name);

        if existing_original.is_file() && files_equal(source_path, &existing_original)? {
            if !destination.join(PROJECT_FILE_NAME).is_file()
                || !destination.join(PREVIEW_FILE_NAME).is_file()
                || !destination.join(rendered_file_name).is_file()
            {
                return Err(format!("existing import {} is incomplete", destination.display()));
            }

            return Ok((id, destination));
        }
    }

    Err("could not allocate a unique local wallpaper id".to_string())
}

fn existing_result(
    id: String,
    directory: PathBuf,
    width: u32,
    height: u32,
    original_file_name: &str,
    rendered_file_name: &str,
) -> Result<StillImportResult, String> {
    let original_path = directory.join(original_file_name);
    let source_path = directory.join(rendered_file_name);
    let preview_path = directory.join(PREVIEW_FILE_NAME);

    if !original_path.is_file()
        || !source_path.is_file()
        || !preview_path.is_file()
        || !directory.join(PROJECT_FILE_NAME).is_file()
    {
        return Err(format!("existing import {} is incomplete", directory.display()));
    }

    Ok(StillImportResult {
        id,
        directory,
        original_path,
        source_path,
        preview_path,
        width,
        height,
        created: false,
    })
}

fn fnv1a64_file(path: &Path) -> Result<u64, String> {
    let mut file =
        File::open(path).map_err(|error| format!("failed to hash {}: {error}", path.display()))?;
    let mut hash = 0xcbf29ce484222325_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to hash {}: {error}", path.display()))?;

        if count == 0 {
            return Ok(hash);
        }

        for byte in &buffer[..count] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left_len = fs::metadata(left)
        .map_err(|error| format!("failed to inspect {}: {error}", left.display()))?
        .len();
    let right_len = fs::metadata(right)
        .map_err(|error| format!("failed to inspect {}: {error}", right.display()))?
        .len();

    if left_len != right_len {
        return Ok(false);
    }

    let mut left =
        File::open(left).map_err(|error| format!("failed to open {}: {error}", left.display()))?;
    let mut right = File::open(right)
        .map_err(|error| format!("failed to open {}: {error}", right.display()))?;
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];

    loop {
        let left_count = left
            .read(&mut left_buffer)
            .map_err(|error| format!("failed to compare exact original: {error}"))?;
        let right_count = right
            .read(&mut right_buffer)
            .map_err(|error| format!("failed to compare exact original: {error}"))?;

        if left_count != right_count {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
        if left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
    }
}

fn copy_private_file(source: &Path, destination: &Path) -> Result<(), String> {
    let expected = fs::metadata(source)
        .map_err(|error| format!("failed to inspect exact original {}: {error}", source.display()))?
        .len();

    let mut input = File::open(source)
        .map_err(|error| format!("failed to open {}: {error}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;

    let copied = io::copy(&mut input, &mut output)
        .map_err(|error| format!("failed to preserve exact original: {error}"))?;

    if copied != expected {
        return Err(format!(
            "exact-original copy length changed: expected {expected} bytes, copied {copied}"
        ));
    }

    output
        .sync_all()
        .map_err(|error| format!("failed to sync {}: {error}", destination.display()))?;

    private_file_permissions(destination)
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
        RENDERED_FILE_NAME,
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
    fn png_import_preserves_exact_original_and_separate_renderer_derivative() {
        let root = temp_root("png");
        let library = root.join("library");
        let source = root.join("opaque-upload.bin");
        fixture_image(&source, ImageFormat::Png);
        let exact = fs::read(&source).expect("read source");

        let imported =
            import_sync(&library, &source, "Vacation Photo.png", "Local Test").expect("import PNG");

        assert!(imported.created);
        assert!(imported.id.starts_with("local-media-"));
        assert_eq!(
            imported.original_path.file_name().and_then(|name| name.to_str()),
            Some("original.png")
        );
        assert_eq!(fs::read(&imported.original_path).expect("read exact original"), exact);
        assert!(imported.directory.join(PROJECT_FILE_NAME).is_file());
        assert!(imported.directory.join(RENDERED_FILE_NAME).is_file());
        assert!(imported.directory.join(PREVIEW_FILE_NAME).is_file());

        let rendered = fs::read(&imported.source_path).expect("read renderer derivative");
        assert!(rendered.starts_with(b"\x89PNG\r\n\x1a\n"));

        let project: serde_json::Value = serde_json::from_slice(
            &fs::read(imported.directory.join(PROJECT_FILE_NAME)).expect("read project.json"),
        )
        .expect("parse project.json");

        assert_eq!(project["title"], "Local Test");
        assert_eq!(project["type"], "video");
        assert_eq!(project["file"], RENDERED_FILE_NAME);
        assert_eq!(project["we_layerd"]["kind"], "image");
        assert_eq!(project["we_layerd"]["version"], 2);
        assert_eq!(project["we_layerd"]["original_name"], "Vacation Photo.png");
        assert_eq!(project["we_layerd"]["original_file"], "original.png");

        let entries = scan_workshop_wallpapers(&library).expect("scan imported library");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, imported.id);
        assert_eq!(entries[0].ty, WallpaperType::Video);
        assert_eq!(entries[0].source_file.as_deref(), Some(imported.source_path.as_path()));
        assert_eq!(entries[0].source_name.as_deref(), Some("Vacation Photo.png"));
        assert_eq!(entries[0].preview.as_deref(), Some(imported.preview_path.as_path()));

        let duplicate = import_sync(&library, &source, "Renamed copy.png", "Different title")
            .expect("repeat identical import");

        assert!(!duplicate.created);
        assert_eq!(duplicate.id, imported.id);
        assert_eq!(duplicate.directory, imported.directory);

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn jpeg_and_webp_keep_exact_bytes_while_rendering_png() {
        for (label, format, display_name, expected_file) in [
            ("jpeg", ImageFormat::Jpeg, "Camera Original.JPG", "original.jpg"),
            ("webp", ImageFormat::WebP, "Downloaded Art.webp", "original.webp"),
        ] {
            let root = temp_root(label);
            let library = root.join("library");
            let source = root.join(format!("{label}.payload"));
            fixture_image(&source, format);
            let exact = fs::read(&source).expect("read source");

            let imported =
                import_sync(&library, &source, display_name, "").expect("import supported still");

            assert_eq!(
                imported.original_path.file_name().and_then(|name| name.to_str()),
                Some(expected_file)
            );
            assert_eq!(fs::read(&imported.original_path).expect("read exact original"), exact);
            assert!(fs::read(&imported.source_path)
                .expect("read rendered PNG")
                .starts_with(b"\x89PNG\r\n\x1a\n"));

            let entries = scan_workshop_wallpapers(&library).expect("scan imported library");
            assert_eq!(entries[0].source_name.as_deref(), Some(display_name));

            fs::remove_dir_all(root).expect("remove fixture");
        }
    }

    #[test]
    fn content_probe_does_not_trust_filename_extension() {
        let root = temp_root("probe");
        let source = root.join("not-an-image.txt");
        fixture_image(&source, ImageFormat::WebP);

        let draft = inspect_sync(&source).expect("WebP content probe");

        assert_eq!((draft.width, draft.height), (64, 48));

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

        let imported = import_sync(&library, &source, "source.png", "Private").expect("import PNG");

        assert_eq!(
            fs::metadata(&imported.directory).expect("directory metadata").permissions().mode()
                & 0o777,
            0o700
        );

        for path in [
            imported.original_path,
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
