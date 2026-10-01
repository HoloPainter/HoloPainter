//! Keep transient input blocking separate from intrinsic unavailability.
use eframe::egui;

const AVAILABILITY_TAG: &str = "holopainter.visual_availability";

pub(crate) fn visually_available(ui: &egui::Ui) -> bool {
    ui.stack()
        .iter()
        .find_map(|node| node.tags().get_downcast::<bool>(AVAILABILITY_TAG).copied())
        .unwrap_or_else(|| ui.is_enabled())
}

pub(crate) trait InteractionGate {
    fn availability_ui<R>(
        &mut self,
        available: bool,
        input_blocked: bool,
        contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R>;
    fn add_available(&mut self, available: bool, widget: impl egui::Widget) -> egui::Response;
    fn add_available_ui<R>(
        &mut self,
        available: bool,
        contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R>;
}

impl InteractionGate for egui::Ui {
    fn availability_ui<R>(
        &mut self,
        available: bool,
        input_blocked: bool,
        contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let parent_available = visually_available(self);
        let visual_available = parent_available && available;
        let builder = egui::UiBuilder::new().ui_stack_info(
            egui::UiStackInfo::default().with_tag_value(AVAILABILITY_TAG, visual_available),
        );
        self.scope_builder(builder, |ui| {
            let opacity = ui.opacity();
            if !available || input_blocked {
                ui.disable();
                // Restore the inherited opacity before applying intrinsic unavailability.
                ui.set_opacity(opacity);
            }
            if parent_available && !available {
                ui.multiply_opacity(ui.visuals().disabled_alpha());
            }
            contents(ui)
        })
    }

    fn add_available(&mut self, available: bool, widget: impl egui::Widget) -> egui::Response {
        self.add_available_ui(available, |ui| ui.add(widget)).inner
    }

    fn add_available_ui<R>(
        &mut self,
        available: bool,
        contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        self.availability_ui(available, false, contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_gate_preserves_opacity_and_nested_intrinsic_unavailability() {
        egui::__run_test_ui(|ui| {
            ui.set_opacity(0.8);
            let alpha = ui.visuals().disabled_alpha();
            ui.availability_ui(true, true, |ui| {
                assert!(!ui.is_enabled());
                assert!(visually_available(ui));
                assert_eq!(ui.opacity(), 0.8);
                ui.add_available_ui(false, |ui| {
                    assert!(!ui.is_enabled());
                    assert!(!visually_available(ui));
                    assert_eq!(ui.opacity(), 0.8 * alpha);
                    ui.add_available_ui(false, |ui| {
                        assert_eq!(ui.opacity(), 0.8 * alpha);
                    });
                });
                assert!(visually_available(ui));
                assert_eq!(ui.opacity(), 0.8);
            });
            assert!(ui.is_enabled());
            assert_eq!(ui.opacity(), 0.8);
        });
    }

    #[test]
    fn silent_gate_does_not_brighten_an_intrinsically_disabled_parent() {
        egui::__run_test_ui(|ui| {
            ui.add_enabled_ui(false, |ui| {
                let opacity = ui.opacity();
                ui.availability_ui(true, true, |ui| {
                    assert!(!visually_available(ui));
                    assert_eq!(ui.opacity(), opacity);
                });
            });
        });
    }

    #[test]
    fn transient_gate_rejects_pointer_clicks_and_recovers_when_unblocked() {
        let ctx = egui::Context::default();
        let mut center = egui::Pos2::ZERO;
        let mut clicked = false;
        let mut draw = |blocked: bool, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.availability_ui(true, blocked, |ui| {
                        let response = ui.button("action");
                        center = response.rect.center();
                        clicked = response.clicked();
                    });
                });
            });
            output.textures_delta.clear();
            (center, clicked)
        };
        let (center, _) = draw(true, vec![]);
        let click = |pressed| {
            vec![
                egui::Event::PointerMoved(center),
                egui::Event::PointerButton {
                    pos: center,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        assert!(!draw(true, click(true)).1);
        assert!(!draw(true, click(false)).1);
        draw(false, vec![]);
        draw(false, click(true));
        assert!(draw(false, click(false)).1);
    }
}
