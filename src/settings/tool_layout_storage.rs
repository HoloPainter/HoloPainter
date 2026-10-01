use std::path::Path;

use crate::{core::tool_layout::ToolLayoutFileV1, persistence::load_ron};

pub(crate) fn load_effective_tool_layout(path: Option<&Path>) -> ToolLayoutFileV1 {
    let builtin = ToolLayoutFileV1::builtin().expect("embedded default tool layout must be valid");
    let Some(path) = path else {
        return builtin;
    };
    match load_file(path) {
        Ok(Some(layout)) => layout,
        Ok(None) => builtin,
        Err(error) => {
            eprintln!("Ignoring invalid tool layout {}: {error}", path.display());
            builtin
        }
    }
}

fn load_file(path: &Path) -> Result<Option<ToolLayoutFileV1>, String> {
    let Some(layout) = load_ron::<ToolLayoutFileV1>(path)
        .map_err(|error| format!("loading tool layout: {error}"))?
    else {
        return Ok(None);
    };
    layout.validate()?;
    Ok(Some(layout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_user_layout_uses_builtin_without_creating_a_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("tools/layouts/default.tool_layout.ron");
        let loaded = load_effective_tool_layout(Some(&path));
        assert_eq!(loaded, ToolLayoutFileV1::builtin().expect("builtin layout"));
        assert!(!path.exists());
    }
}
