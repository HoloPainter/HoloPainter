use eframe::egui;
use glam::Vec2;

use crate::{
    application::{InputModifiers, ToolCancelReason, ViewPointerPhase},
    core::curve::Curve,
    settings::TabletBackend,
};

use super::{
    modifiers::input_modifiers_from_egui,
    native_window,
    tablet::{
        TabletBackendStatus, TabletInputState, TabletPointerKind, TabletSample, TabletSamplePhase,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewPointerTarget {
    Viewport3d,
    Uv,
}

#[derive(Default)]
pub struct ViewPointerInputRouter {
    active_touch: Option<ActiveTouchStroke>,
    active_tablet: Option<ActiveTabletStroke>,
    pending_tablet_cancellation: Option<ViewPointerTarget>,
    tablet: TabletInputState,
    tablet_samples: Vec<QueuedTabletSample>,
    tablet_pressure_raw: Option<f32>,
    suppress_egui_pointer_until: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveTouchStroke {
    id: egui::TouchId,
    target: ViewPointerTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveTabletStroke {
    target: ViewPointerTarget,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct QueuedTabletSample {
    sample: ViewPointerNativeSample,
    consumed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPointerInputSample {
    pub phase: ViewPointerPhase,
    pub position_px: Vec2,
    pub pressure: f32,
    pub time_s: f64,
    pub modifiers: InputModifiers,
}

#[derive(Debug, Default)]
pub struct ViewPointerInputFrame {
    pub saw_touch_event: bool,
    pub saw_tablet_event: bool,
    pub suppress_egui_pointer: bool,
    pub samples: Vec<ViewPointerInputSample>,
    pub cancellations: Vec<ToolCancelReason>,
}

pub(crate) fn pointer_down_position(
    current_pointer: egui::Pos2,
    press_origin: Option<egui::Pos2>,
    view_rect: egui::Rect,
) -> egui::Pos2 {
    press_origin
        .filter(|pointer| view_rect.contains(*pointer))
        .unwrap_or(current_pointer)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ViewPointerNativeSample {
    phase: ViewPointerPhase,
    client_pos_points: egui::Pos2,
    pressure: f32,
    _kind: TabletPointerKind,
    _secondary: bool,
}

impl ViewPointerInputRouter {
    const MOUSE_SUPPRESSION_SECONDS: f64 = 0.08;

    pub fn begin_frame(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        requested_backend: TabletBackend,
        pressure_curve: Curve,
    ) -> Option<TabletBackend> {
        let hwnd = native_window::frame_hwnd(frame);
        let sync = self.tablet.sync_backend(hwnd, requested_backend);
        if sync.active_backend_changed {
            self.pending_tablet_cancellation = self.active_tablet.map(|active| active.target);
            self.tablet_pressure_raw = None;
            self.cancel_active_input();
        }

        let time_s = ctx.input(|input| input.time);
        let ppp = ctx.pixels_per_point().max(1.0);
        let samples = self.tablet.poll_samples(ppp);
        if let Some(sample) = samples.last() {
            self.tablet_pressure_raw = Some(sample.pressure);
        }
        self.tablet_samples = samples
            .into_iter()
            .map(|sample| apply_tablet_pressure_curve(sample, pressure_curve))
            .map(|sample| QueuedTabletSample {
                sample: ViewPointerNativeSample::from(sample),
                consumed: false,
            })
            .collect();

        if !self.tablet_samples.is_empty() {
            self.suppress_egui_pointer_until = time_s + Self::MOUSE_SUPPRESSION_SECONDS;
            ctx.request_repaint();
        }
        sync.normalized_backend
    }

    pub fn tablet_status(&self) -> TabletBackendStatus {
        self.tablet.status()
    }

    pub fn tablet_pressure_raw(&self) -> Option<f32> {
        self.tablet_pressure_raw
    }

    pub fn collect_view_samples(
        &mut self,
        ctx: &egui::Context,
        target: ViewPointerTarget,
        rect: egui::Rect,
        blocked_rects: &[egui::Rect],
        input_enabled: bool,
        can_start: bool,
    ) -> ViewPointerInputFrame {
        let (events, time_s, modifiers) = ctx.input(|input| {
            (
                input.events.clone(),
                input.time,
                input_modifiers_from_egui(input.modifiers),
            )
        });
        let mut frame = ViewPointerInputFrame {
            saw_tablet_event: self.tablet_samples.iter().any(|sample| !sample.consumed),
            suppress_egui_pointer: self.should_suppress_egui_pointer(time_s),
            ..Default::default()
        };
        if self.pending_tablet_cancellation == Some(target) {
            self.pending_tablet_cancellation = None;
            frame
                .cancellations
                .push(ToolCancelReason::PointerCaptureLost);
        }
        if !input_enabled {
            if self.cancel_active_input() {
                frame
                    .cancellations
                    .push(ToolCancelReason::PointerCaptureLost);
            }
            frame.suppress_egui_pointer = true;
            return frame;
        }

        self.collect_touch_samples(
            events,
            target,
            rect,
            blocked_rects,
            can_start,
            time_s,
            modifiers,
            &mut frame,
        );
        self.collect_tablet_samples(
            target,
            rect,
            blocked_rects,
            can_start,
            time_s,
            modifiers,
            &mut frame,
        );

        frame
    }

    pub fn is_active(&self) -> bool {
        self.active_touch.is_some() || self.active_tablet.is_some()
    }

    pub fn cancel_active_input(&mut self) -> bool {
        let was_active = self.is_active();
        self.active_touch = None;
        self.active_tablet = None;
        for sample in &mut self.tablet_samples {
            sample.consumed = true;
        }
        was_active
    }

    fn should_suppress_egui_pointer(&self, time_s: f64) -> bool {
        self.active_tablet.is_some() || time_s < self.suppress_egui_pointer_until
    }

    fn collect_touch_samples(
        &mut self,
        events: Vec<egui::Event>,
        target: ViewPointerTarget,
        rect: egui::Rect,
        blocked_rects: &[egui::Rect],
        can_start: bool,
        time_s: f64,
        modifiers: InputModifiers,
        frame: &mut ViewPointerInputFrame,
    ) {
        for event in events {
            let egui::Event::Touch {
                id,
                phase,
                pos,
                force,
                ..
            } = event
            else {
                continue;
            };
            frame.saw_touch_event = true;

            match phase {
                egui::TouchPhase::Start => {
                    if self.active_touch.is_none()
                        && can_start
                        && rect.contains(pos)
                        && !position_is_blocked(pos, blocked_rects)
                    {
                        self.active_touch = Some(ActiveTouchStroke { id, target });
                        frame.samples.push(sample_from_pos(
                            ViewPointerPhase::Down,
                            pos,
                            rect,
                            force.unwrap_or(1.0),
                            time_s,
                            modifiers,
                        ));
                    }
                }
                egui::TouchPhase::Move => {
                    if self.touch_active_matches(target, id) {
                        frame.samples.push(sample_from_pos(
                            ViewPointerPhase::Move,
                            pos,
                            rect,
                            force.unwrap_or(1.0),
                            time_s,
                            modifiers,
                        ));
                    }
                }
                egui::TouchPhase::End => {
                    if self.touch_active_matches(target, id) {
                        frame.samples.push(sample_from_pos(
                            ViewPointerPhase::Up,
                            pos,
                            rect,
                            force.unwrap_or(1.0),
                            time_s,
                            modifiers,
                        ));
                        self.active_touch = None;
                    }
                }
                egui::TouchPhase::Cancel => {
                    if self.touch_active_matches(target, id) {
                        frame.cancellations.push(ToolCancelReason::TouchCancelled);
                        self.active_touch = None;
                    }
                }
            }
        }
    }

    fn collect_tablet_samples(
        &mut self,
        target: ViewPointerTarget,
        rect: egui::Rect,
        blocked_rects: &[egui::Rect],
        can_start: bool,
        time_s: f64,
        modifiers: InputModifiers,
        frame: &mut ViewPointerInputFrame,
    ) {
        for idx in 0..self.tablet_samples.len() {
            if self.tablet_samples[idx].consumed {
                continue;
            }
            let sample = self.tablet_samples[idx].sample;
            match sample.phase {
                ViewPointerPhase::Down => {
                    if self.active_tablet.is_none()
                        && can_start
                        && rect.contains(sample.client_pos_points)
                        && !position_is_blocked(sample.client_pos_points, blocked_rects)
                    {
                        self.active_tablet = Some(ActiveTabletStroke { target });
                        frame.samples.push(sample_from_pos(
                            ViewPointerPhase::Down,
                            sample.client_pos_points,
                            rect,
                            sample.pressure,
                            time_s,
                            modifiers,
                        ));
                        self.tablet_samples[idx].consumed = true;
                    }
                }
                ViewPointerPhase::Move => {
                    if self.tablet_active_matches(target) {
                        frame.samples.push(sample_from_pos(
                            ViewPointerPhase::Move,
                            sample.client_pos_points,
                            rect,
                            sample.pressure,
                            time_s,
                            modifiers,
                        ));
                        self.tablet_samples[idx].consumed = true;
                    }
                }
                ViewPointerPhase::Up => {
                    if self.tablet_active_matches(target) {
                        frame.samples.push(sample_from_pos(
                            ViewPointerPhase::Up,
                            sample.client_pos_points,
                            rect,
                            sample.pressure,
                            time_s,
                            modifiers,
                        ));
                        self.active_tablet = None;
                        self.tablet_samples[idx].consumed = true;
                    }
                }
                ViewPointerPhase::Click => {
                    self.tablet_samples[idx].consumed = true;
                }
            }
        }
    }

    fn touch_active_matches(&self, target: ViewPointerTarget, id: egui::TouchId) -> bool {
        self.active_touch
            .is_some_and(|active| active.target == target && active.id == id)
    }

    fn tablet_active_matches(&self, target: ViewPointerTarget) -> bool {
        self.active_tablet
            .is_some_and(|active| active.target == target)
    }
}

fn apply_tablet_pressure_curve(mut sample: TabletSample, curve: Curve) -> TabletSample {
    sample.pressure = curve.evaluate(sample.pressure);
    sample
}

impl From<TabletSample> for ViewPointerNativeSample {
    fn from(sample: TabletSample) -> Self {
        Self {
            phase: match sample.phase {
                TabletSamplePhase::Down => ViewPointerPhase::Down,
                TabletSamplePhase::Move => ViewPointerPhase::Move,
                TabletSamplePhase::Up => ViewPointerPhase::Up,
            },
            client_pos_points: sample.client_pos_points,
            pressure: sample.pressure,
            _kind: sample.kind,
            _secondary: sample.secondary,
        }
    }
}

fn position_is_blocked(position: egui::Pos2, blocked_rects: &[egui::Rect]) -> bool {
    blocked_rects
        .iter()
        .any(|blocked_rect| blocked_rect.contains(position))
}

fn sample_from_pos(
    phase: ViewPointerPhase,
    pos: egui::Pos2,
    rect: egui::Rect,
    pressure: f32,
    time_s: f64,
    modifiers: InputModifiers,
) -> ViewPointerInputSample {
    ViewPointerInputSample {
        phase,
        position_px: local_px(pos, rect),
        pressure: pressure.clamp(0.0, 1.0),
        time_s,
        modifiers,
    }
}

fn local_px(pointer: egui::Pos2, rect: egui::Rect) -> Vec2 {
    let local = pointer - rect.min;
    Vec2::new(local.x, local.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::curve::CurvePoint, ui::input::tablet::TabletInputSource};

    #[test]
    fn tablet_pressure_curve_changes_pressure_without_changing_event_identity() {
        let sample = TabletSample {
            phase: TabletSamplePhase::Down,
            client_pos_points: egui::pos2(12.0, 34.0),
            pressure: 0.5,
            kind: TabletPointerKind::Eraser,
            secondary: true,
            source: TabletInputSource::WindowsInk,
        };
        let mut curve = Curve::default();
        assert!(curve.insert_point(CurvePoint { x: 0.5, y: 0.25 }).is_some());

        let adjusted = apply_tablet_pressure_curve(sample, curve);

        assert_eq!(adjusted.pressure, 0.25);
        assert_eq!(adjusted.phase, sample.phase);
        assert_eq!(adjusted.kind, sample.kind);
        assert_eq!(adjusted.source, sample.source);
    }

    #[test]
    fn pointer_down_uses_press_origin_inside_view() {
        let view_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 100.0));
        let press_origin = egui::pos2(20.0, 30.0);
        let current_pointer = egui::pos2(30.0, 40.0);

        assert_eq!(
            pointer_down_position(current_pointer, Some(press_origin), view_rect),
            press_origin
        );
    }

    #[test]
    fn pointer_down_falls_back_when_press_origin_is_outside_view() {
        let view_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 100.0));
        let press_origin = egui::pos2(5.0, 10.0);
        let current_pointer = egui::pos2(30.0, 40.0);

        assert_eq!(
            pointer_down_position(current_pointer, Some(press_origin), view_rect),
            current_pointer
        );
    }
}
