pub mod brush_settings;
pub mod color_picker_options;
pub mod decal_options;
pub mod fill_options;
pub mod selection_options;
pub mod shape_options;
pub mod stroke_options;
pub mod transform_options;
pub mod widgets;

use eframe::egui;

use crate::{
    application::{AppState, EditorActionContext},
    core::{
        image_asset::ImageAssetCatalog,
        tool::{ToolBehavior, ToolId},
    },
    localization::Localization,
    ui::{
        icons::UiIconRegistry,
        input::shortcut_profile::ShortcutProfile,
        render_resources::UiRenderResources,
        view_output::ViewOutput,
        widgets::{
            image_asset_picker::ImageAssetThumbnailCache,
            resource_thumbnail_picker::ResourceThumbnailCache,
        },
    },
};

#[derive(Debug, Default)]
pub struct ToolOptionsUiState {
    brush_settings_open: bool,
    name_edit: Option<(String, String)>,
    tool_id: Option<ToolId>,
    resource_thumbnails: ResourceThumbnailCache,
    image_asset_thumbnails: ImageAssetThumbnailCache,
}

impl ToolOptionsUiState {
    fn sync_tool(&mut self, tool_id: ToolId) {
        if self.tool_id != Some(tool_id) {
            self.tool_id = Some(tool_id);
            self.brush_settings_open = false;
            self.name_edit = None;
        }
    }
}

pub fn draw_tool_options(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    render_resources: &UiRenderResources,
    image_assets: &ImageAssetCatalog,
    selected_decal_asset_id: Option<&str>,
    icons: &UiIconRegistry,
    ui_state: &mut ToolOptionsUiState,
    shortcut_profile: &ShortcutProfile,
) -> ViewOutput {
    let Some(tool) = state.panel_tool_definition().cloned() else {
        ui.label(l10n.text("tool-options-no-active-tool"));
        return ViewOutput::default();
    };
    ui_state.sync_tool(tool.id);
    if !matches!(tool.behavior, ToolBehavior::NoOp) {
        ui.label(crate::ui::localized::tool_name(l10n, state, &tool));
    }
    match tool.behavior {
        ToolBehavior::NoOp => ViewOutput::default(),
        ToolBehavior::Stroke { preset_index } => {
            stroke_options::draw_stroke_tool_options(ui, l10n, state, preset_index, icons, ui_state)
        }
        ToolBehavior::Decal { .. } => decal_options::draw_decal_tool_options(
            ui,
            l10n,
            state,
            render_resources,
            image_assets,
            selected_decal_asset_id,
            &mut ui_state.image_asset_thumbnails,
            shortcut_profile,
        ),
        ToolBehavior::Fill { scope } => {
            fill_options::draw_fill_tool_options(ui, l10n, state, scope)
        }
        ToolBehavior::Shape { .. } => shape_options::draw_shape_tool_options(ui, l10n, state),
        ToolBehavior::Selection { .. } => {
            selection_options::draw_selection_tool_options(ui, l10n, state, actions)
        }
        ToolBehavior::Transform => {
            transform_options::draw_transform_tool_options(ui, l10n, state, shortcut_profile)
        }
        ToolBehavior::ColorPicker => {
            color_picker_options::draw_color_picker_tool_options(ui, l10n, state)
        }
    }
}
