use crate::core::tool::ToolId;

use super::{AppState, Command, paint_edit::PaintTargetScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    Undo,
    Redo,
    SelectAll,
    InvertSelection,
    Deselect,
    DeleteSelectedPixels,
    BeginTransformMode,
    ActivateTool(ToolId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorActionBlockReason {
    RenderCommitPending,
    FatalRendererError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorActionContext {
    pub has_document: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_deselect: bool,
    pub can_delete_selected_pixels: bool,
    pub tool_interacting: bool,
    pub decal_session_active: bool,
    pub edit_block_reason: Option<EditorActionBlockReason>,
    pub keyboard_input_owned_by_ui: bool,
}

impl EditorActionContext {
    pub(crate) fn new(
        state: &AppState,
        edit_block_reason: Option<EditorActionBlockReason>,
        keyboard_owned: bool,
    ) -> Self {
        Self {
            has_document: state.document().is_some(),
            can_undo: state.can_undo(),
            can_redo: state.can_redo(),
            can_deselect: state.has_active_selection(),
            can_delete_selected_pixels: state
                .active_selection()
                .is_some_and(|selection| selection.is_active())
                && state
                    .document
                    .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                    .is_allowed(),
            tool_interacting: state.is_document_edit_interacting(),
            decal_session_active: state.has_active_decal_session(),
            edit_block_reason,
            keyboard_input_owned_by_ui: keyboard_owned,
        }
    }

    pub fn is_enabled(self, action: EditorAction) -> bool {
        if self.editing_blocked() {
            return false;
        }
        match action {
            EditorAction::Undo => self.can_undo && !self.tool_interacting,
            EditorAction::Redo => self.can_redo && !self.tool_interacting,
            EditorAction::SelectAll | EditorAction::InvertSelection => {
                self.has_document && !self.tool_interacting
            }
            EditorAction::Deselect => {
                self.has_document && self.can_deselect && !self.tool_interacting
            }
            EditorAction::DeleteSelectedPixels => {
                self.has_document && self.can_delete_selected_pixels && !self.tool_interacting
            }
            EditorAction::BeginTransformMode => self.has_document && !self.tool_interacting,
            EditorAction::ActivateTool(_) => !self.tool_interacting || self.decal_session_active,
        }
    }

    pub fn shortcut_is_enabled(self, action: EditorAction) -> bool {
        !self.keyboard_input_owned_by_ui && self.is_enabled(action)
    }

    pub fn editing_blocked(self) -> bool {
        self.edit_block_reason.is_some()
    }

    pub fn can_save_project(self) -> bool {
        self.has_document && !self.tool_interacting && !self.editing_blocked()
    }

    pub fn can_paste_image(self) -> bool {
        self.has_document && !self.tool_interacting && !self.editing_blocked()
    }

    pub fn can_paste_image_from_shortcut(self) -> bool {
        !self.keyboard_input_owned_by_ui && self.can_paste_image()
    }

    pub fn can_cut_image(self, has_cuttable_raster_target: bool) -> bool {
        has_cuttable_raster_target && !self.tool_interacting && !self.editing_blocked()
    }

    pub fn can_cut_image_from_shortcut(self, has_cuttable_raster_target: bool) -> bool {
        !self.keyboard_input_owned_by_ui && self.can_cut_image(has_cuttable_raster_target)
    }

    pub fn can_copy_image(self, has_single_raster_target: bool) -> bool {
        has_single_raster_target && !self.tool_interacting && !self.editing_blocked()
    }

    pub fn can_copy_image_from_shortcut(self, has_single_raster_target: bool) -> bool {
        !self.keyboard_input_owned_by_ui && self.can_copy_image(has_single_raster_target)
    }
}

impl EditorAction {
    pub(crate) fn command(self) -> Command {
        match self {
            Self::Undo => Command::Undo,
            Self::Redo => Command::Redo,
            Self::SelectAll => Command::SelectAll,
            Self::InvertSelection => Command::InvertSelection,
            Self::Deselect => Command::DeselectSelection,
            Self::DeleteSelectedPixels => Command::DeleteSelectedPixels,
            Self::BeginTransformMode => Command::BeginTransformMode,
            Self::ActivateTool(tool_id) => Command::SetActiveTool(tool_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_save_requires_an_idle_unblocked_document() {
        let mut context = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };
        assert!(context.can_save_project());

        context.tool_interacting = true;
        assert!(!context.can_save_project());
        context.tool_interacting = false;
        context.edit_block_reason = Some(EditorActionBlockReason::RenderCommitPending);
        assert!(!context.can_save_project());
        context.edit_block_reason = None;
        context.has_document = false;
        assert!(!context.can_save_project());
    }

    #[test]
    fn pending_render_disables_edit_actions() {
        let context = EditorActionContext {
            has_document: true,
            can_undo: true,
            can_redo: true,
            can_deselect: true,
            can_delete_selected_pixels: true,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: Some(EditorActionBlockReason::RenderCommitPending),
            keyboard_input_owned_by_ui: false,
        };
        assert!(!context.is_enabled(EditorAction::Undo));
        assert!(!context.is_enabled(EditorAction::Deselect));
    }

    #[test]
    fn selection_actions_require_an_idle_document_and_map_to_commands() {
        let available = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };
        assert!(available.shortcut_is_enabled(EditorAction::SelectAll));
        assert!(available.shortcut_is_enabled(EditorAction::InvertSelection));
        assert!(matches!(
            EditorAction::SelectAll.command(),
            Command::SelectAll
        ));
        assert!(matches!(
            EditorAction::InvertSelection.command(),
            Command::InvertSelection
        ));
        assert!(
            !EditorActionContext {
                tool_interacting: true,
                ..available
            }
            .is_enabled(EditorAction::SelectAll)
        );
        assert!(
            !EditorActionContext {
                has_document: false,
                ..available
            }
            .is_enabled(EditorAction::InvertSelection)
        );
    }

    #[test]
    fn fatal_renderer_error_disables_edit_actions_with_a_distinct_reason() {
        let context = EditorActionContext {
            has_document: true,
            can_undo: true,
            can_redo: true,
            can_deselect: true,
            can_delete_selected_pixels: true,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: Some(EditorActionBlockReason::FatalRendererError),
            keyboard_input_owned_by_ui: false,
        };

        assert!(context.editing_blocked());
        assert!(!context.is_enabled(EditorAction::Undo));
        assert_eq!(
            context.edit_block_reason,
            Some(EditorActionBlockReason::FatalRendererError)
        );
    }

    #[test]
    fn keyboard_owner_blocks_shortcuts_but_not_action_availability() {
        let context = EditorActionContext {
            has_document: true,
            can_undo: true,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: true,
        };
        assert!(context.is_enabled(EditorAction::Undo));
        assert!(!context.shortcut_is_enabled(EditorAction::Undo));
    }

    #[test]
    fn tool_activation_is_blocked_during_an_operation() {
        let context = EditorActionContext {
            has_document: false,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: true,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };

        assert!(!context.is_enabled(EditorAction::ActivateTool(ToolId::BrushPreset(0))));
    }

    #[test]
    fn tool_activation_remains_available_for_an_active_decal_session() {
        let context = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: true,
            decal_session_active: true,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };

        assert!(context.is_enabled(EditorAction::ActivateTool(ToolId::RectangleSelection)));
        assert!(!context.is_enabled(EditorAction::Undo));
    }

    #[test]
    fn delete_selected_pixels_requires_an_editable_selection_and_unowned_keyboard() {
        let available = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: true,
            can_delete_selected_pixels: true,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };
        assert!(available.is_enabled(EditorAction::DeleteSelectedPixels));
        assert!(available.shortcut_is_enabled(EditorAction::DeleteSelectedPixels));
        assert!(
            !EditorActionContext {
                keyboard_input_owned_by_ui: true,
                ..available
            }
            .shortcut_is_enabled(EditorAction::DeleteSelectedPixels)
        );
        assert!(
            !EditorActionContext {
                can_delete_selected_pixels: false,
                ..available
            }
            .is_enabled(EditorAction::DeleteSelectedPixels)
        );
    }

    #[test]
    fn image_paste_shortcut_respects_editing_and_keyboard_ownership() {
        let available = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };
        assert!(available.can_paste_image());
        assert!(available.can_paste_image_from_shortcut());

        assert!(
            !EditorActionContext {
                keyboard_input_owned_by_ui: true,
                ..available
            }
            .can_paste_image_from_shortcut()
        );
        assert!(
            !EditorActionContext {
                tool_interacting: true,
                ..available
            }
            .can_paste_image()
        );
        assert!(
            !EditorActionContext {
                edit_block_reason: Some(EditorActionBlockReason::RenderCommitPending),
                ..available
            }
            .can_paste_image()
        );
        assert!(
            !EditorActionContext {
                has_document: false,
                ..available
            }
            .can_paste_image()
        );
    }

    #[test]
    fn image_cut_requires_cuttable_target_and_unowned_keyboard() {
        let available = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };
        assert!(available.can_cut_image(true));
        assert!(!available.can_cut_image(false));
        assert!(
            !EditorActionContext {
                keyboard_input_owned_by_ui: true,
                ..available
            }
            .can_cut_image_from_shortcut(true)
        );
        assert!(
            !EditorActionContext {
                tool_interacting: true,
                ..available
            }
            .can_cut_image(true)
        );
        assert!(
            !EditorActionContext {
                edit_block_reason: Some(EditorActionBlockReason::RenderCommitPending),
                ..available
            }
            .can_cut_image(true)
        );
    }

    #[test]
    fn image_copy_requires_single_target_and_unowned_keyboard() {
        let available = EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        };
        assert!(available.can_copy_image(true));
        assert!(!available.can_copy_image(false));
        assert!(
            !EditorActionContext {
                keyboard_input_owned_by_ui: true,
                ..available
            }
            .can_copy_image_from_shortcut(true)
        );
    }
}
