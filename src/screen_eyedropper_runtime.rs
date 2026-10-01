use std::time::Duration;

use eframe::egui::{self, Color32};

use crate::ui::input::screen_eyedropper::{self, EyedropperOverlay, GlobalEyedropperSession};

const UPDATE_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ScreenEyedropperUpdate {
    None,
    Apply([f32; 3]),
    Cancelled,
}

#[derive(Default)]
pub(crate) struct ScreenEyedropperRuntime {
    start_requested: bool,
    active: bool,
    last_escape_down: bool,
    preview_color: Option<Color32>,
    session: Option<GlobalEyedropperSession>,
    overlay: Option<EyedropperOverlay>,
}

impl ScreenEyedropperRuntime {
    pub(crate) fn request_start(&mut self) {
        if !self.active {
            self.start_requested = true;
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn start_if_requested(
        &mut self,
        owner_hwnd: Option<isize>,
        no_sample_text: String,
        instructions_text: String,
    ) -> Result<bool, String> {
        if !std::mem::take(&mut self.start_requested) || self.active {
            return Ok(false);
        }
        if !screen_eyedropper::available() {
            return Err("Screen eyedropper is only available on Windows.".to_owned());
        }

        let session = GlobalEyedropperSession::begin()?;
        let mut overlay = EyedropperOverlay::new(owner_hwnd, no_sample_text, instructions_text)?;
        self.last_escape_down = screen_eyedropper::escape_down();
        self.preview_color = None;

        if let Some(sample) = screen_eyedropper::current_screen_sample() {
            self.preview_color = sample.color.map(rgb_only);
            overlay.update(sample.x, sample.y, self.preview_color);
        }

        self.session = Some(session);
        self.overlay = Some(overlay);
        self.active = true;
        Ok(true)
    }

    pub(crate) fn update(&mut self, ctx: &egui::Context) -> ScreenEyedropperUpdate {
        if !self.active {
            return ScreenEyedropperUpdate::None;
        }

        ctx.request_repaint_after(UPDATE_INTERVAL);

        if let Some(session) = &mut self.session {
            session.refresh_cursor();
        }

        let escape_down = screen_eyedropper::escape_down();
        let global_escape_pressed = rising_edge(escape_down, &mut self.last_escape_down);
        let egui_escape_pressed = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let escape_pressed = global_escape_pressed || egui_escape_pressed;
        if escape_pressed {
            self.cancel();
            return ScreenEyedropperUpdate::Cancelled;
        }

        let clicked = self
            .session
            .as_ref()
            .is_some_and(GlobalEyedropperSession::pick_requested);

        let Some(sample) = screen_eyedropper::current_screen_sample() else {
            self.preview_color = None;
            if let Some(overlay) = &mut self.overlay {
                overlay.hide();
            }
            return ScreenEyedropperUpdate::None;
        };

        self.preview_color = sample.color.map(rgb_only);
        if let Some(overlay) = &mut self.overlay {
            overlay.update(sample.x, sample.y, self.preview_color);
        }

        if clicked && let Some(color) = self.preview_color {
            let rgb = color32_to_rgb(color);
            self.cancel();
            return ScreenEyedropperUpdate::Apply(rgb);
        }

        ScreenEyedropperUpdate::None
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.hide();
        }
        self.active = false;
        self.start_requested = false;
        self.last_escape_down = false;
        self.preview_color = None;
        self.overlay = None;
        self.session = None;
    }
}

impl Drop for ScreenEyedropperRuntime {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn rising_edge(current: bool, previous: &mut bool) -> bool {
    let pressed = current && !*previous;
    *previous = current;
    pressed
}

fn rgb_only(color: Color32) -> Color32 {
    Color32::from_rgb(color.r(), color.g(), color.b())
}

fn color32_to_rgb(color: Color32) -> [f32; 3] {
    [
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rising_edge_requires_release_before_next_press() {
        let mut previous = true;
        assert!(!rising_edge(true, &mut previous));
        assert!(!rising_edge(false, &mut previous));
        assert!(rising_edge(true, &mut previous));
    }

    #[test]
    fn color32_is_converted_to_normalized_rgb() {
        assert_eq!(
            color32_to_rgb(Color32::from_rgb(255, 128, 0)),
            [1.0, 128.0 / 255.0, 0.0]
        );
    }
}
