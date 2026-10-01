use eframe::egui;

use crate::application::InputModifiers;

pub fn input_modifiers_from_egui(modifiers: egui::Modifiers) -> InputModifiers {
    InputModifiers {
        alt: modifiers.alt,
        ctrl: modifiers.ctrl,
        shift: modifiers.shift,
        command: modifiers.command,
    }
}
