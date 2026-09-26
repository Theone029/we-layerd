use we_core::{
    config::ScaleMode,
    presentation::{compute_presentation_geometry, PresentationGeometry},
    wallpaper::settings::Rotation,
};

const DEFAULT_TARGET_WIDTH: u32 = 1920;
const DEFAULT_TARGET_HEIGHT: u32 = 1080;
const MAX_TARGET_DIMENSION: u32 = 16_384;

#[derive(Debug, Clone)]
pub(crate) struct StillEditorState {
    pub(crate) title: String,
    pub(crate) source_dimensions: Option<(u32, u32)>,
    pub(crate) target_width: String,
    pub(crate) target_height: String,
    pub(crate) scale_mode: ScaleMode,
    pub(crate) zoom: f32,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) rotation: Rotation,
}

impl Default for StillEditorState {
    fn default() -> Self {
        Self {
            title: String::new(),
            source_dimensions: None,
            target_width: DEFAULT_TARGET_WIDTH.to_string(),
            target_height: DEFAULT_TARGET_HEIGHT.to_string(),
            scale_mode: ScaleMode::Cover,
            zoom: 1.0,
            position_x: 0.0,
            position_y: 0.0,
            rotation: Rotation::Deg0,
        }
    }
}

impl StillEditorState {
    pub(crate) fn target_extent(&self) -> (u32, u32) {
        (
            parse_dimension(&self.target_width, DEFAULT_TARGET_WIDTH),
            parse_dimension(&self.target_height, DEFAULT_TARGET_HEIGHT),
        )
    }

    pub(crate) fn preview_geometry(&self) -> Option<PresentationGeometry> {
        self.source_dimensions.map(|source| self.preview_geometry_for(source))
    }

    pub(crate) fn preview_geometry_for(
        &self,
        source_dimensions: (u32, u32),
    ) -> PresentationGeometry {
        let (source_width, source_height) = source_dimensions;

        let (render_width, render_height) = match self.rotation {
            Rotation::Deg90 | Rotation::Deg270 => (source_height.max(1), source_width.max(1)),
            Rotation::Deg0 | Rotation::Deg180 => (source_width.max(1), source_height.max(1)),
        };

        let (target_width, target_height) = self.target_extent();

        compute_presentation_geometry(
            self.scale_mode,
            render_width,
            render_height,
            target_width,
            target_height,
            self.zoom as f64,
            self.position_x as f64,
            self.position_y as f64,
        )
    }

    pub(crate) fn reset_transform(&mut self) {
        self.zoom = 1.0;
        self.position_x = 0.0;
        self.position_y = 0.0;
        self.rotation = Rotation::Deg0;
    }
}

fn parse_dimension(value: &str, fallback: u32) -> u32 {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
        .clamp(1, MAX_TARGET_DIMENSION)
}

#[cfg(test)]
mod tests {
    use super::StillEditorState;
    use we_core::{config::ScaleMode, wallpaper::settings::Rotation};

    #[test]
    fn defaults_match_neutral_runtime_transform() {
        let state = StillEditorState::default();

        assert_eq!(state.target_extent(), (1920, 1080));
        assert_eq!(state.scale_mode, ScaleMode::Cover);
        assert_eq!(state.zoom, 1.0);
        assert_eq!(state.position_x, 0.0);
        assert_eq!(state.position_y, 0.0);
        assert_eq!(state.rotation, Rotation::Deg0);
    }

    #[test]
    fn invalid_target_text_falls_back_without_destroying_input() {
        let mut state = StillEditorState::default();

        state.target_width = "nope".to_string();
        state.target_height = "0".to_string();

        assert_eq!(state.target_extent(), (1920, 1080));
        assert_eq!(state.target_width, "nope");
        assert_eq!(state.target_height, "0");
    }

    #[test]
    fn preview_uses_shared_sub_100_geometry() {
        let mut state = StillEditorState::default();

        state.scale_mode = ScaleMode::Stretch;
        state.zoom = 0.5;

        let geometry = state.preview_geometry_for((1920, 1080));

        assert_eq!(geometry.viewport_width, 960);
        assert_eq!(geometry.viewport_height, 540);
        assert!(geometry.viewport_source.is_none());
    }

    #[test]
    fn preview_uses_shared_over_100_crop_and_pan() {
        let mut state = StillEditorState::default();

        state.scale_mode = ScaleMode::Stretch;
        state.zoom = 2.0;
        state.position_x = 1.0;
        state.position_y = 1.0;

        let geometry = state.preview_geometry_for((1920, 1080));
        let source = geometry.viewport_source.expect("zoomed source crop");

        assert_eq!(source.x, 960.0);
        assert_eq!(source.y, 540.0);
        assert_eq!(source.width, 960.0);
        assert_eq!(source.height, 540.0);
    }

    #[test]
    fn quarter_turn_swaps_source_extent_before_geometry() {
        let mut state = StillEditorState::default();

        state.target_width = "300".to_string();
        state.target_height = "400".to_string();
        state.scale_mode = ScaleMode::Stretch;
        state.rotation = Rotation::Deg90;

        let geometry = state.preview_geometry_for((400, 300));

        assert_eq!(geometry.render_width, 300);
        assert_eq!(geometry.render_height, 400);
        assert_eq!(geometry.viewport_width, 300);
        assert_eq!(geometry.viewport_height, 400);
    }

    #[test]
    fn reset_preserves_target_and_scaling_mode() {
        let mut state = StillEditorState::default();

        state.target_width = "2560".to_string();
        state.target_height = "1440".to_string();
        state.scale_mode = ScaleMode::Fit;
        state.zoom = 3.0;
        state.position_x = -0.8;
        state.position_y = 0.6;
        state.rotation = Rotation::Deg270;

        state.reset_transform();

        assert_eq!(state.target_extent(), (2560, 1440));
        assert_eq!(state.scale_mode, ScaleMode::Fit);
        assert_eq!(state.zoom, 1.0);
        assert_eq!(state.position_x, 0.0);
        assert_eq!(state.position_y, 0.0);
        assert_eq!(state.rotation, Rotation::Deg0);
    }
}
