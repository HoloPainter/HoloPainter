use fluent::{FluentArgs, FluentBundle, FluentResource};
use fluent_langneg::{
    LanguageIdentifier as NegotiationLanguageIdentifier, NegotiationStrategy, negotiate_languages,
};
use unic_langid::LanguageIdentifier as BundleLanguageIdentifier;

use crate::settings::LanguagePreference;

const EN_US: &str = "en-US";
const JA_JP: &str = "ja-JP";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppLocale {
    EnUs,
    JaJp,
}

impl AppLocale {
    pub const fn language_name(self) -> &'static str {
        match self {
            Self::EnUs => "English",
            Self::JaJp => "日本語",
        }
    }
}

pub struct Localization {
    preference: LanguagePreference,
    system_locale: AppLocale,
    active_locale: AppLocale,
    english: FluentBundle<FluentResource>,
    japanese: FluentBundle<FluentResource>,
}

impl Localization {
    pub fn new(preference: LanguagePreference) -> Self {
        Self::with_system_locale(preference, sys_locale::get_locale().as_deref())
    }

    pub(crate) fn with_system_locale(
        preference: LanguagePreference,
        system_locale: Option<&str>,
    ) -> Self {
        let system_locale = negotiate_system_locale(system_locale);
        Self {
            preference,
            system_locale,
            active_locale: effective_locale(preference, system_locale),
            english: load_bundle(EN_US),
            japanese: load_bundle(JA_JP),
        }
    }

    pub const fn active_locale(&self) -> AppLocale {
        self.active_locale
    }

    pub const fn system_locale(&self) -> AppLocale {
        self.system_locale
    }

    pub const fn preference(&self) -> LanguagePreference {
        self.preference
    }

    pub fn set_preference(&mut self, preference: LanguagePreference) {
        self.preference = preference;
        self.active_locale = effective_locale(preference, self.system_locale);
    }

    pub fn text(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Looks up a built-in resource by its stable identifier. User-provided
    /// display names must bypass this method and be shown verbatim.
    pub fn builtin_name(&self, namespace: &str, stable_id: &str, fallback: &str) -> String {
        let conventional_prefix = format!("builtin.{namespace}.");
        let resource_prefix = format!("{namespace}.");
        let stable_id = stable_id
            .strip_prefix(&conventional_prefix)
            .or_else(|| stable_id.strip_prefix(&resource_prefix))
            .unwrap_or(stable_id);
        let namespace = localization_key_segment(namespace);
        let stable_id = localization_key_segment(stable_id);
        let key = format!("builtin-{namespace}-{stable_id}");
        self.try_text(&key).unwrap_or_else(|| fallback.to_owned())
    }

    pub fn builtin_part_name(
        &self,
        namespace: &str,
        stable_id: &str,
        part_namespace: &str,
        part_id: &str,
        fallback: &str,
    ) -> String {
        let namespace = localization_key_segment(namespace);
        let stable_id = localization_key_segment(stable_id);
        let part_namespace = localization_key_segment(part_namespace);
        let part_id = localization_key_segment(part_id);
        let key = format!("builtin-{namespace}-{stable_id}-{part_namespace}-{part_id}");
        self.try_text(&key).unwrap_or_else(|| fallback.to_owned())
    }

    fn try_text(&self, key: &str) -> Option<String> {
        let active = match self.active_locale {
            AppLocale::EnUs => &self.english,
            AppLocale::JaJp => &self.japanese,
        };
        format_from_bundle(active, key, None).or_else(|| {
            (self.active_locale != AppLocale::EnUs)
                .then(|| format_from_bundle(&self.english, key, None))
                .flatten()
        })
    }

    pub fn format(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        let active = match self.active_locale {
            AppLocale::EnUs => &self.english,
            AppLocale::JaJp => &self.japanese,
        };
        if let Some(value) = format_from_bundle(active, key, args) {
            return value;
        }
        if self.active_locale != AppLocale::EnUs
            && let Some(value) = format_from_bundle(&self.english, key, args)
        {
            return value;
        }
        eprintln!("Missing localization message: {key}");
        format!("[missing: {key}]")
    }

    #[cfg(test)]
    pub(crate) fn format_status(&self, status: &crate::application::StatusMessage) -> String {
        status.format(self)
    }
}

fn localization_key_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            'A'..='Z' => character.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' | '-' => character,
            _ => '-',
        })
        .collect()
}

fn format_from_bundle(
    bundle: &FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs<'_>>,
) -> Option<String> {
    let pattern = bundle.get_message(key)?.value()?;
    let mut errors = Vec::new();
    let value = bundle
        .format_pattern(pattern, args, &mut errors)
        .into_owned();
    for error in errors {
        eprintln!("Formatting localization message {key:?}: {error}");
    }
    Some(value)
}

fn load_bundle(locale: &str) -> FluentBundle<FluentResource> {
    let language: BundleLanguageIdentifier = locale.parse().expect("built-in locale must be valid");
    let mut bundle = FluentBundle::new(vec![language]);
    bundle
        .add_builtins()
        .expect("failed to register Fluent built-in functions");
    bundle.set_use_isolating(false);
    let directory = format!("i18n/{locale}");
    let mut found = false;
    for file in crate::embedded_resources::files_under(&directory) {
        if !file.path.ends_with(".ftl") {
            continue;
        }
        found = true;
        let source = std::str::from_utf8(file.bytes)
            .unwrap_or_else(|error| panic!("{} is not UTF-8: {error}", file.path));
        let resource = FluentResource::try_new(source.to_owned()).unwrap_or_else(|(_, errors)| {
            panic!("parsing embedded Fluent resource {}: {errors:?}", file.path)
        });
        bundle.add_resource(resource).unwrap_or_else(|errors| {
            panic!("adding embedded Fluent resource {}: {errors:?}", file.path)
        });
    }
    assert!(
        found,
        "no embedded Fluent resources found under {directory}"
    );
    bundle
}

const fn effective_locale(preference: LanguagePreference, system_locale: AppLocale) -> AppLocale {
    match preference {
        LanguagePreference::English => AppLocale::EnUs,
        LanguagePreference::Japanese => AppLocale::JaJp,
        LanguagePreference::System => system_locale,
    }
}

fn negotiate_system_locale(system_locale: Option<&str>) -> AppLocale {
    let available = [EN_US.parse().unwrap(), JA_JP.parse().unwrap()];
    let default: NegotiationLanguageIdentifier = EN_US.parse().unwrap();
    let requested = system_locale
        .and_then(|locale| locale.replace('_', "-").parse().ok())
        .into_iter()
        .collect::<Vec<NegotiationLanguageIdentifier>>();
    let negotiated = negotiate_languages(
        &requested,
        &available,
        Some(&default),
        NegotiationStrategy::Filtering,
    );
    if negotiated
        .first()
        .is_some_and(|locale| locale.language.as_str() == "ja")
    {
        AppLocale::JaJp
    } else {
        AppLocale::EnUs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_embedded_fluent_resources_parse_without_duplicate_keys() {
        let _ = load_bundle(EN_US);
        let _ = load_bundle(JA_JP);
    }

    #[test]
    fn system_locale_is_negotiated_and_explicit_preferences_win() {
        assert_eq!(negotiate_system_locale(Some("ja-JP")), AppLocale::JaJp);
        assert_eq!(negotiate_system_locale(Some("ja")), AppLocale::JaJp);
        assert_eq!(negotiate_system_locale(Some("fr-FR")), AppLocale::EnUs);
        assert_eq!(
            effective_locale(LanguagePreference::English, AppLocale::JaJp),
            AppLocale::EnUs
        );
        assert_eq!(
            effective_locale(LanguagePreference::Japanese, AppLocale::EnUs),
            AppLocale::JaJp
        );
    }

    #[test]
    fn explicit_preference_does_not_overwrite_the_system_locale() {
        let mut localization =
            Localization::with_system_locale(LanguagePreference::English, Some("ja-JP"));
        assert_eq!(localization.active_locale(), AppLocale::EnUs);
        assert_eq!(localization.system_locale(), AppLocale::JaJp);

        localization.set_preference(LanguagePreference::System);
        assert_eq!(localization.active_locale(), AppLocale::JaJp);
    }

    #[test]
    fn japanese_falls_back_to_english_and_missing_keys_are_visible() {
        let localization = Localization::with_system_locale(LanguagePreference::Japanese, None);
        assert_eq!(
            localization.text("localization-fallback-probe"),
            "English fallback"
        );
        assert_eq!(
            localization.text("does-not-exist"),
            "[missing: does-not-exist]"
        );
    }

    #[test]
    fn fluent_arguments_are_formatted() {
        let localization = Localization::with_system_locale(LanguagePreference::Japanese, None);
        let mut args = FluentArgs::new();
        args.set("file_name", "sample.png");
        assert_eq!(
            localization.format("status-imported-layer", Some(&args)),
            "sample.png をレイヤーとして読み込みました。"
        );
    }

    #[test]
    fn fluent_number_builtin_formats_ui_and_status_messages() {
        let localization = Localization::with_system_locale(LanguagePreference::Japanese, None);

        let mut rotation_args = FluentArgs::new();
        rotation_args.set("degrees", 12.5);
        assert_eq!(
            localization.format("viewport-rotation", Some(&rotation_args)),
            "回転: 12.5°"
        );

        let mut blur_args = FluentArgs::new();
        blur_args.set("radius", 4.5);
        blur_args.set("angle", 47.0);
        assert_eq!(
            localization.format(
                "status-spatial-blur-applied-similar-normals",
                Some(&blur_args),
            ),
            "空間ぼかしを適用しました（平均 4.5 テクセル、類似法線 ≤ 47°）"
        );
    }

    #[test]
    fn builtin_key_segments_use_one_normalization_rule() {
        assert_eq!(localization_key_segment("Composite_Mode"), "composite-mode");
        assert_eq!(localization_key_segment("source.over"), "source-over");

        let localization = Localization::with_system_locale(LanguagePreference::Japanese, None);
        assert_eq!(
            localization.builtin_part_name(
                "engine",
                "paint",
                "param-composite_mode-variant",
                "source_over",
                "Color",
            ),
            "カラー"
        );
        assert_eq!(
            localization.builtin_part_name(
                "engine",
                "paint",
                "param-tip_rotation-variant",
                "stroke_direction",
                "Stroke Direction",
            ),
            "ストローク方向"
        );
    }

    #[test]
    fn builtin_brush_engine_ron_labels_have_fluent_entries() {
        use crate::core::{
            brush_engine::{BrushEngineRegistry, ParamExposure, ParamType},
            image_asset::ImageAssetCatalog,
            texture::TextureCatalog,
        };

        let images = ImageAssetCatalog::load_effective(None).unwrap();
        let textures = TextureCatalog::from_image_assets(&images).unwrap();
        let engines = BrushEngineRegistry::load_effective(Vec::new(), &textures).unwrap();
        let localization = Localization::with_system_locale(LanguagePreference::Japanese, None);

        for engine in engines.engines() {
            for param in engine
                .params
                .iter()
                .filter(|param| param.exposure == ParamExposure::Public)
            {
                assert_ne!(
                    localization.builtin_part_name(
                        "engine",
                        &engine.id,
                        "param",
                        &param.name,
                        "__missing__",
                    ),
                    "__missing__",
                    "missing Fluent label for {} parameter {}",
                    engine.id,
                    param.name,
                );
                if let ParamType::Enum { variants } = &param.ty {
                    for variant in variants {
                        assert_ne!(
                            localization.builtin_part_name(
                                "engine",
                                &engine.id,
                                &format!("param-{}-variant", param.name),
                                &variant.name,
                                "__missing__",
                            ),
                            "__missing__",
                            "missing Fluent label for {} parameter {} variant {}",
                            engine.id,
                            param.name,
                            variant.name,
                        );
                    }
                }
            }
        }
    }
}
