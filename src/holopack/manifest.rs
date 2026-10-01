use serde::{Deserialize, Serialize};

pub(crate) const HOLOPACK_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HoloPackManifest {
    pub(crate) format_version: u32,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) author: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    pub(crate) resources: Vec<HoloPackResourceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HoloPackResourceEntry {
    pub(crate) kind: String,
    pub(crate) path: String,
}

impl HoloPackManifest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.format_version != HOLOPACK_FORMAT_VERSION {
            return Err(format!(
                "unsupported HoloPack format version {}",
                self.format_version
            ));
        }
        for (field, value) in [
            ("id", self.id.as_str()),
            ("name", self.name.as_str()),
            ("version", self.version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("HoloPack manifest {field} must not be empty"));
            }
        }
        Ok(())
    }
}
