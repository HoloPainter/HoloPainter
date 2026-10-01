use crate::{
    application::AppState,
    core::{
        brush_engine::{BrushEngineDefinition, BrushEngineOrigin},
        brush_preset::BrushPresetOrigin,
        image_asset::ImageAssetOrigin,
        texture::TextureResourceDefinition,
        tool::{ToolDefinition, ToolGroup},
        tool_layout::ToolGroupDefinition,
    },
    localization::Localization,
};

pub(crate) fn tool_name(l10n: &Localization, state: &AppState, tool: &ToolDefinition) -> String {
    if tool.config_id.starts_with("builtin.tool.") {
        return l10n.builtin_name("tool", &tool.config_id, &tool.name);
    }
    if matches!(
        state.brush_presets().origin(&tool.config_id),
        Some(BrushPresetOrigin::Builtin)
    ) {
        return l10n.builtin_name("brush", &tool.config_id, &tool.name);
    }
    tool.name.clone()
}

pub(crate) fn tool_group_name(l10n: &Localization, group: &ToolGroup) -> String {
    if group.id.starts_with("tool.") {
        l10n.builtin_name("group", &group.id, &group.name)
    } else {
        group.name.clone()
    }
}

pub(crate) fn tool_group_definition_name(
    l10n: &Localization,
    group: &ToolGroupDefinition,
) -> String {
    if group.id.starts_with("tool.") {
        l10n.builtin_name("group", &group.id, &group.display_name)
    } else {
        group.display_name.clone()
    }
}

pub(crate) fn brush_engine_name(
    l10n: &Localization,
    state: &AppState,
    engine: &BrushEngineDefinition,
) -> String {
    brush_engine_definition_name(
        l10n,
        engine,
        matches!(
            state.brush_engines().origin(&engine.id),
            Some(BrushEngineOrigin::Builtin)
        ),
    )
}

pub(crate) fn brush_engine_definition_name(
    l10n: &Localization,
    engine: &BrushEngineDefinition,
    is_builtin: bool,
) -> String {
    if is_builtin {
        l10n.builtin_name("engine", &engine.id, &engine.display_name)
    } else {
        engine.display_name.clone()
    }
}

pub(crate) fn brush_engine_part_name(
    l10n: &Localization,
    engine_id: &str,
    is_builtin: bool,
    part_namespace: &str,
    part_id: &str,
    fallback: &str,
) -> String {
    if is_builtin {
        l10n.builtin_part_name("engine", engine_id, part_namespace, part_id, fallback)
    } else {
        fallback.to_owned()
    }
}

pub(crate) fn texture_resource_name(
    l10n: &Localization,
    texture: &TextureResourceDefinition,
) -> String {
    if texture.source_asset_origin == ImageAssetOrigin::Builtin {
        l10n.builtin_name("image", &texture.source_asset_id, &texture.display_name)
    } else {
        texture.display_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core::brush_engine::TextureResourceFormat, settings::LanguagePreference};

    fn texture(
        origin: ImageAssetOrigin,
        source_asset_id: &str,
        display_name: &str,
    ) -> TextureResourceDefinition {
        TextureResourceDefinition {
            id: "texture.test".to_owned(),
            display_name: display_name.to_owned(),
            source_asset_id: source_asset_id.to_owned(),
            source_asset_origin: origin,
            format: TextureResourceFormat::R8Unorm,
            size: [1, 1],
            r8: vec![255],
            mipmaps: false,
            tags: Vec::new(),
        }
    }

    #[test]
    fn texture_resource_names_translate_only_builtin_source_assets() {
        let l10n = Localization::with_system_locale(LanguagePreference::Japanese, None);
        let builtin = texture(
            ImageAssetOrigin::Builtin,
            "builtin.image.brush_tip.brush",
            "brush",
        );
        let user = texture(
            ImageAssetOrigin::User,
            "builtin.image.brush_tip.brush",
            "My Brush",
        );
        let override_asset = texture(
            ImageAssetOrigin::UserOverride,
            "builtin.image.brush_tip.brush",
            "My Override",
        );

        assert_eq!(texture_resource_name(&l10n, &builtin), "ブラシ");
        assert_eq!(texture_resource_name(&l10n, &user), "My Brush");
        assert_eq!(texture_resource_name(&l10n, &override_asset), "My Override");
    }

    #[test]
    fn user_engine_parts_bypass_builtin_fluent_keys() {
        let l10n = Localization::with_system_locale(LanguagePreference::Japanese, None);

        assert_eq!(
            brush_engine_part_name(&l10n, "paint", false, "param", "opacity", "User Opacity",),
            "User Opacity"
        );
        assert_eq!(
            brush_engine_part_name(&l10n, "paint", true, "param", "opacity", "Opacity",),
            "不透明度"
        );
    }
}
