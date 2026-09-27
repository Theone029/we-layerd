use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use image_rs::{DynamicImage, ImageFormat, ImageReader, Limits};
use serde::Deserialize;
use we_core::wallpaper::{
    settings::{VisualAdjustments, WallpaperSettings},
    WallpaperEntry,
};

use super::{detail_preview::apply_visual_adjustments, media_backend};

const STILL_MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const STILL_MAX_SOURCE_DIMENSION: u32 = 8_192;
const STILL_DECODER_MAX_ALLOC: u64 = 384 * 1024 * 1024;
const MARKER_FILE_NAME: &str = ".we-layerd-visual-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializeOutcome {
    pub(crate) changed: bool,
    pub(crate) derivative: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ProjectDocument {
    #[serde(default)]
    file: String,
    #[serde(default)]
    we_layerd: ImportedMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct ImportedMetadata {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    original_file: String,
    #[serde(default)]
    rendered_file: String,
}

pub(crate) async fn materialize(
    entry: WallpaperEntry,
    settings: WallpaperSettings,
) -> Result<MaterializeOutcome, String> {
    materialize_sync(&entry, &settings)
}

fn materialize_sync(
    entry: &WallpaperEntry,
    settings: &WallpaperSettings,
) -> Result<MaterializeOutcome, String> {
    if !entry.imported {
        return Err("visual derivative materialization is limited to imported local media".into());
    }

    let directory = entry
        .project_json
        .parent()
        .ok_or_else(|| "imported project has no parent directory".to_string())?;
    let project = load_project(&entry.project_json)?;

    if project.file.trim().is_empty() || project.we_layerd.rendered_file.trim().is_empty() {
        return Err("imported project does not name a renderer derivative".to_string());
    }
    if project.file != project.we_layerd.rendered_file {
        return Err("project renderer file disagrees with imported rendered_file metadata".into());
    }

    let original = safe_child(directory, &project.we_layerd.original_file)?;
    let derivative = safe_child(directory, &project.we_layerd.rendered_file)?;
    validate_regular_source(&original)?;

    let visual = settings.visual_adjustments.normalized();
    let fingerprint = recipe_fingerprint(&original, visual)?;
    let marker = directory.join(MARKER_FILE_NAME);

    if marker_matches(&marker, &fingerprint) && valid_existing_derivative(&derivative) {
        return Ok(MaterializeOutcome { changed: false, derivative });
    }

    let temp = derivative_temp_path(&derivative)?;
    if temp.exists() {
        fs::remove_file(&temp).map_err(|error| {
            format!("cannot clear stale derivative temp {}: {error}", temp.display())
        })?;
    }

    let result = match project.we_layerd.kind.as_str() {
        "image" => materialize_still(&original, &temp, visual),
        "gif" | "video" => media_backend::materialize_motion_visual(&original, &temp, visual),
        other => Err(format!("unsupported imported media kind '{other}'")),
    };

    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }

    private_file_permissions(&temp)?;
    validate_regular_output(&temp)?;

    fs::rename(&temp, &derivative).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot atomically replace visual derivative {}: {error}", derivative.display())
    })?;
    sync_directory(directory)?;

    // The marker is derived cache state only. A marker write failure must not
    // make the successfully replaced derivative unusable or become a second
    // source of truth. The next Apply simply regenerates if the marker is absent.
    if let Err(error) = write_marker_atomic(&marker, &fingerprint) {
        eprintln!("visual derivative cache marker was not updated: {error}");
    }

    Ok(MaterializeOutcome { changed: true, derivative })
}

fn load_project(path: &Path) -> Result<ProjectDocument, String> {
    let raw = fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_slice(&raw)
        .map_err(|error| format!("invalid imported project metadata {}: {error}", path.display()))
}

fn safe_child(directory: &Path, name: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(name);
    let mut components = candidate.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !name.trim().is_empty() => {
            Ok(directory.join(candidate))
        }
        _ => Err(format!("unsafe imported media filename '{name}'")),
    }
}

fn validate_regular_source(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect source {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("source must be a regular non-symlink file: {}", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("source is empty: {}", path.display()));
    }
    Ok(())
}

fn valid_existing_derivative(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    !metadata.file_type().is_symlink() && metadata.is_file() && metadata.len() > 0
}

fn validate_regular_output(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect derivative {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(format!(
            "visual derivative is not a non-empty regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

fn recipe_fingerprint(path: &Path, visual: VisualAdjustments) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect visual source {}: {error}", path.display()))?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    Ok(format!(
        "v1\nsource_len={}\nsource_mtime_ns={modified_ns}\nbrightness={:08x}\ncontrast={:08x}\nsaturation={:08x}\nhue={:08x}\n",
        metadata.len(),
        visual.brightness.to_bits(),
        visual.contrast.to_bits(),
        visual.saturation.to_bits(),
        visual.hue_degrees.to_bits(),
    ))
}

fn marker_matches(path: &Path, expected: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|value| value == expected)
}

fn derivative_temp_path(derivative: &Path) -> Result<PathBuf, String> {
    let parent =
        derivative.parent().ok_or_else(|| "renderer derivative has no parent".to_string())?;
    let stem = derivative
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "renderer derivative has an invalid filename".to_string())?;
    let extension = derivative
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "renderer derivative has no extension".to_string())?;

    Ok(parent.join(format!(".{stem}.visual-{}.{}", std::process::id(), extension)))
}

fn materialize_still(input: &Path, output: &Path, visual: VisualAdjustments) -> Result<(), String> {
    let metadata = fs::symlink_metadata(input)
        .map_err(|error| format!("cannot inspect still source {}: {error}", input.display()))?;
    if metadata.len() > STILL_MAX_INPUT_BYTES {
        return Err(format!(
            "still visual source exceeds {} MiB decode bound",
            STILL_MAX_INPUT_BYTES / (1024 * 1024)
        ));
    }

    let file = File::open(input)
        .map_err(|error| format!("cannot open still source {}: {error}", input.display()))?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| format!("cannot determine still visual source format: {error}"))?;
    let format =
        reader.format().ok_or_else(|| "cannot determine still visual source format".to_string())?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) {
        return Err(format!("unsupported still visual source format: {format:?}"));
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(STILL_MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(STILL_MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(STILL_DECODER_MAX_ALLOC);
    reader.limits(limits);

    let mut rgba = reader
        .decode()
        .map_err(|error| format!("cannot decode still visual source: {error}"))?
        .to_rgba8();
    apply_visual_adjustments(&mut rgba, visual);

    let mut file =
        OpenOptions::new().write(true).create_new(true).open(output).map_err(|error| {
            format!("cannot create still visual derivative {}: {error}", output.display())
        })?;
    DynamicImage::ImageRgba8(rgba)
        .write_to(&mut file, ImageFormat::Png)
        .map_err(|error| format!("cannot encode still visual derivative: {error}"))?;
    file.flush().map_err(|error| format!("cannot flush still visual derivative: {error}"))?;
    file.sync_all().map_err(|error| format!("cannot sync still visual derivative: {error}"))?;
    Ok(())
}

fn private_file_permissions(path: &Path) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("cannot inspect private derivative {}: {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("cannot protect private derivative {}: {error}", path.display()))
}

fn write_marker_atomic(path: &Path, value: &str) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "cache marker has no parent".to_string())?;
    let temp = parent.join(format!(".we-layerd-visual-v1.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temp);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("cannot create visual cache marker temp: {error}"))?;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("cannot write visual cache marker: {error}"))?;
    file.sync_all().map_err(|error| format!("cannot sync visual cache marker: {error}"))?;
    private_file_permissions(&temp)?;

    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot atomically publish visual cache marker: {error}")
    })?;
    sync_directory(parent)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync directory {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use image_rs::{Rgba, RgbaImage};
    use we_core::wallpaper::{settings::VisualAdjustments, WallpaperType};

    use super::*;

    fn temp_directory(label: &str) -> PathBuf {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
        let path = std::env::temp_dir().join(format!("we-gui-{label}-{suffix}"));
        fs::create_dir_all(&path).expect("create temp");
        path
    }

    fn write_png(path: &Path, pixel: [u8; 4]) {
        let image = RgbaImage::from_pixel(4, 4, Rgba(pixel));
        DynamicImage::ImageRgba8(image)
            .save_with_format(path, ImageFormat::Png)
            .expect("write png");
    }

    fn image_entry(directory: &Path) -> WallpaperEntry {
        let project_json = directory.join("project.json");
        let project = serde_json::json!({
            "title": "fixture",
            "type": "video",
            "file": "rendered.png",
            "we_layerd": {
                "kind": "image",
                "version": 2,
                "original_name": "fixture.png",
                "original_file": "original.png",
                "rendered_file": "rendered.png"
            }
        });
        fs::write(&project_json, serde_json::to_vec_pretty(&project).expect("json"))
            .expect("project");

        WallpaperEntry {
            id: "local-media-test".to_string(),
            project_json,
            title: "fixture".to_string(),
            ty: WallpaperType::Video,
            preview: None,
            source_file: Some(directory.join("rendered.png")),
            source_name: Some("fixture.png".to_string()),
            imported: true,
            recent_key: 0,
        }
    }

    #[test]
    fn still_recipe_is_atomic_cached_and_original_preserving() {
        let directory = temp_directory("visual-materialize");
        let original = directory.join("original.png");
        let rendered = directory.join("rendered.png");
        write_png(&original, [60, 120, 180, 255]);
        fs::copy(&original, &rendered).expect("baseline derivative");
        let entry = image_entry(&directory);
        let original_before = fs::read(&original).expect("original bytes");
        let baseline_rendered = fs::read(&rendered).expect("baseline rendered");

        let mut settings = WallpaperSettings::default();
        settings.visual_adjustments = VisualAdjustments {
            brightness: 0.2,
            contrast: 1.3,
            saturation: 0.7,
            hue_degrees: 35.0,
        };

        let first = materialize_sync(&entry, &settings).expect("materialize");
        assert!(first.changed);
        assert_eq!(fs::read(&original).expect("original after"), original_before);
        assert_ne!(fs::read(&rendered).expect("rendered after"), baseline_rendered);
        assert!(directory.join(MARKER_FILE_NAME).is_file());

        let second = materialize_sync(&entry, &settings).expect("cached materialize");
        assert!(!second.changed);

        settings.visual_adjustments = VisualAdjustments::default();
        let reset = materialize_sync(&entry, &settings).expect("reset materialize");
        assert!(reset.changed);
        assert_eq!(fs::read(&original).expect("original reset"), original_before);

        let reset_pixel = image_rs::open(&rendered).expect("reset rendered").to_rgba8();
        assert_eq!(reset_pixel.get_pixel(0, 0).0, [60, 120, 180, 255]);

        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn failed_still_materialization_preserves_previous_derivative() {
        let directory = temp_directory("visual-materialize-failure");
        fs::write(directory.join("original.png"), b"not-a-png").expect("invalid original");
        fs::write(directory.join("rendered.png"), b"previous-good-derivative")
            .expect("previous derivative");
        let entry = image_entry(&directory);
        let before = fs::read(directory.join("rendered.png")).expect("before");

        let mut settings = WallpaperSettings::default();
        settings.visual_adjustments.brightness = 0.25;
        assert!(materialize_sync(&entry, &settings).is_err());
        assert_eq!(fs::read(directory.join("rendered.png")).expect("after"), before);

        fs::remove_dir_all(directory).expect("cleanup");
    }
}
