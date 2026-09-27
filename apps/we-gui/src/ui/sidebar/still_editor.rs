use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button, column, container, image, pick_list, row, scrollable, slider, text, text_input,
    },
    Background, Border, Color, ContentFit, Element, Fill, Theme,
};
use we_core::{config::ScaleMode, wallpaper::settings::Rotation};

use crate::domain::{
    i18n::{Language, Text},
    still_editor::StillEditorState,
};

#[derive(Debug, Clone)]
pub(crate) enum StillEditorMessage {
    Close,
    ChooseImage,
    Import,
    TitleChanged(String),
    TargetWidthChanged(String),
    TargetHeightChanged(String),
    ScaleModeChanged(ScaleMode),
    ZoomChanged(f32),
    PositionXChanged(f32),
    PositionYChanged(f32),
    CenterPosition,
    RotationChanged(Rotation),
    ResetTransform,
}

pub(crate) fn view<'a>(
    state: &'a StillEditorState,
    preview: Option<&'a image::Handle>,
    busy: bool,
    error: Option<&'a str>,
    language: Language,
) -> Element<'a, StillEditorMessage> {
    let can_import = preview.is_some() && !busy && error.is_none();
    let preview = preview_panel(state, preview, language);

    let target = section(
        language.text(Text::TargetResolution),
        column![
            row![
                text_input(
                    language.text(Text::Width),
                    &state.target_width
                )
                .on_input(
                    StillEditorMessage::TargetWidthChanged
                )
                .padding([12, 10])
                .width(Fill),
                text_input(
                    language.text(Text::Height),
                    &state.target_height
                )
                .on_input(
                    StillEditorMessage::TargetHeightChanged
                )
                .padding([12, 10])
                .width(Fill),
            ]
            .spacing(8),
            text(language.text(Text::Scaling)).size(13),
            row![
                mode_button(
                    language.text(Text::FillCover),
                    ScaleMode::Cover,
                    state.scale_mode,
                ),
                mode_button(
                    language.text(Text::FillFit),
                    ScaleMode::Fit,
                    state.scale_mode,
                ),
                mode_button(
                    language.text(Text::FillStretch),
                    ScaleMode::Stretch,
                    state.scale_mode,
                ),
            ]
            .spacing(8),
        ]
        .spacing(10),
    );

    let transform = section(
        language.text(Text::Transform),
        column![
            text(format!("{}  {:.0}%", language.text(Text::Zoom), state.zoom * 100.0)).size(13),
            slider(0.1..=4.0, state.zoom, StillEditorMessage::ZoomChanged).step(0.01_f32),
            text(format!(
                "{}  {:+.0}%",
                language.text(Text::HorizontalPosition),
                state.position_x * 100.0
            ))
            .size(13),
            slider(-1.0..=1.0, state.position_x, StillEditorMessage::PositionXChanged)
                .step(0.01_f32),
            text(format!(
                "{}  {:+.0}%",
                language.text(Text::VerticalPosition),
                state.position_y * 100.0
            ))
            .size(13),
            slider(-1.0..=1.0, state.position_y, StillEditorMessage::PositionYChanged)
                .step(0.01_f32),
            text(language.text(Text::Rotation)).size(13),
            pick_list(
                vec![Rotation::Deg0, Rotation::Deg90, Rotation::Deg180, Rotation::Deg270,],
                Some(state.rotation),
                StillEditorMessage::RotationChanged,
            )
            .padding([12, 10])
            .width(Fill),
            row![
                button(text("⌾  Center").size(13))
                    .on_press(StillEditorMessage::CenterPosition)
                    .padding([10, 14]),
                button(text(format!("↺  {}", language.text(Text::ResetTransform))).size(13))
                    .on_press(StillEditorMessage::ResetTransform)
                    .padding([10, 14]),
            ]
            .spacing(8),
        ]
        .spacing(10),
    );

    let source_status = match state.source_dimensions {
        Some((width, height)) => {
            format!("{width} × {height}")
        }
        None => language.text(Text::NoCustomImageSelected).to_string(),
    };

    let choose_label =
        if busy { language.text(Text::LoadingImage) } else { language.text(Text::ChooseImage) };

    let choose_button = button(text(choose_label)).padding([10, 14]);

    let choose_button =
        if busy { choose_button } else { choose_button.on_press(StillEditorMessage::ChooseImage) };

    let import_button = button(text("Import wallpaper")).padding([10, 14]);
    let import_button =
        if can_import { import_button.on_press(StillEditorMessage::Import) } else { import_button };

    let source = section(
        language.text(Text::CustomImageEditor),
        column![
            text_input("Wallpaper title", &state.title)
                .on_input(StillEditorMessage::TitleChanged)
                .padding([12, 10])
                .width(Fill),
            text(source_status).size(14),
            text(language.text(Text::SecureImageIngressPending))
                .size(12)
                .color(Color::from_rgb8(170, 174, 184)),
            choose_button,
            import_button,
            text(error.unwrap_or("")).size(12).color(Color::from_rgb8(255, 180, 171)),
        ]
        .spacing(10),
    );

    let close_button = button(text("×").size(20)).padding([6, 12]);
    let close_button =
        if busy { close_button } else { close_button.on_press(StillEditorMessage::Close) };

    container(
        column![
            row![text(language.text(Text::CustomImageEditor)).size(24).width(Fill), close_button,]
                .align_y(iced::Alignment::Center),
            scrollable(column![preview, source, target, transform,].spacing(16)).height(Fill),
        ]
        .spacing(16),
    )
    .padding(20)
    .width(Fill)
    .height(Fill)
    .style(sidebar_style)
    .into()
}

fn preview_panel<'a>(
    state: &'a StillEditorState,
    preview: Option<&'a image::Handle>,
    language: Language,
) -> Element<'a, StillEditorMessage> {
    let (target_width, target_height) = state.target_extent();

    let summary = match state.preview_geometry() {
        Some(geometry) => {
            let source = geometry
                .viewport_source
                .map(|source| {
                    format!(
                        "  crop {:.0},{:.0} {:.0}×{:.0}",
                        source.x, source.y, source.width, source.height
                    )
                })
                .unwrap_or_default();

            format!(
                "{}×{} → {}×{}{}",
                geometry.render_width,
                geometry.render_height,
                geometry.viewport_width,
                geometry.viewport_height,
                source,
            )
        }

        None => format!(
            "{} — {}×{}",
            language.text(Text::NoCustomImageSelected),
            target_width,
            target_height,
        ),
    };

    let visual: Element<'a, StillEditorMessage> = match preview {
        Some(handle) => container(
            image(handle.clone()).content_fit(ContentFit::Contain).width(Fill).height(Fill),
        )
        .height(170)
        .width(Fill)
        .into(),

        None => container(text(language.text(Text::NoCustomImageSelected)).size(14))
            .height(170)
            .width(Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into(),
    };

    container(
        column![
            text("Preview").size(16),
            visual,
            text(summary).size(12).color(Color::from_rgb8(190, 194, 202)),
        ]
        .spacing(8)
        .align_x(Horizontal::Center),
    )
    .width(Fill)
    .padding(12)
    .style(preview_style)
    .into()
}

fn mode_button<'a>(
    label: &'a str,
    value: ScaleMode,
    selected: ScaleMode,
) -> Element<'a, StillEditorMessage> {
    let active = value == selected;

    button(text(if active { format!("● {label}") } else { label.to_string() }).size(13))
        .on_press(StillEditorMessage::ScaleModeChanged(value))
        .padding([9, 12])
        .into()
}

fn section<'a>(
    title: &'a str,
    content: impl Into<Element<'a, StillEditorMessage>>,
) -> Element<'a, StillEditorMessage> {
    container(column![text(title).size(17), content.into(),].spacing(10))
        .padding(14)
        .style(section_style)
        .into()
}

fn sidebar_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(30, 31, 34))),
        ..Default::default()
    }
}

fn section_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(39, 40, 44))),
        border: Border { radius: 14.0.into(), ..Default::default() },
        ..Default::default()
    }
}

fn preview_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(8, 8, 10))),
        border: Border { radius: 12.0.into(), width: 1.0, color: Color::from_rgb8(70, 72, 78) },
        ..Default::default()
    }
}
