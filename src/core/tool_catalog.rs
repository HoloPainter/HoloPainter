use std::{collections::HashSet, path::Path};

use anyhow::{Result, bail, ensure};

use crate::core::{
    brush_engine::{BrushEngineDefinition, BrushEngineRegistry, BrushSpace},
    brush_preset::BrushPresetCatalog,
    image_asset::ImageAssetCatalog,
    stroke::StrokeSpace,
    texture::TextureCatalog,
    tool::{
        DefaultTools, ToolBehavior, ToolDefinition, ToolEntry, ToolGroup, ToolId, ToolSection,
        ToolShelf, builtin_color_picker_tool, builtin_fill_material_tool, builtin_fill_mesh_tool,
        builtin_fill_polygon_tool, builtin_lasso_erase_tool, builtin_lasso_paint_tool,
        builtin_lasso_selection_tool, builtin_rectangle_erase_tool, builtin_rectangle_paint_tool,
        builtin_rectangle_selection_tool, builtin_surface_decal_tool, builtin_transform_tool,
        builtin_view_projection_decal_tool, empty_group_config_id,
    },
    tool_layout::{ToolEntryDefinition, ToolLayoutFileV1},
};

pub(crate) const DEFAULT_BRUSH_PRESET_ID: &str = "builtin.brush.default";

pub fn load_default_tools_from_resources() -> Result<DefaultTools> {
    let layout = ToolLayoutFileV1::builtin().map_err(anyhow::Error::msg)?;
    let image_assets = ImageAssetCatalog::load_effective(None)?;
    load_tools_from_resources(None, &image_assets, layout)
}

#[cfg(test)]
pub(crate) fn load_test_tools_from_fixtures() -> Result<DefaultTools> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let layout: ToolLayoutFileV1 = ron::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tools/default.tool_layout.ron"
    )))?;
    layout.validate().map_err(anyhow::Error::msg)?;
    let image_assets = ImageAssetCatalog::load_effective(None)?;
    let textures = TextureCatalog::from_image_assets(&image_assets)?;
    let engines = BrushEngineRegistry::load_effective(Vec::new(), &textures)?;
    let presets =
        BrushPresetCatalog::load_from_fixtures(root.join("tests/fixtures/brushes/presets"), None)?;
    resolve_default_tools(&engines, &presets, &textures, &layout)
}

pub(crate) fn load_tools_from_resources(
    user_preset_dir: Option<&Path>,
    image_assets: &ImageAssetCatalog,
    layout: ToolLayoutFileV1,
) -> Result<DefaultTools> {
    load_tools_from_resources_with_engines(user_preset_dir, image_assets, layout, Vec::new())
}

pub(crate) fn load_tools_from_resources_with_engines(
    user_preset_dir: Option<&Path>,
    image_assets: &ImageAssetCatalog,
    layout: ToolLayoutFileV1,
    user_engines: Vec<BrushEngineDefinition>,
) -> Result<DefaultTools> {
    let textures = TextureCatalog::from_image_assets(image_assets)?;
    let engines = BrushEngineRegistry::load_effective(user_engines, &textures)?;
    let presets = BrushPresetCatalog::load_effective(user_preset_dir)?;
    resolve_default_tools(&engines, &presets, &textures, &layout)
}

pub(crate) fn resolve_default_tools(
    engines: &BrushEngineRegistry,
    presets: &BrushPresetCatalog,
    textures: &TextureCatalog,
    layout: &ToolLayoutFileV1,
) -> Result<DefaultTools> {
    let default_brush_preset = resolve_brush_preset(
        engines,
        presets,
        textures,
        DEFAULT_BRUSH_PRESET_ID,
        "default brush preset",
    )?
    .0;
    let mut stroke_presets = Vec::new();
    let mut catalog = Vec::new();
    let mut groups = Vec::new();
    let mut group_ids = HashSet::new();

    for group in &layout.tools {
        ensure!(
            group_ids.insert(group.id.clone()),
            "duplicate tool group id {:?}",
            group.id
        );
        let section = section_for_group(&group.id);
        let mut entries = Vec::new();
        for entry in &group.entries {
            match entry {
                ToolEntryDefinition::BrushPreset(id) => {
                    let (stroke_preset, handle, preset, engine) =
                        resolve_brush_preset(engines, presets, textures, id, "tool layout")?;
                    let preset_index = stroke_presets.len();
                    stroke_presets.push(stroke_preset);
                    let tool_id = ToolId::BrushPreset(handle.0);
                    catalog.push(ToolDefinition {
                        id: tool_id,
                        config_id: preset.id().to_owned(),
                        name: preset.display_name().to_owned(),
                        section,
                        behavior: ToolBehavior::Stroke { preset_index },
                        supported_spaces: spaces_from_engine(engine),
                    });
                    entries.push(ToolEntry {
                        tool_id,
                        name: preset.display_name().to_owned(),
                    });
                }
                ToolEntryDefinition::BuiltinTool(id) => {
                    let tool = resolve_builtin_tool(id)?;
                    entries.push(ToolEntry {
                        tool_id: tool.id,
                        name: tool.name.clone(),
                    });
                    catalog.push(tool);
                }
                ToolEntryDefinition::Separator => {}
            }
        }
        if entries.is_empty() {
            catalog.push(ToolDefinition {
                id: ToolId::EmptyGroup(groups.len()),
                config_id: empty_group_config_id(&group.id),
                name: "No Tool".to_owned(),
                section,
                behavior: ToolBehavior::NoOp,
                supported_spaces: Vec::new(),
            });
        }
        groups.push(ToolGroup {
            id: group.id.clone(),
            name: group.display_name.clone(),
            icon: group.icon.clone(),
            entries,
        });
    }

    ensure!(
        !catalog.is_empty(),
        "tool layout must resolve at least one tool"
    );
    Ok(DefaultTools {
        presets: stroke_presets,
        default_brush_preset,
        catalog,
        shelf: ToolShelf { groups },
        brush_engines: engines.clone(),
        brush_presets: presets.clone(),
        brush_textures: textures.clone(),
        tool_layout: layout.clone(),
    })
}

fn resolve_brush_preset<'a>(
    engines: &'a BrushEngineRegistry,
    presets: &'a BrushPresetCatalog,
    textures: &TextureCatalog,
    id: &str,
    context: &str,
) -> Result<(
    crate::core::stroke_preset::StrokeToolPreset,
    crate::core::brush_preset::BrushPresetHandle,
    &'a crate::core::brush_preset::BrushPresetDefinition,
    &'a BrushEngineDefinition,
)> {
    let handle = presets
        .handle(id)
        .ok_or_else(|| anyhow::anyhow!("{context} references unknown brush preset {:?}", id))?;
    let preset = presets.get(handle).expect("validated brush preset handle");
    let engine = engines.get(preset.engine_id()).ok_or_else(|| {
        anyhow::anyhow!(
            "brush preset {:?} references unknown engine {:?}",
            preset.id(),
            preset.engine_id()
        )
    })?;
    let stroke_preset = preset.resolve(engine, textures)?;
    Ok((stroke_preset, handle, preset, engine))
}

fn resolve_builtin_tool(id: &str) -> Result<ToolDefinition> {
    let tool = match id {
        "builtin.tool.decal.surface" => builtin_surface_decal_tool(),
        "builtin.tool.decal.view_projection" => builtin_view_projection_decal_tool(),
        "builtin.tool.fill.material" => builtin_fill_material_tool(),
        "builtin.tool.fill.mesh" => builtin_fill_mesh_tool(),
        "builtin.tool.fill.polygon" => builtin_fill_polygon_tool(),
        "builtin.tool.shape.rectangle_paint" => builtin_rectangle_paint_tool(),
        "builtin.tool.shape.rectangle_erase" => builtin_rectangle_erase_tool(),
        "builtin.tool.selection.rectangle" => builtin_rectangle_selection_tool(),
        "builtin.tool.shape.lasso_paint" => builtin_lasso_paint_tool(),
        "builtin.tool.shape.lasso_erase" => builtin_lasso_erase_tool(),
        "builtin.tool.selection.lasso" => builtin_lasso_selection_tool(),
        "builtin.tool.transform" => builtin_transform_tool(),
        "builtin.tool.sampler.color_picker" => builtin_color_picker_tool(),
        _ => bail!("unknown builtin tool id {:?}", id),
    };
    Ok(tool)
}

fn section_for_group(id: &str) -> ToolSection {
    match id {
        "tool.fill" => ToolSection::Fill,
        "tool.shape" => ToolSection::Shape,
        "tool.selection" => ToolSection::Select,
        "tool.transform" => ToolSection::Transform,
        "tool.sampler" => ToolSection::Sampler,
        _ => ToolSection::Paint,
    }
}

pub(crate) fn spaces_from_engine(engine: &BrushEngineDefinition) -> Vec<StrokeSpace> {
    engine
        .capabilities
        .spaces
        .iter()
        .map(|space| match space {
            BrushSpace::Uv => StrokeSpace::Uv,
            BrushSpace::Surface => StrokeSpace::Surface,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_decal_fixture_resolves_surface_only_behavior() {
        let tools = load_test_tools_from_fixtures().unwrap();
        let group = tools
            .shelf
            .groups
            .iter()
            .find(|group| group.id == "tool.decal")
            .expect("default layout should contain Decal group");
        assert_eq!(group.name, "Decal");
        assert_eq!(group.icon.as_deref(), Some("builtin.icon.sticker"));
        assert_eq!(group.entries.len(), 2);
        assert_eq!(group.entries[0].tool_id, ToolId::SurfaceDecal);
        assert_eq!(group.entries[1].tool_id, ToolId::ViewProjectionDecal);

        let tool = tools
            .catalog
            .iter()
            .find(|tool| tool.id == ToolId::SurfaceDecal)
            .expect("Decal tool should resolve from default layout");
        assert!(matches!(
            &tool.behavior,
            ToolBehavior::Decal {
                kind: crate::core::tool::DecalKind::Surface
            }
        ));
        assert_eq!(tool.section, ToolSection::Paint);
        assert_eq!(tool.supported_spaces, vec![StrokeSpace::Surface]);
        assert!(tool.edits_paint_target());
        assert_eq!(
            tool.surface_hit_requirement(),
            crate::core::tool::SurfaceHitRequirement::Required
        );

        let view_tool = tools
            .catalog
            .iter()
            .find(|tool| tool.id == ToolId::ViewProjectionDecal)
            .expect("View Projection Decal tool should resolve from default layout");
        assert!(matches!(
            &view_tool.behavior,
            ToolBehavior::Decal {
                kind: crate::core::tool::DecalKind::ViewProjection
            }
        ));
        assert_eq!(
            view_tool.surface_hit_requirement(),
            crate::core::tool::SurfaceHitRequirement::None
        );
    }

    #[test]
    fn fill_tools_support_uv_and_surface_and_require_a_surface_hit() {
        let tools = load_test_tools_from_fixtures().unwrap();
        for (tool_id, expected_scope) in [
            (ToolId::FillMaterial, crate::core::tool::FillScope::Material),
            (ToolId::FillMesh, crate::core::tool::FillScope::Mesh),
            (ToolId::FillPolygon, crate::core::tool::FillScope::Polygon),
        ] {
            let tool = tools
                .catalog
                .iter()
                .find(|tool| tool.id == tool_id)
                .expect("fill tool should resolve from default layout");
            assert!(matches!(
                tool.behavior,
                ToolBehavior::Fill { scope } if scope == expected_scope
            ));
            assert_eq!(
                tool.supported_spaces,
                vec![StrokeSpace::Uv, StrokeSpace::Surface]
            );
            assert_eq!(
                tool.surface_hit_requirement(),
                crate::core::tool::SurfaceHitRequirement::Required
            );
        }
    }

    #[test]
    fn transform_fixture_resolves_uv_only_behavior() {
        let tools = load_test_tools_from_fixtures().unwrap();
        let group = tools
            .shelf
            .groups
            .iter()
            .find(|group| group.id == "tool.transform")
            .expect("default layout should contain Transform group");
        assert_eq!(group.name, "Transform");
        assert_eq!(group.icon.as_deref(), Some("builtin.icon.transform"));
        assert_eq!(group.entries.len(), 1);
        assert_eq!(group.entries[0].tool_id, ToolId::Transform);

        let tool = tools
            .catalog
            .iter()
            .find(|tool| tool.id == ToolId::Transform)
            .expect("Transform tool should resolve from default layout");
        assert!(matches!(&tool.behavior, ToolBehavior::Transform));
        assert_eq!(tool.section, ToolSection::Transform);
        assert_eq!(tool.supported_spaces, vec![StrokeSpace::Uv]);
    }

    #[test]
    fn color_picker_fixture_resolves_sampler_behavior() {
        let tools = load_test_tools_from_fixtures().unwrap();
        let group = tools
            .shelf
            .groups
            .iter()
            .find(|group| group.id == "tool.sampler")
            .expect("default layout should contain Sampler group");
        assert_eq!(group.name, "Sampler");
        assert_eq!(group.icon.as_deref(), Some("builtin.icon.colorize"));
        assert_eq!(group.entries.len(), 1);
        assert_eq!(group.entries[0].tool_id, ToolId::ColorPicker);

        let tool = tools
            .catalog
            .iter()
            .find(|tool| tool.id == ToolId::ColorPicker)
            .expect("Color Picker tool should resolve from default layout");
        assert!(matches!(&tool.behavior, ToolBehavior::ColorPicker));
        assert_eq!(tool.section, ToolSection::Sampler);
        assert_eq!(
            tool.supported_spaces,
            vec![StrokeSpace::Uv, StrokeSpace::Surface]
        );
        assert!(!tool.edits_paint_target());
        assert_eq!(
            tool.surface_hit_requirement(),
            crate::core::tool::SurfaceHitRequirement::Optional
        );
    }

    #[test]
    fn separator_only_group_resolves_to_runtime_no_op() {
        let defaults = load_test_tools_from_fixtures().unwrap();
        let layout = ToolLayoutFileV1 {
            schema_version: crate::core::tool_layout::TOOL_LAYOUT_SCHEMA_VERSION,
            id: "test.layout".to_owned(),
            display_name: "Test".to_owned(),
            tools: vec![crate::core::tool_layout::ToolGroupDefinition {
                id: "tool.empty".to_owned(),
                display_name: "Empty".to_owned(),
                icon: None,
                entries: vec![ToolEntryDefinition::Separator],
            }],
        };

        let tools = resolve_default_tools(
            &defaults.brush_engines,
            &defaults.brush_presets,
            &defaults.brush_textures,
            &layout,
        )
        .expect("separator-only group should resolve");

        assert!(tools.presets.is_empty());
        assert_eq!(tools.shelf.groups.len(), 1);
        assert!(tools.shelf.groups[0].entries.is_empty());
        let no_op = tools
            .catalog
            .iter()
            .find(|tool| tool.id == ToolId::EmptyGroup(0))
            .expect("empty group should have a runtime no-op");
        assert_eq!(no_op.config_id, empty_group_config_id("tool.empty"));
        assert!(matches!(no_op.behavior, ToolBehavior::NoOp));
        assert_eq!(
            no_op.surface_hit_requirement(),
            crate::core::tool::SurfaceHitRequirement::None
        );
        assert!(!no_op.supports_space(StrokeSpace::Uv));
        assert!(!no_op.supports_surface_mirror());
        assert!(!no_op.edits_paint_target());
        assert_eq!(tools.shelf.group_containing(no_op.id), Some(0));
    }

    #[test]
    fn test_brush_template_is_resolved_but_not_added_to_the_shelf() {
        let tools = load_test_tools_from_fixtures().unwrap();

        assert_eq!(tools.default_brush_preset.name, "Default Brush");
        assert!(
            tools
                .catalog
                .iter()
                .all(|tool| tool.config_id != DEFAULT_BRUSH_PRESET_ID)
        );
        assert!(
            tools
                .shelf
                .groups
                .iter()
                .all(|group| group.entries.iter().all(|entry| tools
                    .catalog
                    .iter()
                    .find(|tool| tool.id == entry.tool_id)
                    .is_some_and(|tool| tool.config_id != DEFAULT_BRUSH_PRESET_ID)))
        );
    }
}
