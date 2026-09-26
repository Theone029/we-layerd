use crate::config::ScaleMode;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportSource {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGeometry {
    pub render_width: u32,
    pub render_height: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub viewport_source: Option<ViewportSource>,
}

pub fn compute_presentation_geometry(
    scale_mode: ScaleMode,
    render_width: u32,
    render_height: u32,
    viewport_width: u32,
    viewport_height: u32,
    zoom: f64,
    position_x: f64,
    position_y: f64,
) -> PresentationGeometry {
    let render_width = render_width.max(1);
    let render_height = render_height.max(1);
    let viewport_width = viewport_width.max(1);
    let viewport_height = viewport_height.max(1);

    let mut geometry = match scale_mode {
        ScaleMode::Stretch => PresentationGeometry {
            render_width,
            render_height,
            viewport_width,
            viewport_height,
            viewport_source: None,
        },
        ScaleMode::Cover => {
            let source = cover_source(render_width, render_height, viewport_width, viewport_height);

            PresentationGeometry {
                render_width,
                render_height,
                viewport_width,
                viewport_height,
                viewport_source: source,
            }
        }
        ScaleMode::Fit => {
            let (fit_width, fit_height) =
                fit_destination(render_width, render_height, viewport_width, viewport_height);

            PresentationGeometry {
                render_width,
                render_height,
                viewport_width: fit_width,
                viewport_height: fit_height,
                viewport_source: None,
            }
        }
    };

    apply_transform_source(&mut geometry, zoom, position_x, position_y);

    geometry
}

fn finite_clamp(value: f64, default: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

fn apply_transform_source(
    geometry: &mut PresentationGeometry,
    zoom: f64,
    position_x: f64,
    position_y: f64,
) {
    let zoom = finite_clamp(zoom, 1.0, 0.1, 4.0);
    let position_x = finite_clamp(position_x, 0.0, -1.0, 1.0);
    let position_y = finite_clamp(position_y, 0.0, -1.0, 1.0);

    // Below 100%, shrink the final destination rectangle. Placement of
    // that rectangle is a consumer concern.
    if zoom < 1.0 {
        geometry.viewport_width = (geometry.viewport_width as f64 * zoom).round().max(1.0) as u32;

        geometry.viewport_height = (geometry.viewport_height as f64 * zoom).round().max(1.0) as u32;

        return;
    }

    // Neutral presentation preserves the base geometry.
    if (zoom - 1.0).abs() < f64::EPSILON
        && position_x.abs() < f64::EPSILON
        && position_y.abs() < f64::EPSILON
    {
        return;
    }

    // At 100%+, zoom is represented as a source-window crop. Cover's
    // existing crop is the starting rectangle.
    let render_width = geometry.render_width.max(1) as f64;
    let render_height = geometry.render_height.max(1) as f64;

    let base = geometry.viewport_source.unwrap_or(ViewportSource {
        x: 0.0,
        y: 0.0,
        width: render_width,
        height: render_height,
    });

    let width = (base.width / zoom).clamp(1.0, render_width);
    let height = (base.height / zoom).clamp(1.0, render_height);

    let centered_x =
        (base.x + (base.width - width) / 2.0).clamp(0.0, (render_width - width).max(0.0));

    let centered_y =
        (base.y + (base.height - height) / 2.0).clamp(0.0, (render_height - height).max(0.0));

    let max_x = (render_width - width).max(0.0);
    let max_y = (render_height - height).max(0.0);

    let x = if position_x < 0.0 {
        centered_x + position_x * centered_x
    } else {
        centered_x + position_x * (max_x - centered_x)
    };

    let y = if position_y < 0.0 {
        centered_y + position_y * centered_y
    } else {
        centered_y + position_y * (max_y - centered_y)
    };

    geometry.viewport_source =
        Some(ViewportSource { x: x.clamp(0.0, max_x), y: y.clamp(0.0, max_y), width, height });
}

fn cover_source(
    render_width: u32,
    render_height: u32,
    viewport_width: u32,
    viewport_height: u32,
) -> Option<ViewportSource> {
    let render_aspect = render_width as f64 / render_height as f64;

    let viewport_aspect = viewport_width as f64 / viewport_height as f64;

    if (render_aspect - viewport_aspect).abs() < f64::EPSILON {
        return None;
    }

    if render_aspect > viewport_aspect {
        let cropped_width = render_height as f64 * viewport_aspect;

        let x = ((render_width as f64 - cropped_width) / 2.0).max(0.0);

        return Some(ViewportSource {
            x,
            y: 0.0,
            width: cropped_width,
            height: render_height as f64,
        });
    }

    let cropped_height = render_width as f64 / viewport_aspect;

    let y = ((render_height as f64 - cropped_height) / 2.0).max(0.0);

    Some(ViewportSource { x: 0.0, y, width: render_width as f64, height: cropped_height })
}

fn fit_destination(
    render_width: u32,
    render_height: u32,
    viewport_width: u32,
    viewport_height: u32,
) -> (u32, u32) {
    let width_scale = viewport_width as f64 / render_width as f64;

    let height_scale = viewport_height as f64 / render_height as f64;

    let scale = width_scale.min(height_scale);

    let width = (render_width as f64 * scale).round().max(1.0) as u32;

    let height = (render_height as f64 * scale).round().max(1.0) as u32;

    (width, height)
}

#[cfg(test)]
mod tests {
    use super::{compute_presentation_geometry, PresentationGeometry, ViewportSource};
    use crate::config::ScaleMode;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 0.000_001, "actual={actual} expected={expected}");
    }

    #[test]
    fn fit_preserves_aspect_inside_destination() {
        let geometry =
            compute_presentation_geometry(ScaleMode::Fit, 2560, 1440, 1920, 1200, 1.0, 0.0, 0.0);

        assert_eq!(geometry.render_width, 2560);
        assert_eq!(geometry.render_height, 1440);
        assert_eq!(geometry.viewport_width, 1920);
        assert_eq!(geometry.viewport_height, 1080);
        assert!(geometry.viewport_source.is_none());
    }

    #[test]
    fn cover_centers_base_crop() {
        let geometry =
            compute_presentation_geometry(ScaleMode::Cover, 2560, 1440, 1920, 1200, 1.0, 0.0, 0.0);

        let source = geometry.viewport_source.expect("cover crop");

        assert_close(source.x, 128.0);
        assert_close(source.y, 0.0);
        assert_close(source.width, 2304.0);
        assert_close(source.height, 1440.0);
    }

    #[test]
    fn sub_100_zoom_shrinks_destination() {
        let geometry =
            compute_presentation_geometry(ScaleMode::Fit, 1920, 1080, 1920, 1080, 0.5, 0.0, 0.0);

        assert_eq!(geometry.viewport_width, 960);
        assert_eq!(geometry.viewport_height, 540);
        assert!(geometry.viewport_source.is_none());
    }

    #[test]
    fn over_100_zoom_uses_centered_source_crop() {
        let geometry = compute_presentation_geometry(
            ScaleMode::Stretch,
            1920,
            1080,
            1920,
            1080,
            2.0,
            0.0,
            0.0,
        );

        let source = geometry.viewport_source.expect("zoom crop");

        assert_close(source.x, 480.0);
        assert_close(source.y, 270.0);
        assert_close(source.width, 960.0);
        assert_close(source.height, 540.0);
    }

    #[test]
    fn positive_pan_reaches_lower_right_crop_limit() {
        let geometry = compute_presentation_geometry(
            ScaleMode::Stretch,
            1920,
            1080,
            1920,
            1080,
            2.0,
            1.0,
            1.0,
        );

        let source = geometry.viewport_source.expect("zoom crop");

        assert_close(source.x, 960.0);
        assert_close(source.y, 540.0);
        assert_close(source.width, 960.0);
        assert_close(source.height, 540.0);
    }

    #[test]
    fn negative_pan_reaches_upper_left_crop_limit() {
        let geometry = compute_presentation_geometry(
            ScaleMode::Stretch,
            1920,
            1080,
            1920,
            1080,
            2.0,
            -1.0,
            -1.0,
        );

        let source = geometry.viewport_source.expect("zoom crop");

        assert_close(source.x, 0.0);
        assert_close(source.y, 0.0);
    }

    #[test]
    fn non_finite_transform_values_fall_back_to_neutral() {
        let neutral =
            compute_presentation_geometry(ScaleMode::Cover, 2560, 1440, 1920, 1200, 1.0, 0.0, 0.0);

        let invalid = compute_presentation_geometry(
            ScaleMode::Cover,
            2560,
            1440,
            1920,
            1200,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );

        assert_eq!(invalid, neutral);
    }

    #[test]
    fn public_geometry_types_preserve_expected_shape() {
        let geometry = PresentationGeometry {
            render_width: 100,
            render_height: 50,
            viewport_width: 80,
            viewport_height: 40,
            viewport_source: Some(ViewportSource { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }),
        };

        assert_eq!(geometry.render_width, 100);
        assert_eq!(geometry.viewport_source.expect("source").x, 1.0);
    }
}
