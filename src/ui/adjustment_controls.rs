use crate::ui::widgets::interaction_gate::InteractionGate;
use eframe::egui;

use crate::{
    core::adjustment::{
        Adjustment, BrightnessContrastAdjustment, CurveChannel, CurvesAdjustment,
        HueSaturationAdjustment, LevelsAdjustment, LevelsChannel, MAX_CURVE_POINTS,
    },
    core::surface_filter::SurfaceFilterDomain,
    localization::Localization,
    ui::widgets::curve_editor::draw_curve_editor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LevelsUiChannel {
    #[default]
    Master,
    Red,
    Green,
    Blue,
}

impl LevelsUiChannel {
    const ALL: [Self; 4] = [Self::Master, Self::Red, Self::Green, Self::Blue];

    fn get(self, levels: LevelsAdjustment) -> LevelsChannel {
        match self {
            Self::Master => levels.master,
            Self::Red => levels.red,
            Self::Green => levels.green,
            Self::Blue => levels.blue,
        }
    }

    fn set(self, levels: &mut LevelsAdjustment, channel: LevelsChannel) {
        match self {
            Self::Master => levels.master = channel,
            Self::Red => levels.red = channel,
            Self::Green => levels.green = channel,
            Self::Blue => levels.blue = channel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CurvesUiChannel {
    #[default]
    Master,
    Red,
    Green,
    Blue,
}

impl CurvesUiChannel {
    const ALL: [Self; 4] = [Self::Master, Self::Red, Self::Green, Self::Blue];

    fn get(self, curves: CurvesAdjustment) -> CurveChannel {
        match self {
            Self::Master => curves.master,
            Self::Red => curves.red,
            Self::Green => curves.green,
            Self::Blue => curves.blue,
        }
    }

    fn set(self, curves: &mut CurvesAdjustment, channel: CurveChannel) {
        match self {
            Self::Master => curves.master = channel,
            Self::Red => curves.red = channel,
            Self::Green => curves.green = channel,
            Self::Blue => curves.blue = channel,
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Self::Master => egui::Color32::from_gray(220),
            Self::Red => egui::Color32::from_rgb(235, 80, 80),
            Self::Green => egui::Color32::from_rgb(80, 210, 100),
            Self::Blue => egui::Color32::from_rgb(90, 140, 240),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct AdjustmentControlsState {
    levels_channel: LevelsUiChannel,
    curves_channel: CurvesUiChannel,
    curves_selected_point: Option<usize>,
}

impl AdjustmentControlsState {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct AdjustmentControlsOutput {
    pub(crate) changed: bool,
    pub(crate) discrete_change: bool,
    pub(crate) drag_started: bool,
    pub(crate) dragged: bool,
    pub(crate) drag_stopped: bool,
    pub(crate) interaction_ended: bool,
}

impl AdjustmentControlsOutput {
    fn observe_response(&mut self, response: &egui::Response) {
        if response.changed() {
            self.changed = true;
            self.drag_started |= response.drag_started();
            self.dragged |= response.dragged();
            self.drag_stopped |= response.drag_stopped();
        }
        if response.drag_stopped() {
            self.interaction_ended = true;
        }
    }

    fn mark_discrete(&mut self) {
        self.changed = true;
        self.discrete_change = true;
        self.interaction_ended = true;
    }
}

pub(crate) fn draw_adjustment_controls(
    ui: &mut egui::Ui,
    l10n: &Localization,
    adjustment: &mut Adjustment,
    state: &mut AdjustmentControlsState,
    domain: SurfaceFilterDomain,
) -> AdjustmentControlsOutput {
    let mut output = AdjustmentControlsOutput::default();
    match adjustment {
        Adjustment::BrightnessContrast(value) => {
            let response = ui.add(
                egui::Slider::new(&mut value.brightness, -150..=150)
                    .text(l10n.text("adjustment-brightness")),
            );
            output.observe_response(&response);
            let response = ui.add(
                egui::Slider::new(&mut value.contrast, -50..=100)
                    .text(l10n.text("adjustment-contrast")),
            );
            output.observe_response(&response);
            if ui.button(l10n.text("action-reset")).clicked() {
                *value = BrightnessContrastAdjustment::default();
                output.mark_discrete();
            }
        }
        Adjustment::Levels(levels) => draw_levels(ui, l10n, levels, state, domain, &mut output),
        Adjustment::Curves(curves) => draw_curves(ui, l10n, curves, state, domain, &mut output),
        Adjustment::HueSaturation(value) => {
            let response = ui.add(
                egui::Slider::new(&mut value.hue, -180..=180).text(l10n.text("adjustment-hue")),
            );
            output.observe_response(&response);
            let response = ui.add(
                egui::Slider::new(&mut value.saturation, -100..=100)
                    .text(l10n.text("adjustment-saturation")),
            );
            output.observe_response(&response);
            let response = ui.add(
                egui::Slider::new(&mut value.lightness, -100..=100)
                    .text(l10n.text("adjustment-lightness")),
            );
            output.observe_response(&response);
            if ui.button(l10n.text("action-reset")).clicked() {
                *value = HueSaturationAdjustment::default();
                output.mark_discrete();
            }
        }
        Adjustment::Invert => {
            ui.label(l10n.text("adjustment-invert-rgb-help"));
            ui.label(l10n.text("adjustment-invert-alpha-help"));
        }
        Adjustment::GradientMap(_) | Adjustment::UvMirror(_) => {}
    }
    output
}

fn draw_levels(
    ui: &mut egui::Ui,
    l10n: &Localization,
    levels: &mut LevelsAdjustment,
    state: &mut AdjustmentControlsState,
    domain: SurfaceFilterDomain,
    output: &mut AdjustmentControlsOutput,
) {
    if domain == SurfaceFilterDomain::Scalar {
        state.levels_channel = LevelsUiChannel::Master;
        levels.red = LevelsChannel::default();
        levels.green = LevelsChannel::default();
        levels.blue = LevelsChannel::default();
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.label(l10n.text("adjustment-channel"));
            for channel in LevelsUiChannel::ALL {
                ui.selectable_value(
                    &mut state.levels_channel,
                    channel,
                    levels_channel_label(l10n, channel),
                );
            }
        });
    }

    let mut channel = state.levels_channel.get(*levels);
    let mut input_black = channel.input_black;
    let response = ui.add(
        egui::Slider::new(&mut input_black, 0..=253)
            .integer()
            .text(l10n.text("adjustment-input-black")),
    );
    if response.changed() {
        channel.input_black = input_black.min(channel.input_white.saturating_sub(1));
        state.levels_channel.set(levels, channel);
    }
    output.observe_response(&response);

    let response = ui.add(
        egui::Slider::new(&mut channel.gamma, 0.1..=9.99)
            .logarithmic(true)
            .text(l10n.text("adjustment-gamma")),
    );
    if response.changed() {
        state.levels_channel.set(levels, channel);
    }
    output.observe_response(&response);

    let mut input_white = channel.input_white;
    let response = ui.add(
        egui::Slider::new(&mut input_white, 2..=255)
            .integer()
            .text(l10n.text("adjustment-input-white")),
    );
    if response.changed() {
        channel.input_white = input_white.max(channel.input_black.saturating_add(1));
        state.levels_channel.set(levels, channel);
    }
    output.observe_response(&response);

    let mut output_black = channel.output_black;
    let response = ui.add(
        egui::Slider::new(&mut output_black, 0..=255)
            .integer()
            .text(l10n.text("adjustment-output-black")),
    );
    if response.changed() {
        channel.output_black = output_black;
        state.levels_channel.set(levels, channel);
    }
    output.observe_response(&response);

    let mut output_white = channel.output_white;
    let response = ui.add(
        egui::Slider::new(&mut output_white, 0..=255)
            .integer()
            .text(l10n.text("adjustment-output-white")),
    );
    if response.changed() {
        channel.output_white = output_white;
        state.levels_channel.set(levels, channel);
    }
    output.observe_response(&response);

    ui.horizontal(|ui| {
        if ui.button(l10n.text("adjustment-reset-channel")).clicked() {
            state.levels_channel.set(levels, LevelsChannel::default());
            output.mark_discrete();
        }
        if ui.button(l10n.text("settings-reset-all")).clicked() {
            *levels = LevelsAdjustment::default();
            output.mark_discrete();
        }
    });
}

fn draw_curves(
    ui: &mut egui::Ui,
    l10n: &Localization,
    curves: &mut CurvesAdjustment,
    state: &mut AdjustmentControlsState,
    domain: SurfaceFilterDomain,
    output: &mut AdjustmentControlsOutput,
) {
    if domain == SurfaceFilterDomain::Scalar {
        state.curves_channel = CurvesUiChannel::Master;
        curves.red = CurveChannel::default();
        curves.green = CurveChannel::default();
        curves.blue = CurveChannel::default();
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.label(l10n.text("adjustment-channel"));
            for channel in CurvesUiChannel::ALL {
                if ui
                    .selectable_value(
                        &mut state.curves_channel,
                        channel,
                        curves_channel_label(l10n, channel),
                    )
                    .changed()
                {
                    state.curves_selected_point = None;
                    output.interaction_ended = true;
                }
            }
        });
    }

    let mut channel = state.curves_channel.get(*curves);
    let graph_output = draw_curve_editor(
        ui,
        &mut channel,
        &mut state.curves_selected_point,
        state.curves_channel.color(),
        Some(220.0),
        None,
    );
    if graph_output.changed {
        state.curves_channel.set(curves, channel);
        if graph_output.discrete_change {
            output.mark_discrete();
        } else {
            output.observe_response(&graph_output.response);
        }
    } else if graph_output.response.drag_stopped() {
        output.interaction_ended = true;
        output.drag_stopped = true;
    }

    if let Some(index) = state.curves_selected_point
        && let Some(point) = channel.point(index)
    {
        ui.separator();
        let mut args = fluent::FluentArgs::new();
        args.set("index", (index + 1) as i64);
        args.set("count", channel.points().len() as i64);
        ui.label(l10n.format("adjustment-point-position", Some(&args)));
        ui.horizontal(|ui| {
            let mut input = point.input;
            let input_response = ui.add(
                egui::DragValue::new(&mut input)
                    .prefix(l10n.text("adjustment-input-prefix"))
                    .range(0..=255)
                    .speed(1.0),
            );
            if input_response.changed() {
                let mut edited = point;
                edited.input = input;
                if channel.set_point(index, edited) {
                    state.curves_channel.set(curves, channel);
                    output.observe_response(&input_response);
                }
            } else if input_response.drag_stopped() {
                output.interaction_ended = true;
            }

            let mut value = channel.point(index).unwrap_or(point).output;
            let output_response = ui.add(
                egui::DragValue::new(&mut value)
                    .prefix(l10n.text("adjustment-output-prefix"))
                    .range(0..=255)
                    .speed(1.0),
            );
            if output_response.changed() {
                let mut edited = channel.point(index).unwrap_or(point);
                edited.output = value;
                if channel.set_point(index, edited) {
                    state.curves_channel.set(curves, channel);
                    output.observe_response(&output_response);
                }
            } else if output_response.drag_stopped() {
                output.interaction_ended = true;
            }
        });
    }

    ui.horizontal_wrapped(|ui| {
        let can_delete = state
            .curves_selected_point
            .is_some_and(|index| index < channel.points().len() && channel.points().len() > 2);
        if ui
            .add_available(
                can_delete,
                egui::Button::new(l10n.text("adjustment-delete-point")),
            )
            .clicked()
            && let Some(index) = state.curves_selected_point
            && channel.remove_point(index)
        {
            state.curves_selected_point = None;
            state.curves_channel.set(curves, channel);
            output.mark_discrete();
        }
        if ui.button(l10n.text("adjustment-reset-channel")).clicked() {
            state.curves_selected_point = None;
            state.curves_channel.set(curves, CurveChannel::default());
            output.mark_discrete();
        }
        if ui.button(l10n.text("settings-reset-all")).clicked() {
            state.curves_selected_point = None;
            *curves = CurvesAdjustment::default();
            output.mark_discrete();
        }
        let mut args = fluent::FluentArgs::new();
        args.set("count", channel.points().len() as i64);
        args.set("maximum", MAX_CURVE_POINTS as i64);
        ui.weak(l10n.format("adjustment-points-count", Some(&args)));
    });
    ui.weak(l10n.text("adjustment-curves-help"));
}

fn levels_channel_label(l10n: &Localization, channel: LevelsUiChannel) -> String {
    l10n.text(match channel {
        LevelsUiChannel::Master => "adjustment-channel-rgb",
        LevelsUiChannel::Red => "adjustment-channel-red",
        LevelsUiChannel::Green => "adjustment-channel-green",
        LevelsUiChannel::Blue => "adjustment-channel-blue",
    })
}

fn curves_channel_label(l10n: &Localization, channel: CurvesUiChannel) -> String {
    l10n.text(match channel {
        CurvesUiChannel::Master => "adjustment-channel-rgb",
        CurvesUiChannel::Red => "adjustment-channel-red",
        CurvesUiChannel::Green => "adjustment-channel-green",
        CurvesUiChannel::Blue => "adjustment-channel-blue",
    })
}
