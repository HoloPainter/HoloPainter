use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::{
        brush_engine::{
            BrushEngineDefinition, BrushEngineOrigin, ParamEditor, ParamSpec, ParamType,
            ParamValue, ResourceRefKind, ResourceRefValue,
        },
        stroke_preset::{
            CommonBrushParam, DEFAULT_DIAMETER_SCENE_RATIO_RANGE, INPUT_FILTER_CAP,
            PressureDynamics, QuickParamTarget, StrokePresetOp, StrokeToolPreset,
        },
        texture::{TextureCatalog, texture_tags_match},
    },
    localization::Localization,
    ui::{
        icons::UiIconRegistry,
        tool_options::{ToolOptionsUiState, brush_settings},
        view_output::{UiRequest, ViewOutput},
        widgets::curve_editor::draw_curve_editor,
        widgets::resource_thumbnail_picker::{
            ResourceThumbnailCache, draw_texture_resource_thumbnail_picker,
        },
    },
};

use super::widgets::{grid_labeled_control, grid_labeled_slider, labeled_slider};

const BRUSH_SETTINGS_BUTTON_SIZE: f32 = 22.0;
const BRUSH_SETTINGS_ICON_SIZE: f32 = 16.0;
const BRUSH_SETTINGS_WINDOW_DEFAULT_SIZE: egui::Vec2 = egui::vec2(360.0, 560.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrushParamDisplay {
    Quick,
    Settings,
    SettingsPrefixed,
    SettingsCurve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PressureCurveTarget<'a> {
    Common(&'a str),
    Engine(&'a str),
}

impl BrushParamDisplay {
    fn is_settings(self) -> bool {
        matches!(self, Self::Settings | Self::SettingsPrefixed)
    }
}

pub fn draw_stroke_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    preset_index: usize,
    icons: &UiIconRegistry,
    ui_state: &mut ToolOptionsUiState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let Some(tool_preset) = state.stroke_tool_preset(preset_index) else {
        ui.label(l10n.text("brush-no-tool-preset"));
        return output;
    };
    let mut preset = tool_preset.clone();
    let engine_id = match &preset.stroke_op {
        StrokePresetOp::BrushEngine { engine_id, .. } => engine_id.clone(),
    };
    let engine = state.brush_engines().get(&engine_id);
    let localize_engine = matches!(
        state.brush_engines().origin(&engine_id),
        Some(BrushEngineOrigin::Builtin)
    );

    let changed = draw_quick_params(
        ui,
        l10n,
        &engine_id,
        localize_engine,
        &mut preset,
        engine,
        state.brush_textures(),
        &mut ui_state.resource_thumbnails,
        &mut output,
    );
    ui.add_space(4.0);
    if brush_settings_button(ui, icons)
        .on_hover_text(l10n.text("brush-settings-title"))
        .clicked()
    {
        ui_state.brush_settings_open = true;
    }

    if changed {
        output.push(Command::UpdateRuntimeStrokePreset {
            preset_index,
            preset,
        });
    }
    output
}

pub fn draw_brush_settings_window(
    ctx: &egui::Context,
    l10n: &Localization,
    state: &AppState,
    icons: &UiIconRegistry,
    ui_state: &mut ToolOptionsUiState,
) -> ViewOutput {
    let mut output = ViewOutput::default();

    if let Some(tool) = state.panel_tool_definition() {
        ui_state.sync_tool(tool.id);
    }
    let Some(tool) = state.panel_tool_definition() else {
        return output;
    };
    let crate::core::tool::ToolBehavior::Stroke { preset_index } = tool.behavior else {
        ui_state.brush_settings_open = false;
        return output;
    };
    if !ui_state.brush_settings_open {
        return output;
    }
    let Some(current) = state.stroke_tool_preset(preset_index) else {
        return output;
    };
    let mut preset = current.clone();
    if ui_state
        .name_edit
        .as_ref()
        .is_none_or(|(id, _)| id != &tool.config_id)
    {
        ui_state.name_edit = Some((tool.config_id.clone(), preset.name.clone()));
    }
    let initial_engine_id = brush_engine_id(&preset).to_owned();
    let mut selected_engine_id = initial_engine_id.clone();
    let mut engines = state.brush_engines().engines().collect::<Vec<_>>();
    engines.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let engine = state.brush_engines().get(&initial_engine_id);
    let localize_engine = matches!(
        state.brush_engines().origin(&initial_engine_id),
        Some(BrushEngineOrigin::Builtin)
    );
    let defaults = state
        .brush_engine_defaults(preset_index, &tool.config_id)
        .ok();
    let mut open = ui_state.brush_settings_open;
    let mut changed = false;
    let mut reset_clicked = false;
    egui::Window::new(l10n.text("brush-settings-title"))
        .id(egui::Id::new("brush_settings_window"))
        .open(&mut open)
        .default_size(BRUSH_SETTINGS_WINDOW_DEFAULT_SIZE)
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("brush_settings_window_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(l10n.text("brush-settings-preset"));
                    egui::Grid::new("brush_settings_preset_grid")
                        .num_columns(2)
                        .min_col_width(132.0)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(l10n.text("brush-settings-name"));
                            let name = &mut ui_state.name_edit.as_mut().expect("name draft").1;
                            if ui.text_edit_singleline(name).changed() && !name.trim().is_empty() {
                                preset.name = name.clone();
                                changed = true;
                            }
                            if name.trim().is_empty() {
                                ui.colored_label(
                                    ui.visuals().error_fg_color,
                                    l10n.text("brush-settings-name-required"),
                                );
                            }
                            ui.end_row();

                            ui.label(l10n.text("brush-settings-engine"));
                            let selected_text =
                                state.brush_engines().get(&selected_engine_id).map_or_else(
                                    || selected_engine_id.clone(),
                                    |engine| {
                                        crate::ui::localized::brush_engine_name(l10n, state, engine)
                                    },
                                );
                            egui::ComboBox::from_id_salt("brush_settings_engine")
                                .selected_text(selected_text)
                                .show_ui(ui, |ui| {
                                    for engine in &engines {
                                        ui.selectable_value(
                                            &mut selected_engine_id,
                                            engine.id.clone(),
                                            crate::ui::localized::brush_engine_name(
                                                l10n, state, engine,
                                            ),
                                        );
                                    }
                                });
                            ui.end_row();
                        });

                    changed |= brush_settings::draw_brush_settings(
                        ui,
                        l10n,
                        &mut preset,
                        engine,
                        localize_engine,
                        state.brush_textures(),
                        icons,
                        &mut ui_state.resource_thumbnails,
                        &mut output,
                    );

                    ui.separator();
                    if ui
                        .add_enabled(
                            defaults.as_ref().is_some_and(|value| value != &preset),
                            egui::Button::new(l10n.text("brush-settings-engine-defaults")),
                        )
                        .on_hover_text(l10n.text("brush-settings-reset-help"))
                        .clicked()
                    {
                        reset_clicked = true;
                    }
                });
        });

    if changed {
        output.push(Command::UpdateRuntimeStrokePreset {
            preset_index,
            preset: preset.clone(),
        });
    }
    if selected_engine_id != initial_engine_id {
        output.push(Command::SetRuntimeBrushEngine {
            preset_index,
            preset_id: tool.config_id.clone(),
            engine_id: selected_engine_id,
        });
    }
    if reset_clicked {
        output.push(Command::ResetBrushPreset {
            preset_index,
            preset_id: tool.config_id.clone(),
        });
    }

    ui_state.brush_settings_open = open;
    output
}

fn brush_engine_id(preset: &StrokeToolPreset) -> &str {
    match &preset.stroke_op {
        StrokePresetOp::BrushEngine { engine_id, .. } => engine_id,
    }
}

fn brush_settings_button(ui: &mut egui::Ui, icons: &UiIconRegistry) -> egui::Response {
    let button_size = egui::Vec2::splat(BRUSH_SETTINGS_BUTTON_SIZE);
    let icon_size = egui::Vec2::splat(BRUSH_SETTINGS_ICON_SIZE);
    let (rect, response) = ui.allocate_exact_size(button_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, false);
        let rect = rect.expand(visuals.expansion);
        let show_frame = response.hovered()
            || response.highlighted()
            || response.has_focus()
            || response.is_pointer_button_down_on();
        if show_frame {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
            ui.painter().rect_stroke(
                rect,
                visuals.corner_radius,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }

        let texture = icons
            .texture("builtin.icon.open_in_new")
            .expect("builtin open_in_new icon should be registered");
        let tint = if ui.is_enabled() {
            visuals.fg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.fg_stroke.color
        };
        let icon_rect = egui::Rect::from_center_size(rect.center(), icon_size);
        ui.painter().image(
            texture.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            tint,
        );
    }

    response
}

fn draw_quick_params(
    ui: &mut egui::Ui,
    l10n: &Localization,
    engine_id: &str,
    localize_engine: bool,
    preset: &mut StrokeToolPreset,
    engine: Option<&BrushEngineDefinition>,
    textures: &TextureCatalog,
    resource_thumbnails: &mut ResourceThumbnailCache,
    output: &mut ViewOutput,
) -> bool {
    let targets = preset.quick_params.clone();
    let mut changed = false;
    let mut condition_params = match &preset.stroke_op {
        StrokePresetOp::BrushEngine { params, .. } => params.clone(),
    };

    for target in targets {
        match target {
            QuickParamTarget::Common(param) => {
                changed |=
                    draw_common_brush_param(ui, l10n, preset, param, BrushParamDisplay::Quick);
            }
            QuickParamTarget::Engine(name) => {
                let Some(engine) = engine else { continue };
                if !engine.param_is_active(&name, &condition_params) {
                    continue;
                }
                let Some(spec) = engine.params.iter().find(|spec| spec.name == name) else {
                    continue;
                };
                let StrokePresetOp::BrushEngine { params, .. } = &mut preset.stroke_op;
                let Some((_, value)) = params.iter_mut().find(|(param, _)| *param == name) else {
                    continue;
                };
                let param_changed = draw_brush_param(
                    ui,
                    l10n,
                    engine_id,
                    localize_engine,
                    &name,
                    value,
                    Some(spec),
                    BrushParamDisplay::Quick,
                    textures,
                    resource_thumbnails,
                    output,
                );
                if param_changed
                    && let Some((_, condition_value)) = condition_params
                        .iter_mut()
                        .find(|(condition_name, _)| *condition_name == name)
                {
                    *condition_value = value.clone();
                }
                changed |= param_changed;
            }
        }
    }
    changed
}

pub(super) fn draw_common_brush_param(
    ui: &mut egui::Ui,
    l10n: &Localization,
    preset: &mut StrokeToolPreset,
    param: CommonBrushParam,
    display: BrushParamDisplay,
) -> bool {
    match param {
        CommonBrushParam::Size => {
            let StrokePresetOp::BrushEngine {
                size_scene_ratio, ..
            } = &mut preset.stroke_op;
            let ratio_range = size_scene_ratio
                .range
                .unwrap_or(DEFAULT_DIAMETER_SCENE_RATIO_RANGE);
            size_scene_ratio.value = size_scene_ratio.value.clamp(ratio_range.0, ratio_range.1);
            let mut size_percent = size_scene_ratio.value * 100.0;
            let slider = egui::Slider::new(
                &mut size_percent,
                ratio_range.0 * 100.0..=ratio_range.1 * 100.0,
            )
            .logarithmic(true);
            let changed =
                draw_labeled_slider(ui, l10n.text("brush-param-size"), slider, display).changed();
            if changed {
                size_scene_ratio.value = size_percent * 0.01;
            }
            changed
        }
        CommonBrushParam::SizePressure => {
            let StrokePresetOp::BrushEngine { size_pressure, .. } = &mut preset.stroke_op;
            draw_dynamics_param(
                ui,
                l10n,
                PressureCurveTarget::Common("size_pressure"),
                &l10n.text("brush-param-size-pressure"),
                size_pressure,
                display,
            )
        }
        CommonBrushParam::Spacing => match &mut preset.stroke_strategy {
            crate::core::stroke_preset::StrokeStrategy::SpacingDab { spacing }
            | crate::core::stroke_preset::StrokeStrategy::ContinuousDab { spacing, .. } => {
                draw_labeled_slider(
                    ui,
                    l10n.text("brush-param-spacing"),
                    egui::Slider::new(spacing, 0.01..=1.0),
                    display,
                )
                .changed()
            }
            crate::core::stroke_preset::StrokeStrategy::RawEvent => false,
        },
        CommonBrushParam::SprayRate => match &mut preset.stroke_strategy {
            crate::core::stroke_preset::StrokeStrategy::ContinuousDab { rate_hz, .. } => {
                draw_labeled_slider(
                    ui,
                    l10n.text("brush-param-spray-rate"),
                    egui::Slider::new(rate_hz, 10.0..=120.0)
                        .suffix(l10n.text("brush-dabs-per-second-suffix")),
                    display,
                )
                .changed()
            }
            _ => false,
        },
        CommonBrushParam::Stabilization => {
            preset.input_filter.stabilization =
                preset.input_filter.stabilization.min(INPUT_FILTER_CAP);
            draw_labeled_slider(
                ui,
                l10n.text("brush-param-stabilization"),
                egui::Slider::new(&mut preset.input_filter.stabilization, 0..=INPUT_FILTER_CAP),
                display,
            )
            .changed()
        }
        CommonBrushParam::PressureFilter => {
            preset.input_filter.pressure_filter =
                preset.input_filter.pressure_filter.min(INPUT_FILTER_CAP);
            draw_labeled_slider(
                ui,
                l10n.text("brush-param-pressure-filter"),
                egui::Slider::new(
                    &mut preset.input_filter.pressure_filter,
                    0..=INPUT_FILTER_CAP,
                ),
                display,
            )
            .changed()
        }
        CommonBrushParam::PressureFeel => {
            preset.pressure_response.feel = preset.pressure_response.feel.clamp(0.0, 200.0);
            draw_labeled_slider(
                ui,
                l10n.text("brush-param-pressure-feel"),
                egui::Slider::new(&mut preset.pressure_response.feel, 0.0..=200.0),
                display,
            )
            .changed()
        }
    }
}

pub(super) fn draw_brush_param(
    ui: &mut egui::Ui,
    l10n: &Localization,
    engine_id: &str,
    localize_engine: bool,
    name: &str,
    value: &mut ParamValue,
    spec: Option<&ParamSpec>,
    display: BrushParamDisplay,
    textures: &TextureCatalog,
    resource_thumbnails: &mut ResourceThumbnailCache,
    output: &mut ViewOutput,
) -> bool {
    let label = spec.map_or_else(
        || name.to_owned(),
        |spec| {
            crate::ui::localized::brush_engine_part_name(
                l10n,
                engine_id,
                localize_engine,
                "param",
                name,
                &spec.label,
            )
        },
    );
    match value {
        ParamValue::F32(value) => {
            let (min, max) = spec.and_then(|spec| spec.range).unwrap_or((0.0, 2.0));
            *value = value.clamp(min, max);
            draw_labeled_slider(ui, label, egui::Slider::new(value, min..=max), display).changed()
        }
        ParamValue::Dynamics(value) => draw_dynamics_param(
            ui,
            l10n,
            PressureCurveTarget::Engine(name),
            &label,
            value,
            display,
        ),
        ParamValue::Bool(value) if display.is_settings() => {
            settings_row_prefix(ui, display);
            grid_labeled_control(ui, label, |ui| ui.checkbox(value, "")).changed()
        }
        ParamValue::Bool(value) => ui.checkbox(value, label).changed(),
        ParamValue::U32(value) if display.is_settings() => {
            settings_row_prefix(ui, display);
            grid_labeled_control(ui, label, |ui| ui.add(egui::DragValue::new(value))).changed()
        }
        ParamValue::U32(value) => ui
            .add(egui::DragValue::new(value).prefix(format!("{label}: ")))
            .changed(),
        ParamValue::Enum(value) => draw_brush_enum_param(
            ui,
            l10n,
            engine_id,
            localize_engine,
            name,
            &label,
            value,
            spec,
            display,
        ),
        ParamValue::ResourceRef(value) => draw_resource_ref_param(
            ui,
            l10n,
            name,
            &label,
            value,
            spec,
            display,
            textures,
            resource_thumbnails,
            output,
        ),
    }
}

fn draw_dynamics_param(
    ui: &mut egui::Ui,
    l10n: &Localization,
    target: PressureCurveTarget<'_>,
    label: &str,
    pressure: &mut PressureDynamics,
    display: BrushParamDisplay,
) -> bool {
    let mut changed = false;
    if display.is_settings() {
        settings_row_prefix(ui, display);
        changed |=
            grid_labeled_control(ui, label, |ui| ui.checkbox(&mut pressure.enabled, "")).changed();
    } else {
        changed |= ui.checkbox(&mut pressure.enabled, label).changed();
    }
    if pressure.enabled && !display.is_settings() {
        changed |= draw_pressure_dynamics(ui, l10n, target, pressure, display);
    }
    changed
}

pub(super) fn draw_pressure_dynamics(
    ui: &mut egui::Ui,
    l10n: &Localization,
    target: PressureCurveTarget<'_>,
    pressure: &mut PressureDynamics,
    display: BrushParamDisplay,
) -> bool {
    let mut changed = false;
    let id = egui::Id::new(("pressure_curve", display_id(display), target));
    let mut draw = |ui: &mut egui::Ui| {
        egui::CollapsingHeader::new(l10n.text("brush-pressure-curve"))
            .id_salt(id)
            .default_open(false)
            .show(ui, |ui| {
                let selected_id = id.with("selected_point");
                let mut selected = ui
                    .ctx()
                    .data_mut(|data| data.get_temp::<Option<usize>>(selected_id))
                    .flatten();
                let output = draw_curve_editor(
                    ui,
                    &mut pressure.curve,
                    &mut selected,
                    ui.visuals().selection.stroke.color,
                    None,
                    None,
                );
                ui.ctx()
                    .data_mut(|data| data.insert_temp(selected_id, selected));
                changed |= output.changed;
                if ui
                    .button(l10n.text("brush-pressure-reset-linear"))
                    .clicked()
                    && pressure.curve != Default::default()
                {
                    pressure.curve = Default::default();
                    changed = true;
                }
            });
    };
    draw(ui);
    changed
}

fn draw_resource_ref_param(
    ui: &mut egui::Ui,
    l10n: &Localization,
    name: &str,
    label: &str,
    value: &mut ResourceRefValue,
    spec: Option<&ParamSpec>,
    display: BrushParamDisplay,
    catalog: &TextureCatalog,
    resource_thumbnails: &mut ResourceThumbnailCache,
    output: &mut ViewOutput,
) -> bool {
    let Some(ParamSpec {
        ty:
            ParamType::ResourceRef {
                kind: ResourceRefKind::Texture,
                tags,
            },
        editor,
        ..
    }) = spec
    else {
        ui.label(label);
        return false;
    };
    let mut options = catalog
        .textures()
        .filter(|texture| texture_tags_match(texture, tags))
        .collect::<Vec<_>>();
    options.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut draw_control = |ui: &mut egui::Ui| match editor {
        ParamEditor::Default => draw_resource_ref_combo(ui, l10n, label, value, &options, catalog),
        ParamEditor::ResourceThumbnailGrid => {
            let picker = draw_texture_resource_thumbnail_picker(
                ui,
                l10n,
                egui::Id::new(("texture_ref_thumbnail", name, display_id(display))),
                value,
                &options,
                catalog,
                resource_thumbnails,
            );
            if picker.import_requested {
                output.request_ui(UiRequest::ImportTextureResource {
                    required_tags: tags.clone(),
                });
            }
            if picker.library_requested {
                output.request_ui(UiRequest::OpenImageAssetLibrary);
            }
            picker.changed
        }
    };

    if display.is_settings() {
        settings_row_prefix(ui, display);
        grid_labeled_control(ui, label, draw_control)
    } else {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(label);
            changed = draw_control(ui);
        });
        changed
    }
}

fn draw_resource_ref_combo(
    ui: &mut egui::Ui,
    l10n: &Localization,
    label: &str,
    value: &mut ResourceRefValue,
    options: &[&crate::core::texture::TextureResourceDefinition],
    catalog: &TextureCatalog,
) -> bool {
    let before = value.clone();
    let selected_text = match value {
        ResourceRefValue::None => l10n.text("value-none"),
        ResourceRefValue::Resource(id) => catalog.get(id).map_or_else(
            || {
                let mut args = fluent::FluentArgs::new();
                args.set("id", id.as_str());
                l10n.format("value-missing-id", Some(&args))
            },
            |texture| crate::ui::localized::texture_resource_name(l10n, texture),
        ),
    };
    egui::ComboBox::from_id_salt(("texture_ref", label))
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            for texture in options {
                ui.selectable_value(
                    value,
                    ResourceRefValue::Resource(texture.id.clone()),
                    crate::ui::localized::texture_resource_name(l10n, texture),
                );
            }
        });
    *value != before
}

fn display_id(display: BrushParamDisplay) -> &'static str {
    match display {
        BrushParamDisplay::Quick => "quick",
        BrushParamDisplay::Settings => "settings",
        BrushParamDisplay::SettingsPrefixed => "settings_prefixed",
        BrushParamDisplay::SettingsCurve => "settings_curve",
    }
}

fn draw_brush_enum_param(
    ui: &mut egui::Ui,
    l10n: &Localization,
    engine_id: &str,
    localize_engine: bool,
    name: &str,
    label: &str,
    value: &mut u32,
    spec: Option<&ParamSpec>,
    display: BrushParamDisplay,
) -> bool {
    let Some(ParamSpec {
        ty: ParamType::Enum { variants },
        ..
    }) = spec
    else {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.label(value.to_string());
        });
        return false;
    };
    if variants.is_empty() {
        ui.label(format!("{label}: <no variants>"));
        return false;
    }
    let mut changed = false;
    if !variants.iter().any(|variant| variant.value == *value) {
        *value = variants[0].value;
        changed = true;
    }
    let variant_label = |variant: &crate::core::brush_engine::EnumParamVariant| {
        crate::ui::localized::brush_engine_part_name(
            l10n,
            engine_id,
            localize_engine,
            &format!("param-{name}-variant"),
            &variant.name,
            &variant.label,
        )
    };
    let selected_label = variants
        .iter()
        .find(|variant| variant.value == *value)
        .map_or_else(|| value.to_string(), variant_label);
    let draw_combo = |ui: &mut egui::Ui| {
        egui::ComboBox::from_id_salt(("brush_enum", name))
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                for variant in variants {
                    changed |= ui
                        .selectable_value(value, variant.value, variant_label(variant))
                        .changed();
                }
            });
    };
    if display.is_settings() {
        settings_row_prefix(ui, display);
        grid_labeled_control(ui, label, draw_combo);
    } else {
        ui.horizontal(|ui| {
            ui.label(label);
            draw_combo(ui);
        });
    }
    changed
}

fn draw_labeled_slider(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    slider: egui::Slider<'_>,
    display: BrushParamDisplay,
) -> egui::Response {
    match display {
        BrushParamDisplay::Quick => labeled_slider(ui, label, slider),
        BrushParamDisplay::Settings => {
            settings_row_prefix(ui, display);
            grid_labeled_slider(ui, label, slider)
        }
        BrushParamDisplay::SettingsPrefixed => grid_labeled_slider(ui, label, slider),
        BrushParamDisplay::SettingsCurve => labeled_slider(ui, label, slider),
    }
}

fn settings_row_prefix(ui: &mut egui::Ui, display: BrushParamDisplay) {
    if display == BrushParamDisplay::Settings {
        ui.label("");
    }
}
