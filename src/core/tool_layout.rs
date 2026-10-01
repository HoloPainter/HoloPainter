use std::{collections::HashSet, sync::LazyLock};

use serde::{Deserialize, Serialize};

use crate::persistence::parse_ron;

pub(crate) const TOOL_LAYOUT_SCHEMA_VERSION: u32 = 1;
const BUILTIN_TOOL_LAYOUT_RESOURCE: &str = "tools/layouts/default.tool_layout.ron";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolLayoutFileV1 {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) tools: Vec<ToolGroupDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ToolGroupDefinition {
    pub(crate) id: String,
    pub(crate) display_name: String,
    #[serde(default)]
    pub(crate) icon: Option<String>,
    pub(crate) entries: Vec<ToolEntryDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum ToolEntryDefinition {
    BrushPreset(String),
    BuiltinTool(String),
    Separator,
}

static BUILTIN_TOOL_LAYOUT: LazyLock<Result<ToolLayoutFileV1, String>> = LazyLock::new(|| {
    let layout: ToolLayoutFileV1 = parse_ron(
        crate::embedded_resources::text(BUILTIN_TOOL_LAYOUT_RESOURCE)?,
        "embedded default tool layout",
    )?;
    layout.validate()?;
    Ok(layout)
});

impl ToolLayoutFileV1 {
    pub(crate) fn builtin() -> Result<Self, String> {
        (*BUILTIN_TOOL_LAYOUT).clone()
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != TOOL_LAYOUT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported tool layout schema_version {}",
                self.schema_version
            ));
        }
        if self.id.trim().is_empty() {
            return Err("tool layout id must not be empty".to_owned());
        }
        if self.display_name.trim().is_empty() {
            return Err("tool layout display_name must not be empty".to_owned());
        }

        let mut group_ids = HashSet::new();
        for group in &self.tools {
            if !group_ids.insert(group.id.as_str()) {
                return Err(format!("duplicate tool group id {:?}", group.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tool_layout_is_valid_and_roundtrips() {
        let layout = ToolLayoutFileV1::builtin().expect("builtin tool layout");
        let encoded = ron::ser::to_string(&layout).expect("serialize tool layout");
        let decoded: ToolLayoutFileV1 = ron::from_str(&encoded).expect("deserialize tool layout");

        assert_eq!(decoded, layout);
        decoded.validate().expect("roundtripped layout validates");
        assert_eq!(decoded.schema_version, TOOL_LAYOUT_SCHEMA_VERSION);
        assert!(!decoded.tools.is_empty());
    }

    #[test]
    fn validation_rejects_wrong_schema_version() {
        let mut layout = ToolLayoutFileV1::builtin().expect("builtin tool layout");
        layout.schema_version = TOOL_LAYOUT_SCHEMA_VERSION + 1;

        assert!(layout.validate().is_err());
    }
}
