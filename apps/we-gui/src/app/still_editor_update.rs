use std::path::PathBuf;

use iced::Task;
use we_core::{
    config::ScaleMode,
    wallpaper::settings::{RenderResolution, WallpaperFillMode, WallpaperSettings},
};

use crate::{
    domain::still_editor::StillEditorState,
    services::{still_import, still_ingress},
    ui::sidebar::still_editor::StillEditorMessage,
};

use super::{App, Message};

pub(crate) fn update(app: &mut App, message: StillEditorMessage) -> Task<Message> {
    if app.still_editor_busy {
        return Task::none();
    }

    match message {
        StillEditorMessage::Close => {
            if let Some(draft) = app.still_editor_draft.take() {
                still_ingress::discard_staged(&draft.source_path);
            }

            app.still_editor_error = None;
            app.still_editor_busy = false;
            app.sidebar = None;
        }

        StillEditorMessage::ChooseImage => {
            app.still_editor_error = None;
            app.still_editor_busy = true;

            return Task::perform(still_ingress::pick(), Message::StillIngressPicked);
        }

        StillEditorMessage::Import => {
            let Some(draft) = app.still_editor_draft.as_ref() else {
                app.still_editor_error = Some("choose an image before importing".to_string());
                return Task::none();
            };

            let workshop_root = PathBuf::from(app.ui_settings.workshop_path.trim());
            if workshop_root.as_os_str().is_empty() {
                app.still_editor_error =
                    Some("configure the wallpaper library path before importing".to_string());
                return Task::none();
            }

            let staged_path = draft.source_path.clone();
            let original_name = draft.original_name.clone();
            let title = app.still_editor.title.clone();
            let editor = app.still_editor.clone();

            app.still_editor_error = None;
            app.still_editor_busy = true;

            return Task::perform(
                still_import::import(workshop_root, staged_path.clone(), original_name, title),
                move |result| Message::StillImportCompleted {
                    staged_path,
                    editor,
                    result: result.map(|imported| imported.id),
                },
            );
        }

        StillEditorMessage::TitleChanged(value) => {
            app.still_editor.title = value.chars().take(160).collect();
        }

        StillEditorMessage::TargetWidthChanged(value) => {
            app.still_editor.target_width = value;
            refresh_preview(app);
        }

        StillEditorMessage::TargetHeightChanged(value) => {
            app.still_editor.target_height = value;
            refresh_preview(app);
        }

        StillEditorMessage::ScaleModeChanged(value) => {
            app.still_editor.scale_mode = value;
            refresh_preview(app);
        }

        StillEditorMessage::ZoomChanged(value) => {
            app.still_editor.zoom = bounded_zoom(value);
            refresh_preview(app);
        }

        StillEditorMessage::PositionXChanged(value) => {
            app.still_editor.position_x = bounded_position(value);
            refresh_preview(app);
        }

        StillEditorMessage::PositionYChanged(value) => {
            app.still_editor.position_y = bounded_position(value);
            refresh_preview(app);
        }

        StillEditorMessage::CenterPosition => {
            app.still_editor.position_x = 0.0;
            app.still_editor.position_y = 0.0;
            refresh_preview(app);
        }

        StillEditorMessage::RotationChanged(value) => {
            app.still_editor.rotation = value;
            refresh_preview(app);
        }

        StillEditorMessage::ResetTransform => {
            app.still_editor.reset_transform();
            refresh_preview(app);
        }
    }

    Task::none()
}

fn refresh_preview(app: &mut App) {
    let state = app.still_editor.clone();
    let Some(draft) = app.still_editor_draft.as_mut() else {
        return;
    };

    match still_import::refresh_editor_preview(draft, &state) {
        Ok(()) => app.still_editor_error = None,
        Err(error) => app.still_editor_error = Some(error),
    }
}

pub(crate) fn wallpaper_settings_from_editor(state: &StillEditorState) -> WallpaperSettings {
    let (width, height) = state.target_extent();

    WallpaperSettings {
        render_resolution: RenderResolution::Fixed { width, height },
        fill_mode: match state.scale_mode {
            ScaleMode::Cover => WallpaperFillMode::Cover,
            ScaleMode::Fit => WallpaperFillMode::Fit,
            ScaleMode::Stretch => WallpaperFillMode::Stretch,
        },
        rotation_degrees: state.rotation,
        zoom: state.zoom,
        position_x: state.position_x,
        position_y: state.position_y,
        ..WallpaperSettings::default()
    }
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

#[cfg(test)]
mod tests {
    use super::{bounded_position, bounded_zoom, wallpaper_settings_from_editor};
    use crate::domain::still_editor::StillEditorState;
    use we_core::{
        config::ScaleMode,
        wallpaper::settings::{RenderResolution, Rotation, WallpaperFillMode},
    };

    #[test]
    fn editor_transform_bounds_match_runtime_controls() {
        assert_eq!(bounded_zoom(0.01), 0.1);
        assert_eq!(bounded_zoom(0.5), 0.5);
        assert_eq!(bounded_zoom(2.0), 2.0);
        assert_eq!(bounded_zoom(8.0), 4.0);
        assert_eq!(bounded_zoom(f32::NAN), 1.0);

        assert_eq!(bounded_position(-8.0), -1.0);
        assert_eq!(bounded_position(-0.5), -0.5);
        assert_eq!(bounded_position(0.5), 0.5);
        assert_eq!(bounded_position(8.0), 1.0);
        assert_eq!(bounded_position(f32::NAN), 0.0);
    }

    #[test]
    fn editor_projects_to_canonical_wallpaper_settings() {
        let mut state = StillEditorState::default();
        state.target_width = "2560".to_string();
        state.target_height = "1440".to_string();
        state.scale_mode = ScaleMode::Fit;
        state.rotation = Rotation::Deg270;
        state.zoom = 1.75;
        state.position_x = -0.25;
        state.position_y = 0.5;

        let settings = wallpaper_settings_from_editor(&state);

        assert_eq!(
            settings.render_resolution,
            RenderResolution::Fixed { width: 2560, height: 1440 }
        );
        assert_eq!(settings.fill_mode, WallpaperFillMode::Fit);
        assert_eq!(settings.rotation_degrees, Rotation::Deg270);
        assert_eq!(settings.zoom, 1.75);
        assert_eq!(settings.position_x, -0.25);
        assert_eq!(settings.position_y, 0.5);
    }
}
