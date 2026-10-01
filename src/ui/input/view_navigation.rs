use eframe::egui::{self, PointerButton};

use super::shortcut_profile::{PointerContext, ShortcutPointerButton, ShortcutProfile};

pub(crate) use super::shortcut_profile::PointerGestureAction as ViewNavigationAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewNavigationKind {
    Viewport3d,
    Uv,
}

pub(crate) fn dominant_drag_delta(delta: egui::Vec2) -> f32 {
    if delta.x.abs() >= delta.y.abs() {
        delta.x
    } else {
        delta.y
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ViewNavigationState {
    active: Option<ActiveViewNavigation>,
    profile_revision: u64,
}

#[derive(Debug, Clone, Copy)]
struct ActiveViewNavigation {
    action: ViewNavigationAction,
    button: PointerButton,
    start_position: egui::Pos2,
    previous_position: egui::Pos2,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewNavigationFrame {
    action: Option<ViewNavigationAction>,
    start_position: egui::Pos2,
    pointer_delta: egui::Vec2,
    total_delta: egui::Vec2,
    started: bool,
    ended: bool,
    consumes_primary: bool,
}

impl Default for ViewNavigationFrame {
    fn default() -> Self {
        Self {
            action: None,
            start_position: egui::pos2(0.0, 0.0),
            pointer_delta: egui::Vec2::ZERO,
            total_delta: egui::Vec2::ZERO,
            started: false,
            ended: false,
            consumes_primary: false,
        }
    }
}

impl ViewNavigationFrame {
    pub fn action(self) -> Option<ViewNavigationAction> {
        self.action
    }

    pub fn start_position(self) -> egui::Pos2 {
        self.start_position
    }

    pub fn pointer_delta(self) -> egui::Vec2 {
        self.pointer_delta
    }

    pub fn total_delta(self) -> egui::Vec2 {
        self.total_delta
    }

    pub fn started(self) -> bool {
        self.started
    }

    pub fn ended(self) -> bool {
        self.ended
    }

    pub fn owns_input(self) -> bool {
        self.action.is_some()
    }

    pub fn consumes_primary(self) -> bool {
        self.consumes_primary
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ViewNavigationInput {
    focused: bool,
    modifiers: egui::Modifiers,
    pointer_position: Option<egui::Pos2>,
    press_origin: Option<egui::Pos2>,
    primary_pressed: bool,
    middle_pressed: bool,
    secondary_pressed: bool,
    extra1_pressed: bool,
    extra2_pressed: bool,
    primary_down: bool,
    middle_down: bool,
    secondary_down: bool,
    extra1_down: bool,
    extra2_down: bool,
}

impl ViewNavigationInput {
    fn from_context(ctx: &egui::Context) -> Self {
        ctx.input(|input| Self {
            focused: input.focused,
            modifiers: input.modifiers,
            pointer_position: input.pointer.interact_pos(),
            press_origin: input.pointer.press_origin(),
            primary_pressed: input.pointer.button_pressed(PointerButton::Primary),
            middle_pressed: input.pointer.button_pressed(PointerButton::Middle),
            secondary_pressed: input.pointer.button_pressed(PointerButton::Secondary),
            extra1_pressed: input.pointer.button_pressed(PointerButton::Extra1),
            extra2_pressed: input.pointer.button_pressed(PointerButton::Extra2),
            primary_down: input.pointer.button_down(PointerButton::Primary),
            middle_down: input.pointer.button_down(PointerButton::Middle),
            secondary_down: input.pointer.button_down(PointerButton::Secondary),
            extra1_down: input.pointer.button_down(PointerButton::Extra1),
            extra2_down: input.pointer.button_down(PointerButton::Extra2),
        })
    }

    fn first_pressed_button(self) -> Option<PointerButton> {
        if self.primary_pressed {
            Some(PointerButton::Primary)
        } else if self.middle_pressed {
            Some(PointerButton::Middle)
        } else if self.secondary_pressed {
            Some(PointerButton::Secondary)
        } else if self.extra1_pressed {
            Some(PointerButton::Extra1)
        } else if self.extra2_pressed {
            Some(PointerButton::Extra2)
        } else {
            None
        }
    }

    fn button_down(self, button: PointerButton) -> bool {
        match button {
            PointerButton::Primary => self.primary_down,
            PointerButton::Middle => self.middle_down,
            PointerButton::Secondary => self.secondary_down,
            PointerButton::Extra1 => self.extra1_down,
            PointerButton::Extra2 => self.extra2_down,
        }
    }

    fn another_button_is_down(self, button: PointerButton) -> bool {
        (button != PointerButton::Primary && self.primary_down)
            || (button != PointerButton::Middle && self.middle_down)
            || (button != PointerButton::Secondary && self.secondary_down)
            || (button != PointerButton::Extra1 && self.extra1_down)
            || (button != PointerButton::Extra2 && self.extra2_down)
    }
}

pub fn update_view_navigation(
    ctx: &egui::Context,
    id: egui::Id,
    kind: ViewNavigationKind,
    view_rect: egui::Rect,
    input_enabled: bool,
    can_start: bool,
    profile: (&ShortcutProfile, u64),
) -> ViewNavigationFrame {
    let (profile, profile_revision) = profile;
    let input = ViewNavigationInput::from_context(ctx);
    let mut state = ctx
        .data_mut(|data| data.get_persisted::<ViewNavigationState>(id))
        .unwrap_or_default();
    if state.profile_revision != profile_revision {
        state.active = None;
        state.profile_revision = profile_revision;
    }
    let frame = if input_enabled {
        state.update_with_profile(kind, view_rect, can_start, input, profile)
    } else {
        state.active = None;
        ViewNavigationFrame::default()
    };
    ctx.data_mut(|data| data.insert_persisted(id, state));
    frame
}

impl ViewNavigationState {
    fn update_with_profile(
        &mut self,
        kind: ViewNavigationKind,
        view_rect: egui::Rect,
        can_start: bool,
        input: ViewNavigationInput,
        profile: &ShortcutProfile,
    ) -> ViewNavigationFrame {
        let mut started = false;
        if self.active.is_none() && can_start && input.focused {
            let button = input.first_pressed_button();
            let start_position = input.press_origin.or(input.pointer_position);
            if let (Some(button), Some(start_position)) = (button, start_position)
                && !input.another_button_is_down(button)
                && view_rect.contains(start_position)
                && let Some(action) = resolve_profile_action(profile, kind, button, input.modifiers)
            {
                self.active = Some(ActiveViewNavigation {
                    action,
                    button,
                    start_position,
                    previous_position: start_position,
                });
                started = true;
            }
        }

        let Some(active) = self.active else {
            return ViewNavigationFrame::default();
        };
        let pointer_position = input.pointer_position.unwrap_or(active.previous_position);
        let pointer_delta = pointer_position - active.previous_position;
        let total_delta = pointer_position - active.start_position;
        let still_active = input.focused && input.button_down(active.button);
        if still_active {
            self.active = Some(ActiveViewNavigation {
                previous_position: pointer_position,
                ..active
            });
        } else {
            self.active = None;
        }

        ViewNavigationFrame {
            action: Some(active.action),
            start_position: active.start_position,
            pointer_delta,
            total_delta,
            started,
            ended: !still_active,
            consumes_primary: active.button == PointerButton::Primary,
        }
    }
}

fn resolve_profile_action(
    profile: &ShortcutProfile,
    kind: ViewNavigationKind,
    button: PointerButton,
    modifiers: egui::Modifiers,
) -> Option<ViewNavigationAction> {
    let context = match kind {
        ViewNavigationKind::Viewport3d => PointerContext::Viewport3d,
        ViewNavigationKind::Uv => PointerContext::Uv,
    };
    let button = match button {
        PointerButton::Primary => ShortcutPointerButton::Primary,
        PointerButton::Middle => ShortcutPointerButton::Middle,
        PointerButton::Secondary => ShortcutPointerButton::Secondary,
        PointerButton::Extra1 => ShortcutPointerButton::Extra1,
        PointerButton::Extra2 => ShortcutPointerButton::Extra2,
    };
    profile.pointer_action(context, button, modifiers)
}

#[cfg(test)]
fn resolve_action(
    kind: ViewNavigationKind,
    button: PointerButton,
    modifiers: egui::Modifiers,
) -> Option<ViewNavigationAction> {
    resolve_profile_action(
        &ShortcutProfile::load_default().expect("default shortcut profile"),
        kind,
        button,
        modifiers,
    )
}

#[cfg(test)]
impl ViewNavigationState {
    fn update(
        &mut self,
        kind: ViewNavigationKind,
        view_rect: egui::Rect,
        can_start: bool,
        input: ViewNavigationInput,
    ) -> ViewNavigationFrame {
        self.update_with_profile(
            kind,
            view_rect,
            can_start,
            input,
            &ShortcutProfile::load_default().expect("default shortcut profile"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_rect() -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0))
    }

    fn modifiers(alt: bool, ctrl: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            alt,
            ctrl,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn dominant_drag_delta_accepts_horizontal_and_vertical_motion() {
        assert_eq!(dominant_drag_delta(egui::vec2(12.0, 3.0)), 12.0);
        assert_eq!(dominant_drag_delta(egui::vec2(-12.0, 3.0)), -12.0);
        assert_eq!(dominant_drag_delta(egui::vec2(3.0, 12.0)), 12.0);
        assert_eq!(dominant_drag_delta(egui::vec2(3.0, -12.0)), -12.0);
    }

    #[test]
    fn dominant_drag_delta_does_not_cancel_opposing_diagonal_motion() {
        assert_eq!(dominant_drag_delta(egui::vec2(12.0, -11.0)), 12.0);
        assert_eq!(dominant_drag_delta(egui::vec2(11.0, -12.0)), -12.0);
    }

    fn input_for_press(button: PointerButton, modifiers: egui::Modifiers) -> ViewNavigationInput {
        let mut input = ViewNavigationInput {
            focused: true,
            modifiers,
            pointer_position: Some(egui::pos2(100.0, 100.0)),
            press_origin: Some(egui::pos2(100.0, 100.0)),
            ..Default::default()
        };
        match button {
            PointerButton::Primary => {
                input.primary_pressed = true;
                input.primary_down = true;
            }
            PointerButton::Middle => {
                input.middle_pressed = true;
                input.middle_down = true;
            }
            PointerButton::Secondary => {
                input.secondary_pressed = true;
                input.secondary_down = true;
            }
            PointerButton::Extra1 | PointerButton::Extra2 => {}
        }
        input
    }

    #[test]
    fn resolves_three_d_navigation_bindings() {
        assert_eq!(
            resolve_action(
                ViewNavigationKind::Viewport3d,
                PointerButton::Primary,
                modifiers(true, false, false),
            ),
            Some(ViewNavigationAction::Orbit3d)
        );
        assert_eq!(
            resolve_action(
                ViewNavigationKind::Viewport3d,
                PointerButton::Middle,
                modifiers(false, false, false),
            ),
            Some(ViewNavigationAction::Pan3d)
        );
        assert_eq!(
            resolve_action(
                ViewNavigationKind::Viewport3d,
                PointerButton::Secondary,
                modifiers(true, false, false),
            ),
            Some(ViewNavigationAction::Zoom3d)
        );
        assert_eq!(
            resolve_action(
                ViewNavigationKind::Viewport3d,
                PointerButton::Secondary,
                modifiers(false, false, false),
            ),
            Some(ViewNavigationAction::Orbit3d)
        );
    }

    #[test]
    fn resolves_uv_navigation() {
        assert_eq!(
            resolve_action(
                ViewNavigationKind::Uv,
                PointerButton::Primary,
                modifiers(true, false, false),
            ),
            Some(ViewNavigationAction::RotateUv)
        );
        assert_eq!(
            resolve_action(
                ViewNavigationKind::Uv,
                PointerButton::Middle,
                modifiers(false, false, false),
            ),
            Some(ViewNavigationAction::PanUv)
        );
    }

    #[test]
    fn ctrl_alt_primary_selects_brush_size_before_alt_navigation() {
        for kind in [ViewNavigationKind::Viewport3d, ViewNavigationKind::Uv] {
            assert_eq!(
                resolve_action(kind, PointerButton::Primary, modifiers(true, true, false),),
                Some(ViewNavigationAction::BrushSize)
            );
        }
    }

    #[test]
    fn gesture_kind_is_latched_until_button_release() {
        let mut state = ViewNavigationState::default();
        let started = state.update(
            ViewNavigationKind::Uv,
            view_rect(),
            true,
            input_for_press(PointerButton::Primary, modifiers(true, false, false)),
        );
        assert_eq!(started.action(), Some(ViewNavigationAction::RotateUv));
        assert!(started.started());
        assert!(started.consumes_primary());

        let moved = state.update(
            ViewNavigationKind::Uv,
            view_rect(),
            true,
            ViewNavigationInput {
                focused: true,
                modifiers: modifiers(true, true, false),
                pointer_position: Some(egui::pos2(125.0, 90.0)),
                primary_down: true,
                ..Default::default()
            },
        );
        assert_eq!(moved.action(), Some(ViewNavigationAction::RotateUv));
        assert_eq!(moved.total_delta(), egui::vec2(25.0, -10.0));
        assert!(!moved.ended());
    }

    #[test]
    fn second_button_cannot_take_navigation_ownership() {
        let mut input = input_for_press(PointerButton::Middle, modifiers(true, false, false));
        input.primary_down = true;
        let mut state = ViewNavigationState::default();

        let frame = state.update(ViewNavigationKind::Viewport3d, view_rect(), true, input);

        assert!(!frame.owns_input());
    }

    #[test]
    fn release_frame_still_consumes_primary_input() {
        let mut state = ViewNavigationState::default();
        state.update(
            ViewNavigationKind::Viewport3d,
            view_rect(),
            true,
            input_for_press(PointerButton::Primary, modifiers(true, false, false)),
        );
        let released = state.update(
            ViewNavigationKind::Viewport3d,
            view_rect(),
            true,
            ViewNavigationInput {
                focused: true,
                pointer_position: Some(egui::pos2(110.0, 100.0)),
                ..Default::default()
            },
        );
        assert!(released.ended());
        assert!(released.consumes_primary());
        assert_eq!(released.action(), Some(ViewNavigationAction::Orbit3d));

        let next = state.update(
            ViewNavigationKind::Viewport3d,
            view_rect(),
            true,
            ViewNavigationInput {
                focused: true,
                ..Default::default()
            },
        );
        assert!(!next.owns_input());
    }

    #[test]
    fn press_outside_view_does_not_start_navigation() {
        let mut state = ViewNavigationState::default();
        let mut input = input_for_press(PointerButton::Secondary, modifiers(false, false, false));
        input.pointer_position = Some(egui::pos2(500.0, 100.0));
        input.press_origin = input.pointer_position;
        let frame = state.update(ViewNavigationKind::Viewport3d, view_rect(), true, input);
        assert!(!frame.owns_input());
    }
}
