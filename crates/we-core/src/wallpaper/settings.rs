use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::WallpaperType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WallpaperSettings {
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default = "default_msaa_samples")]
    pub msaa_samples: u32,
    #[serde(default)]
    pub render_resolution: RenderResolution,
    #[serde(default)]
    pub fill_mode: WallpaperFillMode,
    #[serde(default)]
    pub rotation_degrees: Rotation,
    #[serde(default = "default_zoom")]
    pub zoom: f32,
    #[serde(default)]
    pub position_x: f32,
    #[serde(default)]
    pub position_y: f32,
    #[serde(default)]
    pub visual_adjustments: VisualAdjustments,
    #[serde(default)]
    pub user_properties: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct VisualAdjustments {
    #[serde(default)]
    pub brightness: f32,
    #[serde(default = "default_visual_contrast")]
    pub contrast: f32,
    #[serde(default = "default_visual_saturation")]
    pub saturation: f32,
    #[serde(default)]
    pub hue_degrees: f32,
}

impl Default for VisualAdjustments {
    fn default() -> Self {
        Self { brightness: 0.0, contrast: 1.0, saturation: 1.0, hue_degrees: 0.0 }
    }
}

impl VisualAdjustments {
    pub fn normalized(self) -> Self {
        Self {
            brightness: bounded_or(self.brightness, -1.0, 1.0, 0.0),
            contrast: bounded_or(self.contrast, 0.0, 2.0, 1.0),
            saturation: bounded_or(self.saturation, 0.0, 2.0, 1.0),
            hue_degrees: bounded_or(self.hue_degrees, -180.0, 180.0, 0.0),
        }
    }

    pub fn is_neutral(self) -> bool {
        let value = self.normalized();
        value.brightness == 0.0
            && value.contrast == 1.0
            && value.saturation == 1.0
            && value.hue_degrees == 0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderResolution {
    #[default]
    Automatic,
    Fixed {
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperFillMode {
    #[default]
    Cover,
    Fit,
    Stretch,
    Center,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    pub fn degrees(self) -> u32 {
        match self {
            Self::Deg0 => 0,
            Self::Deg90 => 90,
            Self::Deg180 => 180,
            Self::Deg270 => 270,
        }
    }
}

impl std::fmt::Display for WallpaperFillMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cover => "Cover",
            Self::Fit => "Fit",
            Self::Stretch => "Stretch",
            Self::Center => "Center",
        })
    }
}

impl std::fmt::Display for Rotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Deg0 => "0°",
            Self::Deg90 => "90°",
            Self::Deg180 => "180°",
            Self::Deg270 => "270°",
        })
    }
}

impl Default for WallpaperSettings {
    fn default() -> Self {
        Self {
            fps: default_fps(),
            speed: default_speed(),
            volume: default_volume(),
            muted: false,
            msaa_samples: default_msaa_samples(),
            render_resolution: RenderResolution::Automatic,
            fill_mode: WallpaperFillMode::Cover,
            rotation_degrees: Rotation::Deg0,
            zoom: default_zoom(),
            position_x: 0.0,
            position_y: 0.0,
            visual_adjustments: VisualAdjustments::default(),
            user_properties: BTreeMap::new(),
        }
    }
}

fn default_fps() -> u32 {
    60
}
fn default_speed() -> f32 {
    1.0
}
fn default_volume() -> f32 {
    1.0
}
fn default_msaa_samples() -> u32 {
    1
}

fn default_zoom() -> f32 {
    1.0
}

fn default_visual_contrast() -> f32 {
    1.0
}

fn default_visual_saturation() -> f32 {
    1.0
}

fn bounded_or(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

pub fn supports_final_output_msaa(wallpaper_type: WallpaperType) -> bool {
    wallpaper_type == WallpaperType::Scene
}

pub fn inherited_final_output_msaa(global_samples: u32, wallpaper_type: WallpaperType) -> u32 {
    if supports_final_output_msaa(wallpaper_type) {
        global_samples.max(1)
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::{
        inherited_final_output_msaa, supports_final_output_msaa, RenderResolution,
        VisualAdjustments, WallpaperFillMode, WallpaperSettings,
    };
    use crate::wallpaper::WallpaperType;

    #[test]
    fn settings_default_to_dynamic_neutral_rendering() {
        let settings = WallpaperSettings::default();
        assert_eq!(settings.render_resolution, RenderResolution::Automatic);
        assert_eq!(settings.fill_mode, WallpaperFillMode::Cover);
        assert_eq!(settings.rotation_degrees.degrees(), 0);
        assert_eq!(settings.zoom, 1.0);
        assert_eq!(settings.position_x, 0.0);
        assert_eq!(settings.position_y, 0.0);
        assert_eq!(settings.visual_adjustments, VisualAdjustments::default());
        assert!(settings.visual_adjustments.is_neutral());
        assert_eq!(settings.fps, 60);
        assert_eq!(settings.msaa_samples, 1);
    }

    #[test]
    fn legacy_settings_without_transform_fields_use_neutral_defaults() {
        let settings: WallpaperSettings =
            serde_json::from_str("{}").expect("legacy settings deserialize");
        assert_eq!(settings.zoom, 1.0);
        assert_eq!(settings.position_x, 0.0);
        assert_eq!(settings.position_y, 0.0);
        assert_eq!(settings.visual_adjustments, VisualAdjustments::default());
    }

    #[test]
    fn visual_adjustments_bound_invalid_values() {
        let normalized = VisualAdjustments {
            brightness: f32::NAN,
            contrast: 8.0,
            saturation: -4.0,
            hue_degrees: 720.0,
        }
        .normalized();

        assert_eq!(normalized.brightness, 0.0);
        assert_eq!(normalized.contrast, 2.0);
        assert_eq!(normalized.saturation, 0.0);
        assert_eq!(normalized.hue_degrees, 180.0);
    }

    #[test]
    fn transform_settings_round_trip() {
        let settings = WallpaperSettings {
            zoom: 1.75,
            position_x: -0.25,
            position_y: 0.5,
            ..WallpaperSettings::default()
        };
        let encoded = serde_json::to_string(&settings).expect("serialize transform settings");
        let decoded: WallpaperSettings =
            serde_json::from_str(&encoded).expect("deserialize transform settings");
        assert_eq!(decoded.zoom, 1.75);
        assert_eq!(decoded.position_x, -0.25);
        assert_eq!(decoded.position_y, 0.5);
    }

    #[test]
    fn visual_adjustments_round_trip() {
        let settings = WallpaperSettings {
            visual_adjustments: VisualAdjustments {
                brightness: 0.2,
                contrast: 1.4,
                saturation: 0.6,
                hue_degrees: -45.0,
            },
            ..WallpaperSettings::default()
        };
        let encoded = serde_json::to_string(&settings).expect("serialize visual adjustments");
        let decoded: WallpaperSettings =
            serde_json::from_str(&encoded).expect("deserialize visual adjustments");

        assert_eq!(decoded.visual_adjustments, settings.visual_adjustments);
    }

    #[test]
    fn final_output_msaa_is_scene_only() {
        assert!(supports_final_output_msaa(WallpaperType::Scene));
        assert!(!supports_final_output_msaa(WallpaperType::Video));
        assert!(!supports_final_output_msaa(WallpaperType::Web));
        assert!(!supports_final_output_msaa(WallpaperType::Unknown));
        assert_eq!(inherited_final_output_msaa(8, WallpaperType::Scene), 8);
        assert_eq!(inherited_final_output_msaa(8, WallpaperType::Video), 1);
        assert_eq!(inherited_final_output_msaa(8, WallpaperType::Web), 1);
    }
}
