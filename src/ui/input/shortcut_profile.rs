use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use eframe::egui;
use serde::{Deserialize, Serialize};

const BUILTIN_SHORTCUT_PROFILE_RESOURCE: &str = "shortcuts/default.shortcut_profile.ron";
const SHORTCUT_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutProfile {
    schema_version: u32,
    pub(crate) id: String,
    pub(crate) display_name: String,
    #[serde(default = "default_tool_tap_timeout_ms")]
    pub(crate) tool_tap_timeout_ms: u32,
    pub(crate) bindings: Vec<ShortcutBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ShortcutBinding {
    Invoke {
        chord: ShortcutChord,
        action: ShortcutAction,
    },
    PressOrHoldTool {
        key: ShortcutKey,
        target: ToolShortcutTarget,
    },
    HoldOverride {
        modifiers: ModifierMatch,
        target: TransientOverrideTarget,
    },
    PointerGesture {
        context: PointerContext,
        trigger: PointerGestureTrigger,
        action: PointerGestureAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutDiagnostic {
    pub(crate) binding_index: usize,
    pub(crate) reason: ShortcutDiagnosticReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutDiagnosticReason {
    Conflict { conflicts_with: usize },
    Duplicate { duplicates: usize },
    InvalidActionContext,
    EmptyModifiers,
    EmptyTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ShortcutChord {
    pub(crate) key: ShortcutKey,
    #[serde(default)]
    pub(crate) ctrl: bool,
    #[serde(default)]
    pub(crate) shift: bool,
    #[serde(default)]
    pub(crate) alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ShortcutKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Minus,
    Equals,
    Comma,
    Period,
    LeftBracket,
    RightBracket,
    Slash,
    Backslash,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub(crate) struct ModifierMatch {
    #[serde(default)]
    pub(crate) ctrl: bool,
    #[serde(default)]
    pub(crate) shift: bool,
    #[serde(default)]
    pub(crate) alt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ShortcutAction {
    OpenProject,
    SaveProject,
    SaveProjectAs,
    Undo,
    Redo,
    CutImage,
    CopyImage,
    PasteImage,
    SelectAll,
    InvertSelection,
    Deselect,
    DeleteSelectedPixels,
    BeginTransformMode,
    ApplyActiveOperation,
    CancelActiveOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ToolShortcutTarget {
    ToolGroup(String),
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TransientOverrideTarget {
    ActiveToolOverride(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum PointerContext {
    Viewport3d,
    Uv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ShortcutPointerButton {
    Primary,
    Middle,
    Secondary,
    Extra1,
    Extra2,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PointerGestureTrigger {
    pub(crate) button: ShortcutPointerButton,
    #[serde(default)]
    pub(crate) modifiers: ModifierMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointerGestureAction {
    Orbit3d,
    Pan3d,
    Zoom3d,
    RotateUv,
    PanUv,
    ZoomUv,
    BrushSize,
}

fn default_tool_tap_timeout_ms() -> u32 {
    250
}

impl ShortcutProfile {
    pub(crate) fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .with_context(|| format!("reading shortcut profile {}", path.display()))?;
        Self::parse(&source, &path.display().to_string())
    }

    fn parse(source: &str, source_name: &str) -> Result<Self> {
        let profile: Self = ron::from_str(source)
            .with_context(|| format!("parsing shortcut profile {source_name}"))?;
        profile
            .validate()
            .with_context(|| format!("validating shortcut profile {source_name}"))?;
        Ok(profile)
    }

    pub(crate) fn load_default() -> Result<Self> {
        let source = crate::embedded_resources::text(BUILTIN_SHORTCUT_PROFILE_RESOURCE)
            .map_err(anyhow::Error::msg)?;
        Self::parse(
            source,
            &format!("embedded resource {BUILTIN_SHORTCUT_PROFILE_RESOURCE:?}"),
        )
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SHORTCUT_PROFILE_SCHEMA_VERSION,
            "unsupported shortcut profile schema_version {}",
            self.schema_version
        );
        ensure!(
            !self.id.trim().is_empty(),
            "shortcut profile id must not be empty"
        );
        ensure!(
            !self.display_name.trim().is_empty(),
            "shortcut profile display_name must not be empty"
        );
        ensure!(
            (50..=2000).contains(&self.tool_tap_timeout_ms),
            "tool_tap_timeout_ms must be between 50 and 2000"
        );

        Ok(())
    }

    pub(crate) fn diagnostics(&self) -> Vec<ShortcutDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut active_indices = Vec::new();
        for (binding_index, binding) in self.bindings.iter().enumerate() {
            if let Some(reason) = binding_invalid_reason(binding) {
                diagnostics.push(ShortcutDiagnostic {
                    binding_index,
                    reason,
                });
                continue;
            }
            if let Some(conflicts_with) = active_indices
                .iter()
                .copied()
                .find(|active_index| bindings_share_trigger(&self.bindings[*active_index], binding))
            {
                let reason = if same_binding_identity(&self.bindings[conflicts_with], binding) {
                    ShortcutDiagnosticReason::Duplicate {
                        duplicates: conflicts_with,
                    }
                } else {
                    ShortcutDiagnosticReason::Conflict { conflicts_with }
                };
                diagnostics.push(ShortcutDiagnostic {
                    binding_index,
                    reason,
                });
            } else {
                active_indices.push(binding_index);
            }
        }
        diagnostics
    }

    pub(crate) fn resolved_bindings(&self) -> Vec<(usize, &ShortcutBinding)> {
        let diagnostics = self.diagnostics();
        self.bindings
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.binding_index != *index)
            })
            .collect()
    }

    pub(crate) fn pointer_action(
        &self,
        context: PointerContext,
        button: ShortcutPointerButton,
        modifiers: egui::Modifiers,
    ) -> Option<PointerGestureAction> {
        self.resolved_bindings()
            .into_iter()
            .find_map(|(_, binding)| match binding {
                ShortcutBinding::PointerGesture {
                    context: candidate_context,
                    trigger,
                    action,
                } if *candidate_context == context
                    && trigger.button == button
                    && trigger.modifiers.matches_exact(modifiers) =>
                {
                    Some(*action)
                }
                _ => None,
            })
    }

    pub(crate) fn pointer_triggers_for_action(
        &self,
        context: PointerContext,
        wanted: PointerGestureAction,
    ) -> Vec<&PointerGestureTrigger> {
        self.resolved_bindings()
            .into_iter()
            .filter_map(|(_, binding)| match binding {
                ShortcutBinding::PointerGesture {
                    context: candidate_context,
                    trigger,
                    action,
                } if *candidate_context == context && *action == wanted => Some(trigger),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn first_chord_for_action(&self, wanted: ShortcutAction) -> Option<ShortcutChord> {
        self.resolved_bindings()
            .into_iter()
            .find_map(|(_, binding)| match binding {
                ShortcutBinding::Invoke { chord, action } if *action == wanted => Some(*chord),
                _ => None,
            })
    }

    pub(crate) fn first_key_for_tool_group(&self, wanted: &str) -> Option<ShortcutKey> {
        self.resolved_bindings()
            .into_iter()
            .find_map(|(_, binding)| match binding {
                ShortcutBinding::PressOrHoldTool {
                    key,
                    target: ToolShortcutTarget::ToolGroup(id),
                } if id == wanted => Some(*key),
                _ => None,
            })
    }
}

fn binding_invalid_reason(binding: &ShortcutBinding) -> Option<ShortcutDiagnosticReason> {
    match binding {
        ShortcutBinding::PressOrHoldTool { target, .. } if target.id().trim().is_empty() => {
            Some(ShortcutDiagnosticReason::EmptyTarget)
        }
        ShortcutBinding::HoldOverride { modifiers, .. } if modifiers.is_empty() => {
            Some(ShortcutDiagnosticReason::EmptyModifiers)
        }
        ShortcutBinding::HoldOverride { target, .. } if target.id().trim().is_empty() => {
            Some(ShortcutDiagnosticReason::EmptyTarget)
        }
        ShortcutBinding::PointerGesture {
            context, action, ..
        } if !action.valid_for(*context) => Some(ShortcutDiagnosticReason::InvalidActionContext),
        _ => None,
    }
}

pub(crate) fn bindings_share_trigger(left: &ShortcutBinding, right: &ShortcutBinding) -> bool {
    match (left, right) {
        (
            ShortcutBinding::Invoke { chord: left, .. },
            ShortcutBinding::Invoke { chord: right, .. },
        ) => left == right,
        (
            ShortcutBinding::PressOrHoldTool { key: left, .. },
            ShortcutBinding::PressOrHoldTool { key: right, .. },
        ) => left == right,
        (ShortcutBinding::Invoke { chord, .. }, ShortcutBinding::PressOrHoldTool { key, .. })
        | (ShortcutBinding::PressOrHoldTool { key, .. }, ShortcutBinding::Invoke { chord, .. }) => {
            *chord == ShortcutChord::unmodified(*key)
        }
        (
            ShortcutBinding::HoldOverride {
                modifiers: left, ..
            },
            ShortcutBinding::HoldOverride {
                modifiers: right, ..
            },
        ) => left == right,
        (
            ShortcutBinding::PointerGesture {
                context: left_context,
                trigger: left_trigger,
                ..
            },
            ShortcutBinding::PointerGesture {
                context: right_context,
                trigger: right_trigger,
                ..
            },
        ) => left_context == right_context && left_trigger == right_trigger,
        _ => false,
    }
}

fn same_binding_identity(left: &ShortcutBinding, right: &ShortcutBinding) -> bool {
    match (left, right) {
        (
            ShortcutBinding::Invoke { action: left, .. },
            ShortcutBinding::Invoke { action: right, .. },
        ) => left == right,
        (
            ShortcutBinding::PressOrHoldTool { target: left, .. },
            ShortcutBinding::PressOrHoldTool { target: right, .. },
        ) => left == right,
        (
            ShortcutBinding::HoldOverride { target: left, .. },
            ShortcutBinding::HoldOverride { target: right, .. },
        ) => left == right,
        (
            ShortcutBinding::PointerGesture {
                context: left_context,
                action: left_action,
                ..
            },
            ShortcutBinding::PointerGesture {
                context: right_context,
                action: right_action,
                ..
            },
        ) => left_context == right_context && left_action == right_action,
        _ => false,
    }
}

impl ShortcutChord {
    pub(crate) const fn unmodified(key: ShortcutKey) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    pub(crate) fn display(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(self.key.label());
        parts.join("+")
    }
}

impl ShortcutKey {
    pub(crate) const ALL: [Self; 70] = [
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::N,
        Self::O,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::U,
        Self::V,
        Self::W,
        Self::X,
        Self::Y,
        Self::Z,
        Self::Num0,
        Self::Num1,
        Self::Num2,
        Self::Num3,
        Self::Num4,
        Self::Num5,
        Self::Num6,
        Self::Num7,
        Self::Num8,
        Self::Num9,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::Space,
        Self::Enter,
        Self::Escape,
        Self::Tab,
        Self::Backspace,
        Self::Delete,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::Home,
        Self::End,
        Self::PageUp,
        Self::PageDown,
        Self::Minus,
        Self::Equals,
        Self::Comma,
        Self::Period,
        Self::LeftBracket,
        Self::RightBracket,
        Self::Slash,
        Self::Backslash,
    ];

    pub(crate) fn egui_key(self) -> egui::Key {
        use egui::Key;
        match self {
            Self::A => Key::A,
            Self::B => Key::B,
            Self::C => Key::C,
            Self::D => Key::D,
            Self::E => Key::E,
            Self::F => Key::F,
            Self::G => Key::G,
            Self::H => Key::H,
            Self::I => Key::I,
            Self::J => Key::J,
            Self::K => Key::K,
            Self::L => Key::L,
            Self::M => Key::M,
            Self::N => Key::N,
            Self::O => Key::O,
            Self::P => Key::P,
            Self::Q => Key::Q,
            Self::R => Key::R,
            Self::S => Key::S,
            Self::T => Key::T,
            Self::U => Key::U,
            Self::V => Key::V,
            Self::W => Key::W,
            Self::X => Key::X,
            Self::Y => Key::Y,
            Self::Z => Key::Z,
            Self::Num0 => Key::Num0,
            Self::Num1 => Key::Num1,
            Self::Num2 => Key::Num2,
            Self::Num3 => Key::Num3,
            Self::Num4 => Key::Num4,
            Self::Num5 => Key::Num5,
            Self::Num6 => Key::Num6,
            Self::Num7 => Key::Num7,
            Self::Num8 => Key::Num8,
            Self::Num9 => Key::Num9,
            Self::F1 => Key::F1,
            Self::F2 => Key::F2,
            Self::F3 => Key::F3,
            Self::F4 => Key::F4,
            Self::F5 => Key::F5,
            Self::F6 => Key::F6,
            Self::F7 => Key::F7,
            Self::F8 => Key::F8,
            Self::F9 => Key::F9,
            Self::F10 => Key::F10,
            Self::F11 => Key::F11,
            Self::F12 => Key::F12,
            Self::Space => Key::Space,
            Self::Enter => Key::Enter,
            Self::Escape => Key::Escape,
            Self::Tab => Key::Tab,
            Self::Backspace => Key::Backspace,
            Self::Delete => Key::Delete,
            Self::ArrowUp => Key::ArrowUp,
            Self::ArrowDown => Key::ArrowDown,
            Self::ArrowLeft => Key::ArrowLeft,
            Self::ArrowRight => Key::ArrowRight,
            Self::Home => Key::Home,
            Self::End => Key::End,
            Self::PageUp => Key::PageUp,
            Self::PageDown => Key::PageDown,
            Self::Minus => Key::Minus,
            Self::Equals => Key::Equals,
            Self::Comma => Key::Comma,
            Self::Period => Key::Period,
            Self::LeftBracket => Key::OpenBracket,
            Self::RightBracket => Key::CloseBracket,
            Self::Slash => Key::Slash,
            Self::Backslash => Key::Backslash,
        }
    }

    pub(crate) fn from_egui(key: egui::Key) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.egui_key() == key)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::Num0 => "0",
            Self::Num1 => "1",
            Self::Num2 => "2",
            Self::Num3 => "3",
            Self::Num4 => "4",
            Self::Num5 => "5",
            Self::Num6 => "6",
            Self::Num7 => "7",
            Self::Num8 => "8",
            Self::Num9 => "9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::Space => "Space",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::ArrowUp => "Arrow Up",
            Self::ArrowDown => "Arrow Down",
            Self::ArrowLeft => "Arrow Left",
            Self::ArrowRight => "Arrow Right",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "Page Up",
            Self::PageDown => "Page Down",
            Self::Minus => "-",
            Self::Equals => "=",
            Self::Comma => ",",
            Self::Period => ".",
            Self::LeftBracket => "[",
            Self::RightBracket => "]",
            Self::Slash => "/",
            Self::Backslash => "\\",
        }
    }
}

impl ModifierMatch {
    pub(crate) fn from_egui(modifiers: egui::Modifiers) -> Self {
        Self {
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            alt: modifiers.alt,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.ctrl && !self.shift && !self.alt
    }

    pub(crate) fn matches_exact(&self, modifiers: egui::Modifiers) -> bool {
        self.ctrl == modifiers.ctrl
            && self.shift == modifiers.shift
            && self.alt == modifiers.alt
            && (!cfg!(target_os = "macos") || !modifiers.command)
    }
}

impl ToolShortcutTarget {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::ToolGroup(id) | Self::Tool(id) => id,
        }
    }
}

impl TransientOverrideTarget {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::ActiveToolOverride(id) => id,
        }
    }
}

impl PointerGestureAction {
    pub(crate) fn valid_for(self, context: PointerContext) -> bool {
        matches!(
            (context, self),
            (
                PointerContext::Viewport3d,
                Self::Orbit3d | Self::Pan3d | Self::Zoom3d | Self::BrushSize
            ) | (
                PointerContext::Uv,
                Self::RotateUv | Self::PanUv | Self::ZoomUv | Self::BrushSize
            )
        )
    }
}

impl ShortcutPointerButton {
    pub(crate) const ALL: [Self; 5] = [
        Self::Primary,
        Self::Middle,
        Self::Secondary,
        Self::Extra1,
        Self::Extra2,
    ];
    #[cfg(test)]
    fn label(self) -> &'static str {
        match self {
            Self::Primary => "Left Mouse",
            Self::Middle => "Middle Mouse",
            Self::Secondary => "Right Mouse",
            Self::Extra1 => "Mouse 4",
            Self::Extra2 => "Mouse 5",
        }
    }
}

impl PointerGestureTrigger {
    #[cfg(test)]
    fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if self.modifiers.shift {
            parts.push("Shift");
        }
        if self.modifiers.alt {
            parts.push("Alt");
        }
        parts.push(self.button.label());
        parts.join(" + ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_valid_and_covers_navigation() {
        let profile = ShortcutProfile::load_default().expect("default shortcut profile");
        assert!(profile.diagnostics().is_empty());
        assert!(profile.bindings.iter().any(|binding| matches!(
            binding,
            ShortcutBinding::PointerGesture {
                context: PointerContext::Viewport3d,
                action: PointerGestureAction::Orbit3d,
                ..
            }
        )));
        let serialized = ron::ser::to_string(&profile).expect("serialize shortcut profile");
        let roundtrip: ShortcutProfile =
            ron::from_str(&serialized).expect("parse serialized profile");
        assert_eq!(roundtrip, profile);
        assert!(profile.bindings.iter().any(|binding| matches!(
            binding,
            ShortcutBinding::Invoke {
                action: ShortcutAction::ApplyActiveOperation,
                ..
            }
        )));
        assert_eq!(
            profile.first_chord_for_action(ShortcutAction::SelectAll),
            Some(ShortcutChord {
                key: ShortcutKey::A,
                ctrl: true,
                shift: false,
                alt: false,
            })
        );
        assert_eq!(
            profile.first_chord_for_action(ShortcutAction::InvertSelection),
            Some(ShortcutChord {
                key: ShortcutKey::I,
                ctrl: true,
                shift: false,
                alt: false,
            })
        );
    }

    #[test]
    fn invalid_pointer_action_is_an_inactive_diagnostic() {
        let source = r#"(
            schema_version: 1, id: "test", display_name: "Test",
            bindings: [PointerGesture(context: Viewport3d, trigger: (button: Primary), action: RotateUv)],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();
        assert!(profile.validate().is_ok());
        assert_eq!(
            profile.diagnostics(),
            vec![ShortcutDiagnostic {
                binding_index: 0,
                reason: ShortcutDiagnosticReason::InvalidActionContext,
            }]
        );
        assert!(profile.resolved_bindings().is_empty());
    }

    #[test]
    fn hold_modifier_combination_matches_exactly() {
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let alt_ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            alt: true,
            ..Default::default()
        };
        let condition = ModifierMatch {
            ctrl: true,
            ..Default::default()
        };
        assert!(condition.matches_exact(ctrl));
        assert!(!condition.matches_exact(alt_ctrl));
    }

    #[test]
    fn empty_hold_override_is_an_inactive_diagnostic() {
        let source = r#"(
            schema_version: 1, id: "test", display_name: "Test",
            bindings: [HoldOverride(
                modifiers: (),
                target: ActiveToolOverride("erase"),
            )],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();
        assert!(profile.validate().is_ok());
        assert_eq!(
            profile.diagnostics(),
            vec![ShortcutDiagnostic {
                binding_index: 0,
                reason: ShortcutDiagnosticReason::EmptyModifiers,
            }]
        );
    }

    #[test]
    fn later_conflict_is_inactive_and_profile_remains_valid() {
        let source = r#"(
            schema_version: 1, id: "test", display_name: "Test",
            bindings: [
                Invoke(chord: (key: S, ctrl: true), action: SaveProject),
                Invoke(chord: (key: S, ctrl: true), action: Undo),
            ],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();
        assert!(profile.validate().is_ok());
        assert_eq!(
            profile.diagnostics(),
            vec![ShortcutDiagnostic {
                binding_index: 1,
                reason: ShortcutDiagnosticReason::Conflict { conflicts_with: 0 },
            }]
        );
        assert_eq!(profile.resolved_bindings().len(), 1);
    }

    #[test]
    fn later_same_action_binding_is_a_duplicate_diagnostic() {
        let source = r#"(
            schema_version: 1, id: "test", display_name: "Test",
            bindings: [
                Invoke(chord: (key: S, ctrl: true), action: SaveProject),
                Invoke(chord: (key: S, ctrl: true), action: SaveProject),
            ],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();
        assert_eq!(
            profile.diagnostics(),
            vec![ShortcutDiagnostic {
                binding_index: 1,
                reason: ShortcutDiagnosticReason::Duplicate { duplicates: 0 },
            }]
        );
    }

    #[test]
    fn invalid_binding_does_not_block_a_later_valid_trigger() {
        let source = r#"(
            schema_version: 1, id: "test", display_name: "Test",
            bindings: [
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Primary, modifiers: (alt: true)),
                    action: RotateUv,
                ),
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Primary, modifiers: (alt: true)),
                    action: Orbit3d,
                ),
            ],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();

        assert_eq!(profile.resolved_bindings()[0].0, 1);
    }

    #[test]
    fn pointer_triggers_for_action_returns_only_resolved_matches_in_profile_order() {
        let source = r#"(
            schema_version: 1, id: "test", display_name: "Test",
            bindings: [
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Primary, modifiers: (alt: true)),
                    action: Orbit3d,
                ),
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Extra1, modifiers: (ctrl: true)),
                    action: Orbit3d,
                ),
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Primary, modifiers: (alt: true)),
                    action: Pan3d,
                ),
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Extra2),
                    action: RotateUv,
                ),
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Extra1, modifiers: (ctrl: true)),
                    action: Orbit3d,
                ),
                PointerGesture(
                    context: Viewport3d,
                    trigger: (button: Extra2, modifiers: (shift: true)),
                    action: Orbit3d,
                ),
                PointerGesture(
                    context: Uv,
                    trigger: (button: Middle),
                    action: BrushSize,
                ),
            ],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();

        let triggers = profile
            .pointer_triggers_for_action(PointerContext::Viewport3d, PointerGestureAction::Orbit3d);
        assert_eq!(
            triggers
                .iter()
                .map(|trigger| trigger.display())
                .collect::<Vec<_>>(),
            vec!["Alt + Left Mouse", "Ctrl + Mouse 4", "Shift + Mouse 5"]
        );
        assert!(
            profile
                .pointer_triggers_for_action(
                    PointerContext::Viewport3d,
                    PointerGestureAction::BrushSize,
                )
                .is_empty()
        );
        assert_eq!(
            profile
                .pointer_triggers_for_action(PointerContext::Uv, PointerGestureAction::BrushSize,)
                .len(),
            1
        );
    }

    #[test]
    fn unsupported_schema_version_remains_a_hard_error() {
        let source = r#"(
            schema_version: 2, id: "test", display_name: "Test", bindings: [],
        )"#;
        let profile: ShortcutProfile = ron::from_str(source).unwrap();

        assert!(profile.validate().is_err());
    }
}
