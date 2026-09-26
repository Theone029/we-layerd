use iced::Task;

use crate::ui::sidebar::still_editor::StillEditorMessage;

use super::{App, Message};

pub(crate) fn update(app: &mut App, message: StillEditorMessage) -> Task<Message> {
    match message {
        StillEditorMessage::Close => {
            app.sidebar = None;
        }
        StillEditorMessage::TitleChanged(value) => {
            app.still_editor.title = value.chars().take(160).collect();
        }
        StillEditorMessage::TargetWidthChanged(value) => {
            app.still_editor.target_width = value;
        }
        StillEditorMessage::TargetHeightChanged(value) => {
            app.still_editor.target_height = value;
        }
        StillEditorMessage::ScaleModeChanged(value) => {
            app.still_editor.scale_mode = value;
        }
        StillEditorMessage::ZoomChanged(value) => {
            app.still_editor.zoom = bounded_zoom(value);
        }
        StillEditorMessage::PositionXChanged(value) => {
            app.still_editor.position_x = bounded_position(value);
        }
        StillEditorMessage::PositionYChanged(value) => {
            app.still_editor.position_y = bounded_position(value);
        }
        StillEditorMessage::RotationChanged(value) => {
            app.still_editor.rotation = value;
        }
        StillEditorMessage::ResetTransform => {
            app.still_editor.reset_transform();
        }
    }

    Task::none()
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
    use super::{bounded_position, bounded_zoom};

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
}
