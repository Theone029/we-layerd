use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use serde::Deserialize;
use we_core::ingress::MEDIA_INGRESS_MAX_PAYLOAD_BYTES;

const FFMPEG_PATH: &str = "/usr/bin/ffmpeg";
const FFPROBE_PATH: &str = "/usr/bin/ffprobe";
const MAX_SOURCE_DIMENSION: u32 = 16_384;
const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 4 * 1024;
const MAX_GIF_DERIVATIVE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MotionKind {
    Gif,
    Mp4,
    Webm,
    Matroska,
}

impl MotionKind {
    pub(crate) fn original_extension(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Matroska => "mkv",
        }
    }

    pub(crate) fn rendered_extension(self) -> &'static str {
        match self {
            Self::Gif | Self::Webm => "webm",
            Self::Mp4 => "mp4",
            Self::Matroska => "mkv",
        }
    }

    pub(crate) fn metadata_kind(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Mp4 | Self::Webm | Self::Matroska => "video",
        }
    }

    pub(crate) fn format_name(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Matroska => "matroska",
        }
    }

    pub(crate) fn needs_transcode(self) -> bool {
        matches!(self, Self::Gif)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MotionProbe {
    pub(crate) kind: MotionKind,
    pub(crate) codec_name: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_name: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    #[serde(default)]
    format_name: String,
}

pub(crate) fn probe_motion(path: &Path) -> Result<MotionProbe, String> {
    validate_source(path)?;

    let output = Command::new(FFPROBE_PATH)
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
            "-probesize",
            "5242880",
            "-analyzeduration",
            "5000000",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height",
            "-show_entries",
            "format=format_name",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|error| format!("failed to execute {FFPROBE_PATH}: {error}"))?;

    if !output.status.success() {
        return Err(format!("ffprobe rejected media: {}", bounded_error(&output)));
    }

    parse_probe_json(&output.stdout)
}

pub(crate) fn extract_preview_png(path: &Path) -> Result<Vec<u8>, String> {
    validate_source(path)?;

    let output = Command::new(FFMPEG_PATH)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
            "-probesize",
            "5242880",
            "-analyzeduration",
            "5000000",
            "-i",
        ])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-an",
            "-vf",
            "scale=480:270:force_original_aspect_ratio=decrease",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "pipe:1",
        ])
        .output()
        .map_err(|error| format!("failed to execute {FFMPEG_PATH}: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "ffmpeg could not extract a media preview: {}",
            bounded_error(&output)
        ));
    }

    if output.stdout.is_empty() {
        return Err("ffmpeg produced an empty media preview".to_string());
    }

    if output.stdout.len() > MAX_PREVIEW_BYTES {
        return Err(format!(
            "ffmpeg media preview exceeds {} MiB bound",
            MAX_PREVIEW_BYTES / (1024 * 1024)
        ));
    }

    Ok(output.stdout)
}

pub(crate) fn transcode_gif_to_webm(input: &Path, output: &Path) -> Result<(), String> {
    validate_source(input)?;

    if output.exists() {
        return Err(format!("refusing to overwrite media derivative {}", output.display()));
    }

    let status = Command::new(FFMPEG_PATH)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-n",
            "-protocol_whitelist",
            "file,pipe",
            "-probesize",
            "5242880",
            "-analyzeduration",
            "5000000",
            "-i",
        ])
        .arg(input)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p",
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-row-mt",
            "1",
            "-fs",
            "1073741824",
        ])
        .arg(output)
        .status()
        .map_err(|error| format!("failed to execute {FFMPEG_PATH}: {error}"))?;

    if !status.success() {
        let _ = fs::remove_file(output);
        return Err("ffmpeg GIF-to-WebM conversion failed".to_string());
    }

    let metadata = fs::metadata(output).map_err(|error| {
        format!("cannot inspect media derivative {}: {error}", output.display())
    })?;

    if !metadata.is_file() || metadata.len() == 0 {
        let _ = fs::remove_file(output);
        return Err("ffmpeg produced an empty GIF derivative".to_string());
    }

    if metadata.len() > MAX_GIF_DERIVATIVE_BYTES {
        let _ = fs::remove_file(output);
        return Err("ffmpeg GIF derivative exceeded its output bound".to_string());
    }

    Ok(())
}

fn validate_source(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect media {}: {error}", path.display()))?;

    if metadata.file_type().is_symlink() {
        return Err(format!(
            "media source must be a regular file, not a symlink: {}",
            path.display()
        ));
    }

    if !metadata.is_file() {
        return Err(format!("media source is not a regular file: {}", path.display()));
    }

    if metadata.len() == 0 {
        return Err("media source is empty".to_string());
    }

    if metadata.len() > MEDIA_INGRESS_MAX_PAYLOAD_BYTES {
        return Err(format!(
            "media source exceeds the {} GiB transport bound",
            MEDIA_INGRESS_MAX_PAYLOAD_BYTES / (1024 * 1024 * 1024)
        ));
    }

    Ok(())
}

fn parse_probe_json(raw: &[u8]) -> Result<MotionProbe, String> {
    let document: ProbeDocument =
        serde_json::from_slice(raw).map_err(|error| format!("invalid ffprobe JSON: {error}"))?;

    let stream = document
        .streams
        .into_iter()
        .next()
        .ok_or_else(|| "media has no video stream".to_string())?;

    let width = stream
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| "media video stream has no positive width".to_string())?;
    let height = stream
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| "media video stream has no positive height".to_string())?;

    if width > MAX_SOURCE_DIMENSION || height > MAX_SOURCE_DIMENSION {
        return Err(format!(
            "media dimensions {width}x{height} exceed {MAX_SOURCE_DIMENSION}px bound"
        ));
    }

    let format_name = document.format.map(|format| format.format_name).unwrap_or_default();

    let kind = classify_motion(&format_name, &stream.codec_name)?;

    Ok(MotionProbe { kind, codec_name: stream.codec_name, width, height })
}

fn classify_motion(format_name: &str, codec_name: &str) -> Result<MotionKind, String> {
    let formats =
        format_name.split(',').map(str::trim).filter(|value| !value.is_empty()).collect::<Vec<_>>();

    if formats.contains(&"gif") || codec_name == "gif" {
        return Ok(MotionKind::Gif);
    }

    let webm_codec = matches!(codec_name, "vp8" | "vp9" | "av1");
    if formats.contains(&"webm") || (formats.contains(&"matroska") && webm_codec) {
        return Ok(MotionKind::Webm);
    }

    if formats.iter().any(|format| matches!(*format, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"))
    {
        return Ok(MotionKind::Mp4);
    }

    if formats.contains(&"matroska") {
        return Ok(MotionKind::Matroska);
    }

    Err(format!("unsupported motion-media container '{format_name}' with codec '{codec_name}'"))
}

fn bounded_error(output: &Output) -> String {
    let raw = if output.stderr.is_empty() { &output.stdout } else { &output.stderr };

    let end = raw.len().min(MAX_ERROR_BYTES);
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::{parse_probe_json, MotionKind, FFMPEG_PATH, FFPROBE_PATH};

    #[test]
    fn adapter_uses_absolute_pinned_executables() {
        assert_eq!(FFMPEG_PATH, "/usr/bin/ffmpeg");
        assert_eq!(FFPROBE_PATH, "/usr/bin/ffprobe");
    }

    #[test]
    fn parses_gif_probe() {
        let probe = parse_probe_json(
            br#"{"streams":[{"codec_name":"gif","width":320,"height":180}],"format":{"format_name":"gif"}}"#,
        )
        .expect("GIF probe");

        assert_eq!(probe.kind, MotionKind::Gif);
        assert_eq!((probe.width, probe.height), (320, 180));
    }

    #[test]
    fn parses_representative_mp4_probe() {
        let probe = parse_probe_json(
            br#"{"streams":[{"codec_name":"mpeg4","width":320,"height":180}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2"}}"#,
        )
        .expect("MP4 probe");

        assert_eq!(probe.kind, MotionKind::Mp4);
        assert_eq!(probe.codec_name, "mpeg4");
    }

    #[test]
    fn parses_vp9_webm_probe() {
        let probe = parse_probe_json(
            br#"{"streams":[{"codec_name":"vp9","width":320,"height":180}],"format":{"format_name":"matroska,webm"}}"#,
        )
        .expect("WebM probe");

        assert_eq!(probe.kind, MotionKind::Webm);
        assert_eq!(probe.codec_name, "vp9");
    }

    #[test]
    fn generic_matroska_is_kept_distinct_from_webm() {
        let probe = parse_probe_json(
            br#"{"streams":[{"codec_name":"mpeg4","width":320,"height":180}],"format":{"format_name":"matroska"}}"#,
        )
        .expect("Matroska probe");

        assert_eq!(probe.kind, MotionKind::Matroska);
    }

    #[test]
    fn rejects_unbounded_dimensions() {
        let error = parse_probe_json(
            br#"{"streams":[{"codec_name":"vp9","width":20000,"height":180}],"format":{"format_name":"webm"}}"#,
        )
        .expect_err("must reject");

        assert!(error.contains("exceed"));
    }
}
