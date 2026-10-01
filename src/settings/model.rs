use std::{collections::HashSet, sync::LazyLock};

use serde::{Deserialize, Serialize};

use crate::{
    core::{brush_engine::BrushEngineDefinition, curve::Curve},
    persistence::parse_ron,
};

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
const BUILTIN_SETTINGS_RESOURCE: &str = "settings/default.settings.ron";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsFileV1 {
    pub schema_version: u32,
    pub settings: UserSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSettings {
    pub appearance: AppearanceSettings,
    pub input: InputSettings,
    #[serde(default)]
    pub brush_engines: Vec<BrushEngineDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: AppTheme,
    #[serde(default)]
    pub language: LanguagePreference,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePreference {
    #[default]
    System,
    English,
    Japanese,
}

impl LanguagePreference {
    pub const ALL: [Self; 3] = [Self::System, Self::English, Self::Japanese];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    Light,
    Dark,
}

impl AppTheme {
    pub const ALL: [Self; 2] = [Self::Light, Self::Dark];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputSettings {
    pub tablet: TabletSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabletSettings {
    pub backend: TabletBackend,
    pub pressure_curve: Curve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabletBackend {
    #[serde(rename = "wintab")]
    WinTab,
    WindowsInk,
}

impl TabletBackend {
    pub const PRIORITY: [Self; 2] = [Self::WinTab, Self::WindowsInk];

    pub const fn label(self) -> &'static str {
        match self {
            Self::WinTab => "WinTab",
            Self::WindowsInk => "Windows Ink",
        }
    }
}

static BUILTIN_SETTINGS: LazyLock<Result<SettingsFileV1, String>> = LazyLock::new(|| {
    let settings: SettingsFileV1 = parse_ron(
        crate::embedded_resources::text(BUILTIN_SETTINGS_RESOURCE)?,
        "embedded default settings",
    )?;
    settings.validate()?;
    Ok(settings)
});

impl Default for UserSettings {
    fn default() -> Self {
        SettingsFileV1::builtin()
            .expect("embedded default settings must be valid")
            .settings
    }
}

impl SettingsFileV1 {
    pub fn builtin() -> Result<Self, String> {
        (*BUILTIN_SETTINGS).clone()
    }

    pub fn builtin_tablet_pressure_curve() -> Result<Curve, String> {
        Ok(Self::builtin()?.settings.input.tablet.pressure_curve)
    }

    pub fn from_user_settings(settings: UserSettings) -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            settings,
        }
    }

    pub fn into_user_settings(self) -> UserSettings {
        self.settings
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            return Err(format!(
                "unsupported settings schema version {}",
                self.schema_version
            ));
        }
        self.settings
            .input
            .tablet
            .pressure_curve
            .validate()
            .map_err(|error| format!("settings.input.tablet.pressure_curve: {error}"))?;
        let mut engine_ids = HashSet::new();
        for engine in &self.settings.brush_engines {
            if engine.id.trim().is_empty() {
                return Err("settings.brush_engines contains an empty engine id".to_owned());
            }
            if !engine_ids.insert(engine.id.as_str()) {
                return Err(format!(
                    "settings.brush_engines contains duplicate id {:?}",
                    engine.id
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_settings_are_valid_and_roundtrip() {
        let settings = SettingsFileV1::builtin().expect("builtin settings");
        let encoded = ron::ser::to_string(&settings).expect("serialize settings");
        assert!(!encoded.contains("view"));
        let decoded: SettingsFileV1 = ron::from_str(&encoded).expect("deserialize settings");
        assert_eq!(decoded, settings);
        decoded.validate().expect("roundtripped settings validate");
        assert_eq!(decoded.settings.appearance.theme, AppTheme::Dark);
        assert_eq!(decoded.settings.input.tablet.backend, TabletBackend::WinTab);
    }

    #[test]
    fn tablet_backend_ron_names_are_stable() {
        assert_eq!(ron::to_string(&TabletBackend::WinTab).unwrap(), "wintab");
        assert_eq!(
            ron::to_string(&TabletBackend::WindowsInk).unwrap(),
            "windows_ink"
        );
        assert_eq!(SETTINGS_SCHEMA_VERSION, 1);
    }

    #[test]
    fn app_theme_ron_names_are_stable() {
        assert_eq!(ron::to_string(&AppTheme::Light).unwrap(), "light");
        assert_eq!(ron::to_string(&AppTheme::Dark).unwrap(), "dark");
        assert_eq!(SETTINGS_SCHEMA_VERSION, 1);
    }

    #[test]
    fn language_preference_ron_names_are_stable() {
        assert_eq!(
            ron::to_string(&LanguagePreference::System).unwrap(),
            "system"
        );
        assert_eq!(
            ron::to_string(&LanguagePreference::English).unwrap(),
            "english"
        );
        assert_eq!(
            ron::to_string(&LanguagePreference::Japanese).unwrap(),
            "japanese"
        );
        assert_eq!(SETTINGS_SCHEMA_VERSION, 1);
    }

    #[test]
    fn settings_without_language_default_to_system() {
        let source = crate::embedded_resources::text(BUILTIN_SETTINGS_RESOURCE).unwrap();
        let legacy = source.replace("            language: system,\n", "");
        let decoded: SettingsFileV1 = ron::from_str(&legacy).expect("legacy settings deserialize");
        assert_eq!(
            decoded.settings.appearance.language,
            LanguagePreference::System
        );
        assert_eq!(decoded.schema_version, 1);
    }

    #[test]
    fn user_settings_default_to_embedded_settings() {
        let builtin = SettingsFileV1::builtin().expect("builtin settings");
        assert_eq!(UserSettings::default(), builtin.settings);
    }

    #[test]
    fn builtin_tablet_pressure_curve_matches_default_settings_resource() {
        let builtin = SettingsFileV1::builtin().expect("builtin settings");
        assert_eq!(
            SettingsFileV1::builtin_tablet_pressure_curve().expect("builtin pressure curve"),
            builtin.settings.input.tablet.pressure_curve
        );
    }

    #[test]
    fn settings_require_the_global_pressure_curve() {
        // Unknown fields are ignored, so renaming the key removes the required field
        // without depending on the default curve's points or source formatting.
        let source = crate::embedded_resources::text(BUILTIN_SETTINGS_RESOURCE).unwrap();
        let missing_curve = source.replacen("pressure_curve:", "unknown_curve:", 1);
        assert_ne!(source, missing_curve);
        let error = ron::from_str::<SettingsFileV1>(&missing_curve).unwrap_err();
        assert!(error.to_string().contains("pressure_curve"));
    }

    #[test]
    fn validation_rejects_wrong_version() {
        let mut settings = SettingsFileV1::builtin().expect("builtin settings");
        settings.schema_version = SETTINGS_SCHEMA_VERSION + 1;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let text = crate::embedded_resources::text(BUILTIN_SETTINGS_RESOURCE)
            .unwrap()
            .replacen(
                "schema_version: 1,",
                "schema_version: 1, future_setting: 123,",
                1,
            );
        let settings: SettingsFileV1 = ron::from_str(&text).expect("unknown field ignored");
        settings.validate().expect("settings validate");
    }

    #[test]
    fn user_brush_engines_round_trip_without_changing_settings_version() {
        let mut settings = SettingsFileV1::builtin().expect("builtin settings");
        let mut engine: BrushEngineDefinition =
            ron::from_str(crate::embedded_resources::text("brushes/engines/paint.ron").unwrap())
                .unwrap();
        engine.id = "user.engine.settings_roundtrip".to_owned();
        settings.settings.brush_engines.push(engine);
        let encoded = ron::ser::to_string(&settings).unwrap();
        let decoded: SettingsFileV1 = ron::from_str(&encoded).unwrap();
        assert_eq!(decoded, settings);
        assert_eq!(decoded.schema_version, 1);
    }
}
