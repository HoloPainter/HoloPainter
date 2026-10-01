use std::collections::{HashMap, HashSet};

use eframe::egui;

use crate::{
    application::{AppState, Command, EditorAction, EditorActionContext, InputHoldToken},
    ui::input::shortcut_profile::{
        ShortcutAction, ShortcutBinding, ShortcutChord, ShortcutKey, ShortcutProfile,
        ToolShortcutTarget, TransientOverrideTarget,
    },
};

#[derive(Debug)]
pub(crate) struct ShortcutInputRuntime {
    profile: ShortcutProfile,
    default_profile: ShortcutProfile,
    profile_revision: u64,
    next_token: u64,
    active_invoke_keys: HashSet<ShortcutKey>,
    active_tool_keys: HashMap<ShortcutKey, ToolKeyGesture>,
    active_modifier_overrides:
        HashMap<crate::ui::input::shortcut_profile::ModifierMatch, InputHoldToken>,
}

#[derive(Debug)]
struct ToolKeyGesture {
    token: InputHoldToken,
    tool_id: crate::core::tool::ToolId,
    started_at_s: f64,
    used_by_pointer: bool,
    released_at_s: Option<f64>,
    cancelled: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ShortcutFrameStart {
    pub(crate) actions: Vec<EditorAction>,
    pub(crate) commands: Vec<Command>,
    pub(crate) open_requested: bool,
    pub(crate) save_requested: bool,
    pub(crate) save_as_requested: bool,
    pub(crate) cut_requested: bool,
    pub(crate) copy_requested: bool,
    pub(crate) paste_requested: bool,
    pub(crate) apply_requested: bool,
    pub(crate) cancel_requested: bool,
    pub(crate) needs_repaint: bool,
}

impl ShortcutInputRuntime {
    pub(crate) fn load() -> anyhow::Result<Self> {
        let default_profile = ShortcutProfile::load_default()?;
        let profile = match crate::settings::current_user_shortcut_profile_path() {
            Some(path) if path.exists() => ShortcutProfile::load(path)?,
            _ => default_profile.clone(),
        };
        Ok(Self::new(profile, default_profile))
    }

    fn new(profile: ShortcutProfile, default_profile: ShortcutProfile) -> Self {
        Self {
            profile,
            default_profile,
            profile_revision: 1,
            next_token: 1,
            active_invoke_keys: HashSet::new(),
            active_tool_keys: HashMap::new(),
            active_modifier_overrides: HashMap::new(),
        }
    }

    pub(crate) fn profile(&self) -> &ShortcutProfile {
        &self.profile
    }

    pub(crate) fn default_profile(&self) -> &ShortcutProfile {
        &self.default_profile
    }

    pub(crate) fn profile_revision(&self) -> u64 {
        self.profile_revision
    }

    pub(crate) fn replace_profile(&mut self, profile: ShortcutProfile) -> anyhow::Result<()> {
        profile.validate()?;
        self.profile = profile;
        self.profile_revision = self.profile_revision.wrapping_add(1).max(1);
        Ok(())
    }

    pub(crate) fn begin_frame(
        &mut self,
        ctx: &egui::Context,
        state: &AppState,
        action_context: EditorActionContext,
    ) -> ShortcutFrameStart {
        let (focused, time_s, modifiers) =
            ctx.input(|input| (input.focused, input.time, input.modifiers));
        let mut output = ShortcutFrameStart::default();

        self.active_invoke_keys
            .retain(|key| focused && ctx.input(|input| input.key_down(key.egui_key())));

        for (_, binding) in self.profile.resolved_bindings() {
            let ShortcutBinding::HoldOverride {
                modifiers: condition,
                target,
            } = binding
            else {
                continue;
            };
            let is_down = focused && condition.matches_exact(modifiers);
            let trigger = condition.clone();
            let active = self.active_modifier_overrides.get(&trigger).copied();
            match (is_down, active) {
                (true, None)
                    if !action_context.keyboard_input_owned_by_ui
                        && !action_context.editing_blocked() =>
                {
                    let token = next_token(&mut self.next_token);
                    self.active_modifier_overrides
                        .insert(trigger.clone(), token);
                    let TransientOverrideTarget::ActiveToolOverride(id) = target;
                    output.commands.push(Command::BeginTransientToolOverride {
                        token,
                        id: id.clone(),
                    });
                    output.needs_repaint = true;
                }
                (false, Some(token)) => {
                    self.active_modifier_overrides.remove(&trigger);
                    output
                        .commands
                        .push(Command::EndTransientToolOverride { token });
                    output.needs_repaint = true;
                }
                _ => {}
            }
        }

        if !action_context.keyboard_input_owned_by_ui && focused {
            for (_, binding) in self.profile.resolved_bindings() {
                match binding {
                    ShortcutBinding::Invoke { chord, action } => {
                        let pressed = ctx.input(|input| input.key_pressed(chord.key.egui_key()));
                        if pressed
                            && chord_matches(*chord, modifiers)
                            && self.active_invoke_keys.insert(chord.key)
                        {
                            match action {
                                ShortcutAction::OpenProject
                                    if !action_context.tool_interacting
                                        && !action_context.editing_blocked() =>
                                {
                                    output.open_requested = true;
                                }
                                ShortcutAction::SaveProject
                                    if action_context.can_save_project() =>
                                {
                                    output.save_requested = true;
                                }
                                ShortcutAction::SaveProjectAs
                                    if action_context.can_save_project() =>
                                {
                                    output.save_as_requested = true;
                                }
                                ShortcutAction::CutImage => output.cut_requested = true,
                                ShortcutAction::CopyImage => output.copy_requested = true,
                                ShortcutAction::PasteImage => output.paste_requested = true,
                                ShortcutAction::ApplyActiveOperation => {
                                    output.apply_requested = true
                                }
                                ShortcutAction::CancelActiveOperation => {
                                    output.cancel_requested = true
                                }
                                ShortcutAction::OpenProject
                                | ShortcutAction::SaveProject
                                | ShortcutAction::SaveProjectAs => {}
                                action => output.actions.push(editor_action(*action)),
                            }
                            output.needs_repaint = true;
                        }
                    }
                    ShortcutBinding::PressOrHoldTool { key, target } => {
                        let pressed = ctx.input(|input| input.key_pressed(key.egui_key()));
                        if pressed
                            && modifiers_are_empty(modifiers)
                            && !self.active_tool_keys.contains_key(key)
                            && !action_context.editing_blocked()
                            && state.is_tool_idle()
                            && !state.has_active_modal_tool()
                        {
                            let Some(tool_id) = resolve_tool_target(state, target) else {
                                continue;
                            };
                            let token = next_token(&mut self.next_token);
                            self.active_tool_keys.insert(
                                *key,
                                ToolKeyGesture {
                                    token,
                                    tool_id,
                                    started_at_s: time_s,
                                    used_by_pointer: false,
                                    released_at_s: None,
                                    cancelled: false,
                                },
                            );
                            output
                                .commands
                                .push(Command::BeginMomentaryTool { token, tool_id });
                            output.needs_repaint = true;
                        }
                    }
                    ShortcutBinding::HoldOverride { .. }
                    | ShortcutBinding::PointerGesture { .. } => {}
                }
            }
        }

        for (key, gesture) in &mut self.active_tool_keys {
            if gesture.released_at_s.is_some() {
                continue;
            }
            let released = ctx.input(|input| input.key_released(key.egui_key()));
            let is_down = ctx.input(|input| input.key_down(key.egui_key()));
            if released || !focused || !is_down {
                gesture.released_at_s = Some(time_s);
                gesture.cancelled = !focused;
            }
        }

        output
    }

    pub(crate) fn mark_pointer_down(&mut self) {
        for gesture in self.active_tool_keys.values_mut() {
            gesture.used_by_pointer = true;
        }
    }

    pub(crate) fn cancel_active_inputs(&mut self) -> Vec<Command> {
        self.active_invoke_keys.clear();
        let mut commands = Vec::new();
        commands.extend(
            self.active_modifier_overrides
                .drain()
                .map(|(_, token)| Command::EndTransientToolOverride { token }),
        );
        commands.extend(self.active_tool_keys.drain().map(|(_, gesture)| {
            Command::CompleteMomentaryTool {
                token: gesture.token,
                tool_id: gesture.tool_id,
                select_tool: false,
            }
        }));
        commands
    }

    pub(crate) fn finish_frame(&mut self) -> Vec<Command> {
        let timeout_s = f64::from(self.profile.tool_tap_timeout_ms) / 1000.0;
        let completed: Vec<_> = self
            .active_tool_keys
            .iter()
            .filter_map(|(key, gesture)| {
                gesture.released_at_s.map(|released_at_s| {
                    let select_tool = !gesture.cancelled
                        && !gesture.used_by_pointer
                        && released_at_s - gesture.started_at_s <= timeout_s;
                    (
                        *key,
                        Command::CompleteMomentaryTool {
                            token: gesture.token,
                            tool_id: gesture.tool_id,
                            select_tool,
                        },
                    )
                })
            })
            .collect();
        for (key, _) in &completed {
            self.active_tool_keys.remove(key);
        }
        completed.into_iter().map(|(_, command)| command).collect()
    }
}

fn next_token(next_token: &mut u64) -> InputHoldToken {
    let token = InputHoldToken(*next_token);
    *next_token = (*next_token).wrapping_add(1).max(1);
    token
}

fn editor_action(action: ShortcutAction) -> EditorAction {
    match action {
        ShortcutAction::OpenProject
        | ShortcutAction::SaveProject
        | ShortcutAction::SaveProjectAs
        | ShortcutAction::CutImage
        | ShortcutAction::CopyImage
        | ShortcutAction::PasteImage
        | ShortcutAction::ApplyActiveOperation
        | ShortcutAction::CancelActiveOperation => {
            unreachable!("application shortcut is not an editor action")
        }
        ShortcutAction::Undo => EditorAction::Undo,
        ShortcutAction::Redo => EditorAction::Redo,
        ShortcutAction::SelectAll => EditorAction::SelectAll,
        ShortcutAction::InvertSelection => EditorAction::InvertSelection,
        ShortcutAction::Deselect => EditorAction::Deselect,
        ShortcutAction::DeleteSelectedPixels => EditorAction::DeleteSelectedPixels,
        ShortcutAction::BeginTransformMode => EditorAction::BeginTransformMode,
    }
}

fn resolve_tool_target(
    state: &AppState,
    target: &ToolShortcutTarget,
) -> Option<crate::core::tool::ToolId> {
    match target {
        ToolShortcutTarget::ToolGroup(id) => state.representative_tool_id_for_group(id),
        ToolShortcutTarget::Tool(id) => state.tool_id_by_config_id(id),
    }
}

fn chord_matches(chord: ShortcutChord, modifiers: egui::Modifiers) -> bool {
    chord.ctrl == modifiers.ctrl
        && chord.shift == modifiers.shift
        && chord.alt == modifiers.alt
        && (!cfg!(target_os = "macos") || !modifiers.command)
}

fn modifiers_are_empty(modifiers: egui::Modifiers) -> bool {
    !modifiers.alt && !modifiers.ctrl && !modifiers.shift && !modifiers.command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(alt: bool, ctrl: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            alt,
            ctrl,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn chord_matching_is_exact_for_modifiers() {
        let chord = ShortcutChord {
            key: ShortcutKey::S,
            ctrl: true,
            shift: false,
            alt: false,
        };
        let ctrl = modifiers(false, true, false);
        assert!(chord_matches(chord, ctrl));
        let alt_ctrl = modifiers(true, true, false);
        assert!(!chord_matches(chord, alt_ctrl));
    }

    #[test]
    fn selection_shortcuts_map_to_editor_actions() {
        assert_eq!(
            editor_action(ShortcutAction::SelectAll),
            EditorAction::SelectAll
        );
        assert_eq!(
            editor_action(ShortcutAction::InvertSelection),
            EditorAction::InvertSelection
        );
    }
}
