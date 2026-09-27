use iced::Task;
use we_core::wallpaper::{
    settings::{RenderResolution, Rotation, WallpaperSettings},
    WallpaperEntry,
};

use crate::{
    domain::runtime_status::RuntimeStatus,
    services::{config, detail_preview, runtime, visual_materialize},
    ui::sidebar::detail as wallpaper_detail,
};

use super::{App, Message};

pub(crate) fn update(app: &mut App, message: wallpaper_detail::DetailMessage) -> Task<Message> {
    use wallpaper_detail::{DetailMessage, ResolutionMode};

    let message = match message {
        DetailMessage::PreviewDragStart => {
            app.detail_drag_active = true;
            app.detail_drag_last = None;
            return Task::none();
        }
        DetailMessage::PreviewPointerMoved { position, width, height } => {
            if !app.detail_drag_active {
                return Task::none();
            }
            let previous = app.detail_drag_last.replace(position);
            let Some(previous) = previous else {
                return Task::none();
            };
            if width <= 1.0 || height <= 1.0 {
                return Task::none();
            }

            let Some(selected_id) = app.selected_id.clone() else {
                return Task::none();
            };
            let profile = app.launch_settings.wallpapers.entry(selected_id).or_default();
            profile.position_x =
                dragged_position(profile.position_x, position.x - previous.x, width);
            profile.position_y =
                dragged_position(profile.position_y, position.y - previous.y, height);
            refresh_detail_drag_preview(app);
            return Task::none();
        }
        DetailMessage::PreviewDragEnd => {
            let was_dragging = app.detail_drag_active;
            app.detail_drag_active = false;
            app.detail_drag_last = None;
            if was_dragging {
                refresh_detail_preview(app);
                if let Err(error) = persist_wallpaper_profiles(app) {
                    app.runtime_status = RuntimeStatus::ConfigSaveFailed(error.clone());
                    eprintln!("failed to save dragged wallpaper position: {error}");
                }
            }
            return Task::none();
        }
        message => message,
    };

    if matches!(message, DetailMessage::Apply) {
        if app.visual_materialize_busy {
            return Task::none();
        }

        let Some(selected_id) = app.selected_id.clone() else {
            return super::update::update(app, Message::PlayPressed);
        };
        let Some(entry) = app.entries.iter().find(|entry| entry.id == selected_id).cloned() else {
            return super::update::update(app, Message::PlayPressed);
        };
        if !entry.imported {
            return super::update::update(app, Message::PlayPressed);
        }

        if let Err(error) = persist_wallpaper_profiles(app) {
            app.runtime_status = RuntimeStatus::ConfigSaveFailed(error.clone());
            eprintln!("failed to save visual recipe before apply: {error}");
            return Task::none();
        }

        let settings =
            app.launch_settings.wallpapers.get(&selected_id).cloned().unwrap_or_default();
        app.visual_materialize_busy = true;

        return Task::perform(visual_materialize::materialize(entry, settings), move |result| {
            Message::VisualDerivativePrepared { wallpaper_id: selected_id, result }
        });
    }
    match message {
        DetailMessage::SelectTab(tab) => {
            app.detail_tab = tab;
            return Task::none();
        }
        DetailMessage::TogglePlayback => {
            if !app.selected_wallpaper_is_running() {
                return super::update::update(app, Message::PlayPressed);
            }
            if runtime::send_control("pause") {
                app.playback_paused = true;
            }
            return Task::none();
        }
        DetailMessage::Stop => return super::update::update(app, Message::StopPressed),
        DetailMessage::ToggleOutput(output) => {
            return super::update::update(app, Message::ToggleOutput(output))
        }
        _ => {}
    }
    if let DetailMessage::PickPath { key, directory } = message {
        let title = app.language.text(crate::domain::i18n::Text::SelectPropertyPath);
        return Task::perform(
            async move {
                let dialog = rfd::FileDialog::new().set_title(title);
                let path = if directory { dialog.pick_folder() } else { dialog.pick_file() };
                DetailMessage::PathPicked { key, path: path.map(|path| path.display().to_string()) }
            },
            Message::Detail,
        );
    }

    let Some(selected_id) = app.selected_id.clone() else {
        return Task::none();
    };
    let profile = app.launch_settings.wallpapers.entry(selected_id).or_default();
    match message {
        DetailMessage::Apply
        | DetailMessage::TogglePlayback
        | DetailMessage::Stop
        | DetailMessage::ToggleOutput(_)
        | DetailMessage::SelectTab(_)
        | DetailMessage::PreviewDragStart
        | DetailMessage::PreviewPointerMoved { .. }
        | DetailMessage::PreviewDragEnd => {
            unreachable!("detail action handled before profile mutation")
        }
        DetailMessage::FpsChanged(value) => {
            if let Ok(fps) = value.parse::<u32>() {
                profile.fps = fps.clamp(1, 360);
            }
        }
        DetailMessage::SpeedChanged(value) => profile.speed = value,
        DetailMessage::VolumeChanged(value) => profile.volume = value,
        DetailMessage::MutedChanged(value) => profile.muted = value,
        DetailMessage::MsaaChanged(value) => profile.msaa_samples = value.max(1),
        DetailMessage::ResolutionModeChanged(ResolutionMode::Automatic) => {
            profile.render_resolution = RenderResolution::Automatic;
            app.resolution_width.clear();
            app.resolution_height.clear();
        }
        DetailMessage::ResolutionModeChanged(ResolutionMode::Fixed) => {
            let width = app.resolution_width.parse().unwrap_or(1920).max(1);
            let height = app.resolution_height.parse().unwrap_or(1080).max(1);
            profile.render_resolution = RenderResolution::Fixed { width, height };
            app.resolution_width = width.to_string();
            app.resolution_height = height.to_string();
        }
        DetailMessage::ResolutionWidthChanged(value) => {
            app.resolution_width = value;
            sync_fixed_resolution(profile, &app.resolution_width, &app.resolution_height);
        }
        DetailMessage::ResolutionHeightChanged(value) => {
            app.resolution_height = value;
            sync_fixed_resolution(profile, &app.resolution_width, &app.resolution_height);
        }
        DetailMessage::FillModeChanged(value) => profile.fill_mode = value,
        DetailMessage::RotationChanged(value) => profile.rotation_degrees = value,
        DetailMessage::ZoomChanged(value) => profile.zoom = bounded_zoom(value),
        DetailMessage::PositionXChanged(value) => profile.position_x = bounded_position(value),
        DetailMessage::PositionYChanged(value) => profile.position_y = bounded_position(value),
        DetailMessage::CenterPosition => {
            profile.position_x = 0.0;
            profile.position_y = 0.0;
        }
        DetailMessage::BrightnessChanged(value) => {
            profile.visual_adjustments.brightness = bounded_visual(value, -1.0, 1.0, 0.0);
        }
        DetailMessage::ContrastChanged(value) => {
            profile.visual_adjustments.contrast = bounded_visual(value, 0.0, 2.0, 1.0);
        }
        DetailMessage::SaturationChanged(value) => {
            profile.visual_adjustments.saturation = bounded_visual(value, 0.0, 2.0, 1.0);
        }
        DetailMessage::HueChanged(value) => {
            profile.visual_adjustments.hue_degrees = bounded_visual(value, -180.0, 180.0, 0.0);
        }
        DetailMessage::ResetVisualAdjustments => {
            profile.visual_adjustments = Default::default();
        }
        DetailMessage::ResetTransform => reset_transform(profile),
        DetailMessage::PropertyChanged { key, value } => {
            profile.user_properties.insert(key, value);
        }
        DetailMessage::PathPicked { key, path } => {
            if let Some(path) = path {
                profile.user_properties.insert(key, serde_json::Value::String(path));
            }
        }
        DetailMessage::PickPath { .. } => {
            unreachable!("path picker handled before profile mutation")
        }
        DetailMessage::ResetProperties => profile.user_properties.clear(),
    }
    refresh_detail_preview(app);
    if let Err(error) = persist_wallpaper_profiles(app) {
        app.runtime_status = RuntimeStatus::ConfigSaveFailed(error.clone());
        eprintln!("failed to save config: {error}");
    }
    Task::none()
}

pub(crate) fn load_detail_preview_for_selection(
    app: &mut App,
    entry: &WallpaperEntry,
    settings: &WallpaperSettings,
) {
    app.detail_drag_active = false;
    app.detail_drag_last = None;
    app.detail_preview_source = None;
    app.detail_preview = None;
    app.detail_preview_error = None;

    if !entry.imported {
        return;
    }

    let Some(path) = entry.preview.as_deref() else {
        app.detail_preview_error = Some("Imported wallpaper has no preview asset".to_string());
        return;
    };

    match detail_preview::load(path) {
        Ok(source) => {
            match detail_preview::render(&source, settings) {
                Ok(handle) => app.detail_preview = Some(handle),
                Err(error) => app.detail_preview_error = Some(error),
            }
            app.detail_preview_source = Some(source);
        }
        Err(error) => app.detail_preview_error = Some(error),
    }
}

fn refresh_detail_preview(app: &mut App) {
    refresh_detail_preview_with(app, false);
}

fn refresh_detail_drag_preview(app: &mut App) {
    refresh_detail_preview_with(app, true);
}

fn refresh_detail_preview_with(app: &mut App, lightweight: bool) {
    let Some(source) = app.detail_preview_source.as_ref() else {
        return;
    };
    let Some(selected_id) = app.selected_id.as_deref() else {
        return;
    };
    let Some(settings) = app.launch_settings.wallpapers.get(selected_id) else {
        return;
    };

    let rendered = if lightweight {
        detail_preview::render_drag(source, settings)
    } else {
        detail_preview::render(source, settings)
    };

    match rendered {
        Ok(handle) => {
            app.detail_preview = Some(handle);
            app.detail_preview_error = None;
        }
        Err(error) => {
            app.detail_preview = None;
            app.detail_preview_error = Some(error);
        }
    }
}

fn dragged_position(current: f32, delta_pixels: f32, preview_extent: f32) -> f32 {
    if !delta_pixels.is_finite() || !preview_extent.is_finite() || preview_extent <= 1.0 {
        return bounded_position(current);
    }

    bounded_position(current + (2.0 * delta_pixels / preview_extent))
}

fn bounded_zoom(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.1, 4.0)
    } else {
        1.0
    }
}

fn bounded_position(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn bounded_visual(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

fn reset_transform(profile: &mut WallpaperSettings) {
    profile.zoom = 1.0;
    profile.position_x = 0.0;
    profile.position_y = 0.0;
    profile.rotation_degrees = Rotation::Deg0;
}

fn sync_fixed_resolution(profile: &mut WallpaperSettings, width: &str, height: &str) {
    let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) else {
        return;
    };
    profile.render_resolution =
        RenderResolution::Fixed { width: width.max(1), height: height.max(1) };
}

pub(crate) fn set_resolution_inputs(app: &mut App, profile: &WallpaperSettings) {
    match profile.render_resolution {
        RenderResolution::Automatic => {
            app.resolution_width.clear();
            app.resolution_height.clear();
        }
        RenderResolution::Fixed { width, height } => {
            app.resolution_width = width.to_string();
            app.resolution_height = height.to_string();
        }
    }
}

pub(crate) fn persist_wallpaper_profiles(app: &App) -> Result<(), String> {
    config::persist_wallpapers(&app.config_path, &app.launch_settings.wallpapers)
}

pub(crate) fn persist_playback_config(app: &App) -> Result<(), String> {
    if !app.outputs.is_empty() || !app.launch_settings.outputs.is_empty() {
        return config::persist_wallpapers_playlists_profiles_and_outputs(
            &app.config_path,
            &app.launch_settings.wallpapers,
            &app.launch_settings.playlists,
            &app.launch_settings.profiles,
            &app.launch_settings.outputs,
        );
    }
    let Some(selected_id) = app.selected_id.as_deref() else {
        return Ok(());
    };
    let Some(entry) = app.entries.iter().find(|entry| entry.id == selected_id) else {
        return Ok(());
    };

    config::persist_selected(&app.config_path, &app.launch_settings, entry)
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_position, bounded_visual, bounded_zoom, dragged_position, reset_transform,
    };
    use we_core::wallpaper::settings::{Rotation, WallpaperFillMode, WallpaperSettings};

    #[test]
    fn transform_controls_use_backend_bounds() {
        assert_eq!(bounded_zoom(0.05), 0.1);
        assert_eq!(bounded_zoom(0.5), 0.5);
        assert_eq!(bounded_zoom(2.0), 2.0);
        assert_eq!(bounded_zoom(8.0), 4.0);
        assert_eq!(bounded_zoom(f32::NAN), 1.0);

        assert_eq!(bounded_position(-2.0), -1.0);
        assert_eq!(bounded_position(0.25), 0.25);
        assert_eq!(bounded_position(2.0), 1.0);
        assert_eq!(bounded_position(f32::INFINITY), 0.0);
    }

    #[test]
    fn visual_controls_use_canonical_bounds() {
        assert_eq!(bounded_visual(-3.0, -1.0, 1.0, 0.0), -1.0);
        assert_eq!(bounded_visual(0.25, -1.0, 1.0, 0.0), 0.25);
        assert_eq!(bounded_visual(3.0, 0.0, 2.0, 1.0), 2.0);
        assert_eq!(bounded_visual(f32::NAN, 0.0, 2.0, 1.0), 1.0);
    }

    #[test]
    fn drag_delta_maps_preview_motion_into_normalized_position() {
        assert_eq!(dragged_position(0.0, 50.0, 200.0), 0.5);
        assert_eq!(dragged_position(0.0, -50.0, 200.0), -0.5);
        assert_eq!(dragged_position(0.9, 50.0, 200.0), 1.0);
        assert_eq!(dragged_position(-0.9, -50.0, 200.0), -1.0);
    }

    #[test]
    fn reset_transform_preserves_non_transform_settings() {
        let mut profile = WallpaperSettings::default();
        profile.zoom = 3.0;
        profile.position_x = -0.8;
        profile.position_y = 0.6;
        profile.rotation_degrees = Rotation::Deg270;
        profile.fill_mode = WallpaperFillMode::Fit;

        reset_transform(&mut profile);

        assert_eq!(profile.zoom, 1.0);
        assert_eq!(profile.position_x, 0.0);
        assert_eq!(profile.position_y, 0.0);
        assert_eq!(profile.rotation_degrees, Rotation::Deg0);
        assert_eq!(profile.fill_mode, WallpaperFillMode::Fit);
    }
}
