use eframe::egui;

use crate::core::{
    brush_engine::{BrushEngineDefinition, ParamExposure, ParamValue},
    stroke_preset::{
        CommonBrushParam, QuickParamTarget, StrokePresetOp, StrokeStrategy, StrokeToolPreset,
    },
    texture::TextureCatalog,
};
use crate::localization::Localization;
use crate::ui::{
    icons::UiIconRegistry,
    view_output::ViewOutput,
    widgets::{resource_thumbnail_picker::ResourceThumbnailCache, visibility_toggle},
};

use super::stroke_options::{self, BrushParamDisplay, PressureCurveTarget};

fn settings_grid<R>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    add_rows: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Grid::new(id)
        .num_columns(3)
        .min_col_width(0.0)
        .spacing([8.0, 4.0])
        .show(ui, add_rows)
        .inner
}

pub(crate) fn draw_brush_settings(
    ui: &mut egui::Ui,
    l10n: &Localization,
    preset: &mut StrokeToolPreset,
    engine: Option<&BrushEngineDefinition>,
    localize_engine: bool,
    textures: &TextureCatalog,
    icons: &UiIconRegistry,
    resource_thumbnails: &mut ResourceThumbnailCache,
    output: &mut ViewOutput,
) -> bool {
    let mut changed = false;
    let quick_param_order = brush_settings_quick_param_order(engine);

    ui.separator();
    ui.label(l10n.text("brush-settings-common"));
    settings_grid(ui, "brush_settings_common_grid", |ui| {
        for param in [CommonBrushParam::Size, CommonBrushParam::SizePressure] {
            changed |= draw_quick_param_toggle(
                ui,
                l10n,
                preset,
                QuickParamTarget::Common(param),
                icons,
                &quick_param_order,
            );
            changed |= stroke_options::draw_common_brush_param(
                ui,
                l10n,
                preset,
                param,
                BrushParamDisplay::SettingsPrefixed,
            );
        }
    });
    let StrokePresetOp::BrushEngine { size_pressure, .. } = &mut preset.stroke_op;
    if size_pressure.enabled {
        ui.indent("brush_settings_size_pressure_curve", |ui| {
            changed |= stroke_options::draw_pressure_dynamics(
                ui,
                l10n,
                PressureCurveTarget::Common("size_pressure"),
                size_pressure,
                BrushParamDisplay::SettingsCurve,
            );
        });
    }

    ui.separator();
    ui.label(l10n.text("brush-settings-stroke"));
    settings_grid(ui, "brush_settings_stroke_grid", |ui| {
        if matches!(preset.stroke_strategy, StrokeStrategy::RawEvent) {
            ui.label("");
            ui.label(l10n.text("brush-settings-strategy"));
            ui.label(l10n.text("brush-settings-strategy-raw-event"));
            ui.end_row();
        } else {
            for param in [CommonBrushParam::Spacing, CommonBrushParam::SprayRate] {
                if param.is_available_for(&preset.stroke_strategy) {
                    changed |= draw_quick_param_toggle(
                        ui,
                        l10n,
                        preset,
                        QuickParamTarget::Common(param),
                        icons,
                        &quick_param_order,
                    );
                    changed |= stroke_options::draw_common_brush_param(
                        ui,
                        l10n,
                        preset,
                        param,
                        BrushParamDisplay::SettingsPrefixed,
                    );
                }
            }
        }
    });

    ui.separator();
    ui.label(l10n.text("brush-settings-input"));
    settings_grid(ui, "brush_settings_input_grid", |ui| {
        for param in [
            CommonBrushParam::Stabilization,
            CommonBrushParam::PressureFilter,
            CommonBrushParam::PressureFeel,
        ] {
            changed |= draw_quick_param_toggle(
                ui,
                l10n,
                preset,
                QuickParamTarget::Common(param),
                icons,
                &quick_param_order,
            );
            changed |= stroke_options::draw_common_brush_param(
                ui,
                l10n,
                preset,
                param,
                BrushParamDisplay::SettingsPrefixed,
            );
        }
    });

    let Some(engine) = engine else {
        return changed;
    };
    ui.separator();
    ui.label(crate::ui::localized::brush_engine_definition_name(
        l10n,
        engine,
        localize_engine,
    ));
    let mut condition_params = match &preset.stroke_op {
        StrokePresetOp::BrushEngine { params, .. } => params.clone(),
    };
    let mut current_section: Option<&str> = None;
    for spec in engine
        .params
        .iter()
        .filter(|spec| matches!(spec.exposure, ParamExposure::Public))
    {
        if !engine.param_is_active(&spec.name, &condition_params) {
            continue;
        }
        if let Some(section) = spec.section.as_deref()
            && current_section != Some(section)
        {
            ui.strong(crate::ui::localized::brush_engine_part_name(
                l10n,
                &engine.id,
                localize_engine,
                "section",
                section,
                section,
            ));
        }
        current_section = spec.section.as_deref();
        let has_value = match &preset.stroke_op {
            StrokePresetOp::BrushEngine { params, .. } => {
                params.iter().any(|(name, _)| *name == spec.name)
            }
        };
        if !has_value {
            continue;
        }
        let mut param_changed = false;
        settings_grid(ui, ("brush_settings_engine_param", &spec.name), |ui| {
            changed |= draw_quick_param_toggle(
                ui,
                l10n,
                preset,
                QuickParamTarget::Engine(spec.name.clone()),
                icons,
                &quick_param_order,
            );
            let StrokePresetOp::BrushEngine { params, .. } = &mut preset.stroke_op;
            let (_, value) = params
                .iter_mut()
                .find(|(name, _)| *name == spec.name)
                .expect("checked engine parameter should exist");
            param_changed = stroke_options::draw_brush_param(
                ui,
                l10n,
                &engine.id,
                localize_engine,
                &spec.name,
                value,
                Some(spec),
                BrushParamDisplay::SettingsPrefixed,
                textures,
                resource_thumbnails,
                output,
            );
        });
        if param_changed {
            let StrokePresetOp::BrushEngine { params, .. } = &preset.stroke_op;
            let value = params
                .iter()
                .find(|(name, _)| *name == spec.name)
                .map(|(_, value)| value)
                .expect("checked engine parameter should exist");
            if let Some((_, condition_value)) = condition_params
                .iter_mut()
                .find(|(name, _)| *name == spec.name)
            {
                *condition_value = value.clone();
            }
            changed = true;
        }
        let StrokePresetOp::BrushEngine { params, .. } = &mut preset.stroke_op;
        if let Some((_, ParamValue::Dynamics(pressure))) =
            params.iter_mut().find(|(name, _)| *name == spec.name)
            && pressure.enabled
        {
            ui.indent(("brush_settings_engine_curve", &spec.name), |ui| {
                changed |= stroke_options::draw_pressure_dynamics(
                    ui,
                    l10n,
                    PressureCurveTarget::Engine(&spec.name),
                    pressure,
                    BrushParamDisplay::SettingsCurve,
                );
            });
        }
    }
    changed
}

fn draw_quick_param_toggle(
    ui: &mut egui::Ui,
    l10n: &Localization,
    preset: &mut StrokeToolPreset,
    target: QuickParamTarget,
    icons: &UiIconRegistry,
    quick_param_order: &[QuickParamTarget],
) -> bool {
    let visible = preset.quick_param_visible(&target);
    let tooltip = if visible {
        l10n.text("brush-hide-quick-param")
    } else {
        l10n.text("brush-show-quick-param")
    };
    if visibility_toggle::draw_visibility_toggle(ui, icons, visible)
        .on_hover_text(tooltip)
        .clicked()
    {
        let changed = preset.set_quick_param_visible(target, !visible);
        if changed {
            preset.reorder_quick_params(quick_param_order);
        }
        changed
    } else {
        false
    }
}

fn brush_settings_quick_param_order(
    engine: Option<&BrushEngineDefinition>,
) -> Vec<QuickParamTarget> {
    let mut order = vec![
        QuickParamTarget::Common(CommonBrushParam::Size),
        QuickParamTarget::Common(CommonBrushParam::SizePressure),
        QuickParamTarget::Common(CommonBrushParam::Spacing),
        QuickParamTarget::Common(CommonBrushParam::SprayRate),
        QuickParamTarget::Common(CommonBrushParam::Stabilization),
        QuickParamTarget::Common(CommonBrushParam::PressureFilter),
        QuickParamTarget::Common(CommonBrushParam::PressureFeel),
    ];
    if let Some(engine) = engine {
        order.extend(
            engine
                .params
                .iter()
                .filter(|spec| matches!(spec.exposure, ParamExposure::Public))
                .map(|spec| QuickParamTarget::Engine(spec.name.clone())),
        );
    }
    order
}
