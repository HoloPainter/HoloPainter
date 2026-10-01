use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{application::AppState, core::tool::ToolId};

pub(crate) const EDITOR_SESSION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EditorSessionFileV1 {
    pub(crate) schema_version: u32,
    pub(crate) tool_selection: ToolSelectionStateV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolSelectionStateV1 {
    pub(crate) active_group_id: Option<String>,
    pub(crate) groups: Vec<ToolGroupSelectionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolGroupSelectionV1 {
    pub(crate) group_id: String,
    pub(crate) selected_entry_id: Option<String>,
}

impl EditorSessionFileV1 {
    pub(crate) fn capture(state: &AppState) -> Result<Self, String> {
        let groups = state
            .tool_shelf()
            .groups
            .iter()
            .enumerate()
            .map(|(group_index, group)| {
                let tool_id = state
                    .representative_tool_id(group_index)
                    .ok_or_else(|| format!("tool group {:?} has no representative", group.id))?;
                let selected_entry_id = match tool_id {
                    ToolId::EmptyGroup(_) => None,
                    _ => Some(
                        state
                            .tool_catalog()
                            .iter()
                            .find(|tool| tool.id == tool_id)
                            .ok_or_else(|| {
                                format!(
                                    "tool group {:?} representative {:?} is missing from the catalog",
                                    group.id, tool_id
                                )
                            })?
                            .config_id
                            .clone(),
                    ),
                };
                Ok(ToolGroupSelectionV1 {
                    group_id: group.id.clone(),
                    selected_entry_id,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let active_group_id = state
            .tool_shelf()
            .group_containing(state.active_tool_id())
            .and_then(|index| state.tool_shelf().groups.get(index))
            .map(|group| group.id.clone());
        let file = Self {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id,
                groups,
            },
        };
        file.validate()?;
        Ok(file)
    }

    pub(crate) fn restore_into(&self, state: &mut AppState) {
        state.restore_persistent_tool_selection(
            self.tool_selection.active_group_id.as_deref(),
            self.tool_selection
                .groups
                .iter()
                .map(|group| (group.group_id.as_str(), group.selected_entry_id.as_deref())),
        );
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != EDITOR_SESSION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported editor session schema version {}",
                self.schema_version
            ));
        }
        if self
            .tool_selection
            .active_group_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err("editor session active_group_id must not be empty".to_owned());
        }

        let mut group_ids = HashSet::new();
        for group in &self.tool_selection.groups {
            if group.group_id.trim().is_empty() {
                return Err("editor session group_id must not be empty".to_owned());
            }
            if !group_ids.insert(group.group_id.as_str()) {
                return Err(format!(
                    "editor session contains duplicate group_id {:?}",
                    group.group_id
                ));
            }
            if group
                .selected_entry_id
                .as_deref()
                .is_some_and(|id| id.trim().is_empty())
            {
                return Err(format!(
                    "editor session group {:?} selected_entry_id must not be empty",
                    group.group_id
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        core::{
            tool::ToolId,
            tool_catalog::{load_test_tools_from_fixtures, resolve_default_tools},
            tool_layout::{ToolEntryDefinition, ToolLayoutFileV1},
        },
        settings::UserSettings,
    };

    use super::*;

    fn session() -> EditorSessionFileV1 {
        EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: Some("tool.brush".to_owned()),
                groups: vec![
                    ToolGroupSelectionV1 {
                        group_id: "tool.selection".to_owned(),
                        selected_entry_id: Some("builtin.tool.selection.lasso".to_owned()),
                    },
                    ToolGroupSelectionV1 {
                        group_id: "tool.brush".to_owned(),
                        selected_entry_id: Some("builtin.brush.paint.directional_flat".to_owned()),
                    },
                ],
            },
        }
    }

    fn state_with_layout(layout: ToolLayoutFileV1) -> AppState {
        let defaults = load_test_tools_from_fixtures().expect("test tool fixtures");
        let resolved = resolve_default_tools(
            &defaults.brush_engines,
            &defaults.brush_presets,
            &defaults.brush_textures,
            &layout,
        )
        .expect("resolve test tool layout");
        AppState::with_status_and_configuration("test", UserSettings::default(), resolved)
    }

    fn representative_config_id<'a>(state: &'a AppState, group_id: &str) -> Option<&'a str> {
        let tool_id = state.representative_tool_id_for_group(group_id)?;
        if matches!(tool_id, ToolId::EmptyGroup(_)) {
            return None;
        }
        state
            .tool_catalog()
            .iter()
            .find(|tool| tool.id == tool_id)
            .map(|tool| tool.config_id.as_str())
    }

    fn select(state: &mut AppState, config_id: &str) {
        let tool_id = state
            .tool_id_by_config_id(config_id)
            .unwrap_or_else(|| panic!("missing test tool {config_id:?}"));
        state.set_active_tool(tool_id);
    }

    #[test]
    fn editor_session_roundtrips_through_ron() {
        let expected = session();
        let encoded = ron::ser::to_string(&expected).expect("serialize editor session");
        let decoded: EditorSessionFileV1 =
            ron::from_str(&encoded).expect("deserialize editor session");

        assert_eq!(decoded, expected);
        decoded.validate().expect("roundtripped session validates");
    }

    #[test]
    fn validation_rejects_unsupported_schema_version() {
        let mut file = session();
        file.schema_version = EDITOR_SESSION_SCHEMA_VERSION + 1;

        assert!(file.validate().is_err());
    }

    #[test]
    fn validation_rejects_duplicate_groups() {
        let mut file = session();
        file.tool_selection.groups.push(ToolGroupSelectionV1 {
            group_id: "tool.brush".to_owned(),
            selected_entry_id: None,
        });

        assert!(file.validate().is_err());
    }

    #[test]
    fn validation_rejects_empty_ids() {
        let mut empty_active = session();
        empty_active.tool_selection.active_group_id = Some("  ".to_owned());
        assert!(empty_active.validate().is_err());

        let mut empty_group = session();
        empty_group.tool_selection.groups[0].group_id.clear();
        assert!(empty_group.validate().is_err());

        let mut empty_entry = session();
        empty_entry.tool_selection.groups[0].selected_entry_id = Some(String::new());
        assert!(empty_entry.validate().is_err());
    }

    #[test]
    fn capture_uses_stable_ids_for_every_representative_and_active_group() {
        let mut state = AppState::default();
        select(&mut state, "builtin.tool.selection.lasso");
        select(&mut state, "builtin.tool.fill.mesh");
        select(&mut state, "builtin.brush.paint.directional_flat");

        let captured = EditorSessionFileV1::capture(&state).expect("capture editor session");

        assert_eq!(
            captured.tool_selection.groups.len(),
            state.tool_shelf().groups.len()
        );
        assert_eq!(
            captured.tool_selection.active_group_id.as_deref(),
            Some("tool.brush")
        );
        for (group_index, group) in state.tool_shelf().groups.iter().enumerate() {
            let saved = captured
                .tool_selection
                .groups
                .iter()
                .find(|saved| saved.group_id == group.id)
                .expect("captured group");
            assert_eq!(
                saved.selected_entry_id.as_deref(),
                representative_config_id(&state, &group.id),
                "group index {group_index}"
            );
        }
    }

    #[test]
    fn normal_restore_recovers_each_representative_and_active_group() {
        let mut source = AppState::default();
        select(&mut source, "builtin.tool.selection.lasso");
        select(&mut source, "builtin.tool.fill.mesh");
        select(&mut source, "builtin.brush.paint.directional_flat");
        let captured = EditorSessionFileV1::capture(&source).expect("capture editor session");
        let mut restored = AppState::default();

        captured.restore_into(&mut restored);

        assert_eq!(
            representative_config_id(&restored, "tool.selection"),
            Some("builtin.tool.selection.lasso")
        );
        assert_eq!(
            representative_config_id(&restored, "tool.fill"),
            Some("builtin.tool.fill.mesh")
        );
        assert_eq!(
            representative_config_id(&restored, "tool.brush"),
            Some("builtin.brush.paint.directional_flat")
        );
        assert_eq!(
            restored.active_tool_id(),
            restored
                .representative_tool_id_for_group("tool.brush")
                .expect("brush representative")
        );
    }

    #[test]
    fn missing_entry_keeps_group_fallback_and_activates_it() {
        let mut file = session();
        file.tool_selection.groups[1].selected_entry_id = Some("deleted.brush.preset".to_owned());
        let mut state = AppState::default();
        let fallback = state
            .representative_tool_id_for_group("tool.brush")
            .expect("brush fallback");

        file.restore_into(&mut state);

        assert_eq!(
            state.representative_tool_id_for_group("tool.brush"),
            Some(fallback)
        );
        assert_eq!(state.active_tool_id(), fallback);
    }

    #[test]
    fn missing_group_and_active_group_are_ignored() {
        let file = EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: Some("tool.deleted".to_owned()),
                groups: vec![ToolGroupSelectionV1 {
                    group_id: "tool.deleted".to_owned(),
                    selected_entry_id: Some("deleted.entry".to_owned()),
                }],
            },
        };
        let mut state = AppState::default();
        let initial_active = state.active_tool_id();

        file.restore_into(&mut state);

        assert_eq!(state.active_tool_id(), initial_active);
    }

    #[test]
    fn missing_active_group_keeps_the_initial_group_with_its_restored_representative() {
        let file = EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: Some("tool.deleted".to_owned()),
                groups: vec![ToolGroupSelectionV1 {
                    group_id: "tool.brush".to_owned(),
                    selected_entry_id: Some("builtin.brush.paint.directional_flat".to_owned()),
                }],
            },
        };
        let mut state = AppState::default();

        file.restore_into(&mut state);

        assert_eq!(
            representative_config_id(&state, "tool.brush"),
            Some("builtin.brush.paint.directional_flat")
        );
        assert_eq!(
            state.active_tool_id(),
            state
                .representative_tool_id_for_group("tool.brush")
                .expect("brush representative")
        );
    }

    #[test]
    fn moved_entry_is_not_restored_into_its_old_group() {
        let mut layout = AppState::default().tool_layout_snapshot();
        let fill = layout
            .tools
            .iter_mut()
            .find(|group| group.id == "tool.fill")
            .expect("fill group");
        let entry_index = fill
            .entries
            .iter()
            .position(|entry| {
                matches!(entry, ToolEntryDefinition::BuiltinTool(id) if id == "builtin.tool.fill.mesh")
            })
            .expect("mesh fill entry");
        let moved_entry = fill.entries.remove(entry_index);
        layout
            .tools
            .iter_mut()
            .find(|group| group.id == "tool.shape")
            .expect("shape group")
            .entries
            .push(moved_entry);
        let mut state = state_with_layout(layout);
        let fill_fallback = state
            .representative_tool_id_for_group("tool.fill")
            .expect("fill fallback");
        let file = EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: Some("tool.fill".to_owned()),
                groups: vec![ToolGroupSelectionV1 {
                    group_id: "tool.fill".to_owned(),
                    selected_entry_id: Some("builtin.tool.fill.mesh".to_owned()),
                }],
            },
        };

        file.restore_into(&mut state);

        assert_eq!(
            state.representative_tool_id_for_group("tool.fill"),
            Some(fill_fallback)
        );
        assert_eq!(state.active_tool_id(), fill_fallback);
    }

    #[test]
    fn reordered_groups_restore_by_group_id_instead_of_index() {
        let mut layout = AppState::default().tool_layout_snapshot();
        let brush_index = layout
            .tools
            .iter()
            .position(|group| group.id == "tool.brush")
            .expect("brush group");
        let selection_index = layout
            .tools
            .iter()
            .position(|group| group.id == "tool.selection")
            .expect("selection group");
        layout.tools.swap(brush_index, selection_index);
        let mut state = state_with_layout(layout);

        session().restore_into(&mut state);

        assert_eq!(
            representative_config_id(&state, "tool.selection"),
            Some("builtin.tool.selection.lasso")
        );
        assert_eq!(
            representative_config_id(&state, "tool.brush"),
            Some("builtin.brush.paint.directional_flat")
        );
        assert_eq!(
            state.active_tool_id(),
            state
                .representative_tool_id_for_group("tool.brush")
                .expect("brush representative")
        );
    }

    #[test]
    fn empty_group_restores_none_as_current_empty_group_tool() {
        let mut layout = AppState::default().tool_layout_snapshot();
        layout
            .tools
            .iter_mut()
            .find(|group| group.id == "tool.brush")
            .expect("brush group")
            .entries
            .clear();
        let mut state = state_with_layout(layout);
        let brush_index = state
            .tool_shelf()
            .groups
            .iter()
            .position(|group| group.id == "tool.brush")
            .expect("brush group");
        let file = EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: Some("tool.brush".to_owned()),
                groups: vec![ToolGroupSelectionV1 {
                    group_id: "tool.brush".to_owned(),
                    selected_entry_id: None,
                }],
            },
        };

        file.restore_into(&mut state);

        assert_eq!(
            state.representative_tool_id_for_group("tool.brush"),
            Some(ToolId::EmptyGroup(brush_index))
        );
        assert_eq!(state.active_tool_id(), ToolId::EmptyGroup(brush_index));
        let captured = EditorSessionFileV1::capture(&state).expect("capture empty group");
        assert_eq!(
            captured
                .tool_selection
                .groups
                .iter()
                .find(|group| group.group_id == "tool.brush")
                .expect("captured brush group")
                .selected_entry_id,
            None
        );
    }

    #[test]
    fn none_does_not_empty_a_group_that_now_has_entries() {
        let mut state = AppState::default();
        let fallback = state
            .representative_tool_id_for_group("tool.brush")
            .expect("brush fallback");
        let file = EditorSessionFileV1 {
            schema_version: EDITOR_SESSION_SCHEMA_VERSION,
            tool_selection: ToolSelectionStateV1 {
                active_group_id: Some("tool.brush".to_owned()),
                groups: vec![ToolGroupSelectionV1 {
                    group_id: "tool.brush".to_owned(),
                    selected_entry_id: None,
                }],
            },
        };

        file.restore_into(&mut state);

        assert_eq!(state.active_tool_id(), fallback);
        assert_eq!(
            state.representative_tool_id_for_group("tool.brush"),
            Some(fallback)
        );
    }
}
