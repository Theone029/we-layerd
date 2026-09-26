use crate::config::ScaleMode;
use we_core::presentation::compute_presentation_geometry;
pub(crate) use we_core::presentation::{PresentationGeometry, ViewportSource};

pub(crate) const FRACTIONAL_SCALE_DENOMINATOR: u32 = 120;

#[derive(Debug, Clone, Copy)]
pub(crate) struct GeometryInput {
    pub(crate) logical_width: u32,
    pub(crate) logical_height: u32,
    pub(crate) output_mode_width: u32,
    pub(crate) output_mode_height: u32,
    pub(crate) fallback_width: u32,
    pub(crate) fallback_height: u32,
    pub(crate) output_scale: u32,
    pub(crate) preferred_fractional_scale: u32,
    pub(crate) scale_mode: ScaleMode,
    pub(crate) render_size_override: Option<(u32, u32)>,
    pub(crate) zoom: f64,
    pub(crate) position_x: f64,
    pub(crate) position_y: f64,
}

fn render_scale_factor(input: GeometryInput) -> f64 {
    if input.preferred_fractional_scale >= FRACTIONAL_SCALE_DENOMINATOR {
        input.preferred_fractional_scale as f64 / FRACTIONAL_SCALE_DENOMINATOR as f64
    } else {
        input.output_scale.max(1) as f64
    }
}

pub(crate) fn compute_geometry(input: GeometryInput) -> PresentationGeometry {
    let viewport_width =
        if input.logical_width > 0 { input.logical_width } else { input.fallback_width }.max(1);
    let viewport_height =
        if input.logical_height > 0 { input.logical_height } else { input.fallback_height }.max(1);

    if let Some((render_width, render_height)) = input.render_size_override {
        return compute_presentation_geometry(
            input.scale_mode,
            render_width.max(1),
            render_height.max(1),
            viewport_width,
            viewport_height,
            input.zoom,
            input.position_x,
            input.position_y,
        );
    }

    if input.output_mode_width > 0 && input.output_mode_height > 0 {
        return compute_presentation_geometry(
            input.scale_mode,
            input.output_mode_width,
            input.output_mode_height,
            viewport_width,
            viewport_height,
            input.zoom,
            input.position_x,
            input.position_y,
        );
    }

    let scale = render_scale_factor(input);
    let render_width = (viewport_width as f64 * scale).round().max(1.0) as u32;
    let render_height = (viewport_height as f64 * scale).round().max(1.0) as u32;

    compute_presentation_geometry(
        input.scale_mode,
        render_width,
        render_height,
        viewport_width,
        viewport_height,
        input.zoom,
        input.position_x,
        input.position_y,
    )
}

fn transformed_frame_extent(width: u32, height: u32, rotation_degrees: u32) -> (u32, u32) {
    match rotation_degrees {
        90 | 270 => (height, width),
        _ => (width, height),
    }
}

pub(crate) struct OutputState {
    pub(crate) output_scale: u32,
    pub(crate) preferred_fractional_scale: u32,
    pub(crate) output_mode_width: u32,
    pub(crate) output_mode_height: u32,
    pub(crate) logical_width: u32,
    pub(crate) logical_height: u32,
    pub(crate) fallback_width: u32,
    pub(crate) fallback_height: u32,
    pub(crate) scale_mode: ScaleMode,
    pub(crate) render_size_override: Option<(u32, u32)>,
    pub(crate) zoom: f64,
    pub(crate) position_x: f64,
    pub(crate) position_y: f64,
    pub(crate) rotation_degrees: u32,
    pub(crate) geometry: PresentationGeometry,
}

impl OutputState {
    pub(crate) fn new(scale_mode: ScaleMode) -> Self {
        let mut output = Self {
            output_scale: 1,
            preferred_fractional_scale: 0,
            output_mode_width: 0,
            output_mode_height: 0,
            logical_width: 0,
            logical_height: 0,
            fallback_width: 1920,
            fallback_height: 1080,
            scale_mode,
            render_size_override: None,
            zoom: 1.0,
            position_x: 0.0,
            position_y: 0.0,
            rotation_degrees: 0,
            geometry: PresentationGeometry {
                render_width: 1920,
                render_height: 1080,
                viewport_width: 1920,
                viewport_height: 1080,
                viewport_source: None,
            },
        };
        output.recompute_geometry();
        output
    }

    pub(crate) fn render_scale_factor(&self) -> f64 {
        if self.preferred_fractional_scale >= FRACTIONAL_SCALE_DENOMINATOR {
            self.preferred_fractional_scale as f64 / FRACTIONAL_SCALE_DENOMINATOR as f64
        } else {
            self.output_scale.max(1) as f64
        }
    }

    pub(crate) fn recompute_geometry(&mut self) {
        self.geometry = compute_geometry(GeometryInput {
            logical_width: self.logical_width,
            logical_height: self.logical_height,
            output_mode_width: self.output_mode_width,
            output_mode_height: self.output_mode_height,
            fallback_width: self.fallback_width,
            fallback_height: self.fallback_height,
            output_scale: self.output_scale,
            preferred_fractional_scale: self.preferred_fractional_scale,
            scale_mode: self.scale_mode,
            render_size_override: self.render_size_override,
            zoom: self.zoom,
            position_x: self.position_x,
            position_y: self.position_y,
        });
    }

    pub(crate) fn geometry_for_frame(
        &self,
        frame_width: u32,
        frame_height: u32,
    ) -> PresentationGeometry {
        let viewport_width =
            if self.logical_width > 0 { self.logical_width } else { self.fallback_width }.max(1);
        let viewport_height =
            if self.logical_height > 0 { self.logical_height } else { self.fallback_height }.max(1);
        let (frame_width, frame_height) = transformed_frame_extent(
            frame_width.max(1),
            frame_height.max(1),
            self.rotation_degrees,
        );
        compute_presentation_geometry(
            self.scale_mode,
            frame_width,
            frame_height,
            viewport_width,
            viewport_height,
            self.zoom,
            self.position_x,
            self.position_y,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputState, FRACTIONAL_SCALE_DENOMINATOR};
    use crate::config::ScaleMode;

    #[test]
    fn render_extent_prefers_output_mode() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.output_mode_width = 2560;
        output.output_mode_height = 1440;
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.recompute_geometry();
        assert_eq!(output.geometry.render_width, 2560);
        assert_eq!(output.geometry.render_height, 1440);
    }

    #[test]
    fn render_extent_uses_logical_when_no_output_mode() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 100;
        output.logical_height = 50;
        output.recompute_geometry();
        assert_eq!(output.geometry.render_width, 100);
        assert_eq!(output.geometry.render_height, 50);
    }

    #[test]
    fn render_extent_falls_back_when_logical_is_zero() {
        let output = OutputState::new(ScaleMode::Stretch);
        assert_eq!(output.geometry.render_width, 1920);
        assert_eq!(output.geometry.render_height, 1080);
    }

    #[test]
    fn render_extent_uses_fractional_scale() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.preferred_fractional_scale = FRACTIONAL_SCALE_DENOMINATOR + 60;
        output.logical_width = 100;
        output.logical_height = 50;
        output.recompute_geometry();
        assert_eq!(output.geometry.render_width, 150);
        assert_eq!(output.geometry.render_height, 75);
    }

    #[test]
    fn fixed_render_resolution_is_presented_at_its_configured_extent() {
        let mut output = OutputState::new(ScaleMode::Cover);
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.output_mode_width = 3840;
        output.output_mode_height = 2160;
        output.render_size_override = Some((1280, 720));
        output.recompute_geometry();

        assert_eq!(output.geometry.render_width, 1280);
        assert_eq!(output.geometry.render_height, 720);
        assert_eq!(output.geometry.viewport_width, 1920);
        assert_eq!(output.geometry.viewport_height, 1080);
    }

    #[test]
    fn fit_geometry_uses_letterboxed_destination_for_16_by_9_to_16_by_10() {
        let mut output = OutputState::new(ScaleMode::Fit);
        output.output_mode_width = 2560;
        output.output_mode_height = 1440;
        output.logical_width = 1920;
        output.logical_height = 1200;
        output.recompute_geometry();

        assert_eq!(output.geometry.viewport_width, 1920);
        assert_eq!(output.geometry.viewport_height, 1080);
        assert!(output.geometry.viewport_source.is_none());
    }

    #[test]
    fn cover_geometry_crops_width_for_16_by_9_to_16_by_10() {
        let mut output = OutputState::new(ScaleMode::Cover);
        output.output_mode_width = 2560;
        output.output_mode_height = 1440;
        output.logical_width = 1920;
        output.logical_height = 1200;
        output.recompute_geometry();

        let source = output.geometry.viewport_source.expect("cover should crop");
        assert_eq!(output.geometry.viewport_width, 1920);
        assert_eq!(output.geometry.viewport_height, 1200);
        assert!(source.x > 0.0);
        assert_eq!(source.y, 0.0);
    }

    #[test]
    fn frame_geometry_uses_actual_buffer_extent_for_cover() {
        let mut output = OutputState::new(ScaleMode::Cover);
        output.logical_width = 1707;
        output.logical_height = 1067;
        output.output_mode_width = 2560;
        output.output_mode_height = 1600;
        output.recompute_geometry();

        let geometry = output.geometry_for_frame(1920, 1080);
        let source = geometry.viewport_source.expect("cover should crop the actual frame");
        assert_eq!(geometry.render_width, 1920);
        assert_eq!(geometry.render_height, 1080);
        assert!(source.x >= 0.0);
        assert!(source.y >= 0.0);
        assert!(source.x + source.width <= 1920.0);
        assert!(source.y + source.height <= 1080.0);
    }

    #[test]
    fn half_zoom_shrinks_final_stretch_destination() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.output_mode_width = 1920;
        output.output_mode_height = 1080;
        output.zoom = 0.5;
        output.recompute_geometry();

        assert_eq!(output.geometry.viewport_width, 960);
        assert_eq!(output.geometry.viewport_height, 540);
        assert!(output.geometry.viewport_source.is_none());
    }

    #[test]
    fn ten_percent_zoom_is_supported_and_bounded() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.output_mode_width = 1920;
        output.output_mode_height = 1080;
        output.zoom = 0.1;
        output.recompute_geometry();

        assert_eq!(output.geometry.viewport_width, 192);
        assert_eq!(output.geometry.viewport_height, 108);
    }

    #[test]
    fn zoom_two_crops_about_the_center() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.zoom = 2.0;
        output.recompute_geometry();

        let source = output.geometry.viewport_source.expect("zoom requires source crop");
        assert!((source.x - 480.0).abs() < 0.001);
        assert!((source.y - 270.0).abs() < 0.001);
        assert!((source.width - 960.0).abs() < 0.001);
        assert!((source.height - 540.0).abs() < 0.001);
    }

    #[test]
    fn pan_reaches_frame_edges_at_zoom_two() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.zoom = 2.0;

        output.position_x = -1.0;
        output.position_y = -1.0;
        output.recompute_geometry();
        let top_left = output.geometry.viewport_source.expect("transformed crop");
        assert!((top_left.x - 0.0).abs() < 0.001);
        assert!((top_left.y - 0.0).abs() < 0.001);

        output.position_x = 1.0;
        output.position_y = 1.0;
        output.recompute_geometry();
        let bottom_right = output.geometry.viewport_source.expect("transformed crop");
        assert!((bottom_right.x - 960.0).abs() < 0.001);
        assert!((bottom_right.y - 540.0).abs() < 0.001);
    }

    #[test]
    fn cover_crop_can_pan_without_extra_zoom() {
        let mut output = OutputState::new(ScaleMode::Cover);
        output.logical_width = 1440;
        output.logical_height = 2560;
        output.render_size_override = Some((2560, 1440));

        output.position_x = -1.0;
        output.recompute_geometry();
        let left = output.geometry.viewport_source.expect("cover crop");
        assert!((left.x - 0.0).abs() < 0.001);
        assert!((left.width - 810.0).abs() < 0.001);

        output.position_x = 1.0;
        output.recompute_geometry();
        let right = output.geometry.viewport_source.expect("cover crop");
        assert!((right.x - 1750.0).abs() < 0.001);
        assert!((right.width - 810.0).abs() < 0.001);
    }

    #[test]
    fn invalid_transform_values_fall_back_to_neutral() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 1920;
        output.logical_height = 1080;
        output.zoom = f64::NAN;
        output.position_x = f64::INFINITY;
        output.position_y = f64::NEG_INFINITY;
        output.recompute_geometry();

        assert!(output.geometry.viewport_source.is_none());
    }

    #[test]
    fn quarter_turn_uses_post_transform_frame_extent() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.logical_width = 1920;
        output.logical_height = 1080;

        let normal = output.geometry_for_frame(1920, 1080);
        assert_eq!(normal.render_width, 1920);
        assert_eq!(normal.render_height, 1080);

        output.rotation_degrees = 90;
        let rotated = output.geometry_for_frame(1920, 1080);
        assert_eq!(rotated.render_width, 1080);
        assert_eq!(rotated.render_height, 1920);

        output.rotation_degrees = 180;
        let half_turn = output.geometry_for_frame(1920, 1080);
        assert_eq!(half_turn.render_width, 1920);
        assert_eq!(half_turn.render_height, 1080);

        output.rotation_degrees = 270;
        let rotated = output.geometry_for_frame(1920, 1080);
        assert_eq!(rotated.render_width, 1080);
        assert_eq!(rotated.render_height, 1920);
    }

    #[test]
    fn stretch_geometry_keeps_full_destination_for_16_by_9_to_16_by_10() {
        let mut output = OutputState::new(ScaleMode::Stretch);
        output.output_mode_width = 2560;
        output.output_mode_height = 1440;
        output.logical_width = 1920;
        output.logical_height = 1200;
        output.recompute_geometry();

        assert_eq!(output.geometry.viewport_width, 1920);
        assert_eq!(output.geometry.viewport_height, 1200);
        assert!(output.geometry.viewport_source.is_none());
    }
}
