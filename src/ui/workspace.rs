mod persistence;
mod storage;
pub(crate) use persistence::{WorkspaceFileV1, WorkspaceWindowV1};
pub use storage::{StartupWorkspace, load_startup_workspace};

use eframe::egui;
use egui_tiles::{
    Container, ContainerKind, SimplificationOptions, TabState, Tile, TileId, Tiles, Tree,
    UiResponse,
};

use crate::{
    application::{AppState, EditorActionContext},
    core::image_asset::ImageAssetCatalog,
    localization::Localization,
    ui::{
        adjustment_editor::{self, AdjustmentEditorState},
        icons::UiIconRegistry,
        input::{shortcut_profile::ShortcutProfile, view_pointer::ViewPointerInputRouter},
        panels::{color_panel, layer_panel, materials_panel, mesh_panel, toolbox},
        render_resources::UiRenderResources,
        tool_options,
        view_output::ViewOutput,
        views::{uv_view, viewport_3d},
    },
};

const PANEL_INNER_MARGIN: i8 = 8;
const TOOL_GROUP_SIDE_PANEL_WIDTH: f32 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspacePane {
    ToolList,
    ToolProperties,
    Viewport3d,
    UvView,
    Color,
    MaterialsTextures,
    Meshes,
    Layers,
}

impl WorkspacePane {
    pub const ALL: [Self; 8] = [
        Self::ToolList,
        Self::ToolProperties,
        Self::Viewport3d,
        Self::UvView,
        Self::Color,
        Self::MaterialsTextures,
        Self::Meshes,
        Self::Layers,
    ];

    pub fn title_key(self) -> &'static str {
        match self {
            Self::ToolList => "workspace-tool-list",
            Self::ToolProperties => "workspace-tool-properties",
            Self::Viewport3d => "workspace-viewport-3d",
            Self::UvView => "workspace-uv-view",
            Self::Color => "workspace-color",
            Self::MaterialsTextures => "workspace-materials-textures",
            Self::Meshes => "workspace-meshes",
            Self::Layers => "workspace-layers",
        }
    }
}

pub struct WorkspaceDrawInput<'a> {
    pub localization: &'a Localization,
    pub state: &'a AppState,
    pub action_context: EditorActionContext,
    pub focused_texture_size: Option<[usize; 2]>,
    pub max_texture_dimension_2d: Option<u32>,
    pub render_resources: &'a UiRenderResources,
    pub image_assets: &'a ImageAssetCatalog,
    pub selected_decal_asset_id: Option<&'a str>,
    pub pointer_input: &'a mut ViewPointerInputRouter,
    pub icons: &'a UiIconRegistry,
    pub interaction_enabled: bool,
    pub shortcut_profile: &'a ShortcutProfile,
    pub shortcut_profile_revision: u64,
}

pub struct WorkspaceState {
    tree: Tree<WorkspacePane>,
    layer_panel: layer_panel::LayerPanelUiState,
    adjustment_editor: AdjustmentEditorState,
    tool_options: tool_options::ToolOptionsUiState,
    toolbox: toolbox::ToolboxUiState,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        WorkspaceFileV1::builtin()
            .and_then(|workspace| workspace.build_state())
            .expect("embedded default workspace must be valid")
    }
}

impl WorkspaceState {
    pub fn collapsed_layer_group_ids(&self) -> Vec<crate::core::surface::LayerId> {
        self.layer_panel.collapsed_group_ids()
    }

    pub fn restore_collapsed_layer_groups(
        &mut self,
        layer_ids: impl IntoIterator<Item = crate::core::surface::LayerId>,
    ) {
        self.layer_panel.restore_collapsed_groups(layer_ids);
    }

    pub fn single_selected_copy_target(
        &self,
        state: &AppState,
    ) -> Option<crate::core::surface::LayerId> {
        let document = state.document()?;
        let layer_id = self
            .layer_panel
            .single_selected_layer(&document.layer_tree, state.active_layer_id())?;
        let copyable = match state.active_layer_target() {
            crate::core::document::ActiveLayerTarget::Raster => {
                document.layer_tree.is_paintable(layer_id)
            }
            crate::core::document::ActiveLayerTarget::LayerMask => {
                document.layer_tree.has_layer_mask(layer_id)
            }
            crate::core::document::ActiveLayerTarget::SolidFill
            | crate::core::document::ActiveLayerTarget::Adjustment
            | crate::core::document::ActiveLayerTarget::EmbeddedImage
            | crate::core::document::ActiveLayerTarget::Structure => false,
        };
        copyable.then_some(layer_id)
    }

    pub fn open_adjustment_editor(
        &mut self,
        layer_id: crate::core::surface::LayerId,
        document_generation: u64,
    ) {
        self.adjustment_editor
            .open_for(layer_id, document_generation);
    }

    pub fn draw_adjustment_editor(
        &mut self,
        ctx: &egui::Context,
        l10n: &Localization,
        state: &AppState,
    ) -> ViewOutput {
        adjustment_editor::draw_adjustment_editor_window(
            ctx,
            l10n,
            state,
            &mut self.adjustment_editor,
        )
    }

    pub fn reset_layout(&mut self) {
        *self = Self::default();
    }

    pub fn pane_visible(&self, pane: WorkspacePane) -> bool {
        self.tile_id_for_pane(pane)
            .is_some_and(|tile_id| self.tile_and_ancestors_visible(tile_id))
    }

    pub fn set_pane_visible(&mut self, pane: WorkspacePane, visible: bool) {
        if let Some(tile_id) = self.tile_id_for_pane(pane) {
            if visible {
                self.tree.set_visible(tile_id, true);
                if let Some(tab_id) = self.tab_parent_for_pane_tile(tile_id) {
                    self.tree.set_visible(tab_id, true);
                }
                self.tree.make_active(|id, _tile| id == tile_id);
            } else {
                let hidden_tile_id = self.tile_id_to_hide_for_pane(tile_id);
                self.tree.set_visible(hidden_tile_id, false);
            }
        }
    }

    pub fn draw_tool_group_side_panel(
        &mut self,
        root_ui: &mut egui::Ui,
        l10n: &Localization,
        state: &AppState,
        action_context: EditorActionContext,
        icons: &UiIconRegistry,
        shortcut_profile: &ShortcutProfile,
    ) -> ViewOutput {
        let mut output = ViewOutput::default();
        egui::Panel::left("tool_group_side_panel")
            .resizable(false)
            .exact_size(TOOL_GROUP_SIDE_PANEL_WIDTH)
            .frame(
                egui::Frame::side_top_panel(root_ui.style())
                    .inner_margin(egui::Margin::same(PANEL_INNER_MARGIN)),
            )
            .show(root_ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("tool_group_side_panel_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        output.extend(toolbox::draw_tool_groups(
                            ui,
                            l10n,
                            state,
                            action_context,
                            icons,
                            shortcut_profile,
                            &mut self.toolbox,
                        ));
                        let scroll_rect = ui.clip_rect();
                        toolbox::update_group_drag_auto_scroll(ui, scroll_rect, &self.toolbox);
                    });
            });
        output
    }

    pub fn draw(&mut self, ui: &mut egui::Ui, input: WorkspaceDrawInput<'_>) -> ViewOutput {
        self.normalize_tab_children_to_panes();

        let tree = &mut self.tree;
        let layer_panel = &mut self.layer_panel;
        let tool_options = &mut self.tool_options;
        let toolbox = &mut self.toolbox;
        let mut behavior = WorkspaceBehavior {
            input,
            layer_panel,
            tool_options,
            toolbox,
            output: ViewOutput::default(),
        };
        tree.ui(&mut behavior, ui);

        behavior
            .output
            .extend(tool_options::stroke_options::draw_brush_settings_window(
                ui.ctx(),
                behavior.input.localization,
                behavior.input.state,
                behavior.input.icons,
                behavior.tool_options,
            ));

        let output = std::mem::take(&mut behavior.output);
        drop(behavior);

        self.normalize_tab_children_to_panes();

        output
    }

    fn normalize_tab_children_to_panes(&mut self) {
        loop {
            let tab_ids = self.tab_container_ids();
            let mut changed = false;

            for tab_id in tab_ids {
                let Some((children, active, containers_to_remove)) =
                    self.normalized_tab_children(tab_id)
                else {
                    continue;
                };

                if let Some(Tile::Container(Container::Tabs(tabs))) =
                    self.tree.tiles.get_mut(tab_id)
                {
                    tabs.children = children;
                    tabs.active = active;
                }

                for container_id in containers_to_remove {
                    self.tree.tiles.remove(container_id);
                }

                changed = true;
            }

            if !changed {
                break;
            }
        }
    }

    fn tab_container_ids(&self) -> Vec<TileId> {
        self.tree
            .tiles
            .iter()
            .filter_map(|(&tile_id, tile)| {
                (tile.kind() == Some(ContainerKind::Tabs)).then_some(tile_id)
            })
            .collect()
    }

    fn normalized_tab_children(
        &self,
        tab_id: TileId,
    ) -> Option<(Vec<TileId>, Option<TileId>, Vec<TileId>)> {
        let tabs = match self.tree.tiles.get_container(tab_id)? {
            Container::Tabs(tabs) => tabs,
            _ => return None,
        };

        let original_active = tabs.active;
        let mut children = Vec::with_capacity(tabs.children.len());
        let mut active = original_active;
        let mut containers_to_remove = Vec::new();
        let mut changed = false;

        for child_id in tabs.children.iter().copied() {
            match self.tree.tiles.get(child_id) {
                Some(Tile::Pane(_)) => children.push(child_id),
                Some(Tile::Container(container)) => {
                    let promoted_children = container.children_vec();

                    if original_active == Some(child_id) {
                        active = self.first_visible_child(&promoted_children);
                    }

                    children.extend(promoted_children);
                    containers_to_remove.push(child_id);
                    changed = true;
                }
                None => {
                    if original_active == Some(child_id) {
                        active = None;
                    }
                    changed = true;
                }
            }
        }

        if !changed {
            return None;
        }

        active = active
            .filter(|active_id| children.contains(active_id))
            .or_else(|| self.first_visible_child(&children));

        Some((children, active, containers_to_remove))
    }

    fn first_visible_child(&self, children: &[TileId]) -> Option<TileId> {
        children
            .iter()
            .copied()
            .find(|&child_id| self.tree.is_visible(child_id))
            .or_else(|| children.first().copied())
    }

    fn tile_id_for_pane(&self, pane: WorkspacePane) -> Option<TileId> {
        self.tree.tiles.find_pane(&pane)
    }

    fn tile_and_ancestors_visible(&self, tile_id: TileId) -> bool {
        let mut current = Some(tile_id);
        while let Some(id) = current {
            if !self.tree.is_visible(id) {
                return false;
            }
            current = self.tree.tiles.parent_of(id);
        }
        true
    }

    fn tile_id_to_hide_for_pane(&self, pane_tile_id: TileId) -> TileId {
        if let Some(tab_id) = self.tab_parent_for_pane_tile(pane_tile_id) {
            if !self.tab_has_other_visible_child(tab_id, pane_tile_id) {
                return tab_id;
            }
        }
        pane_tile_id
    }

    fn tab_parent_for_pane_tile(&self, pane_tile_id: TileId) -> Option<TileId> {
        let parent_id = self.tree.tiles.parent_of(pane_tile_id)?;
        let parent = self.tree.tiles.get_container(parent_id)?;
        (parent.kind() == ContainerKind::Tabs).then_some(parent_id)
    }

    fn tab_has_other_visible_child(&self, tab_id: TileId, pane_tile_id: TileId) -> bool {
        self.tree.tiles.get_container(tab_id).is_some_and(|tab| {
            tab.children().any(|&child_id| {
                child_id != pane_tile_id && self.tile_and_ancestors_visible(child_id)
            })
        })
    }
}

struct WorkspaceBehavior<'a, 'b> {
    input: WorkspaceDrawInput<'a>,
    layer_panel: &'b mut layer_panel::LayerPanelUiState,
    tool_options: &'b mut tool_options::ToolOptionsUiState,
    toolbox: &'b mut toolbox::ToolboxUiState,
    output: ViewOutput,
}

fn pane_uses_inner_margin(pane: WorkspacePane) -> bool {
    !matches!(
        pane,
        WorkspacePane::Viewport3d
            | WorkspacePane::UvView
            | WorkspacePane::MaterialsTextures
            | WorkspacePane::Layers
    )
}

impl WorkspaceBehavior<'_, '_> {
    fn draw_pane_content(&mut self, ui: &mut egui::Ui, pane: WorkspacePane) -> ViewOutput {
        match pane {
            WorkspacePane::ToolList => toolbox::draw_tool_list(
                ui,
                self.input.localization,
                self.input.state,
                self.input.action_context,
                self.input.render_resources,
                self.input.icons,
                self.toolbox,
            ),
            WorkspacePane::ToolProperties => draw_tool_properties(
                ui,
                self.input.localization,
                self.input.state,
                self.input.action_context,
                self.input.render_resources,
                self.input.image_assets,
                self.input.selected_decal_asset_id,
                self.input.icons,
                self.tool_options,
                self.input.shortcut_profile,
            ),
            WorkspacePane::Viewport3d => viewport_3d::draw_viewport_3d(
                ui,
                self.input.localization,
                self.input.state,
                self.input.render_resources,
                &mut *self.input.pointer_input,
                self.input.icons,
                self.input.interaction_enabled,
                self.input.shortcut_profile,
                self.input.shortcut_profile_revision,
            ),
            WorkspacePane::UvView => uv_view::draw_uv_view(
                ui,
                self.input.localization,
                self.input.state,
                self.input.render_resources,
                &mut *self.input.pointer_input,
                self.input.icons,
                self.input.interaction_enabled,
                self.input.shortcut_profile,
                self.input.shortcut_profile_revision,
            ),
            WorkspacePane::Color => color_panel::draw_color_panel(
                ui,
                self.input.localization,
                self.input.state,
                self.input.icons,
            ),
            WorkspacePane::MaterialsTextures => materials_panel::draw_materials_textures_panel(
                ui,
                self.input.localization,
                self.input.state,
                self.input.focused_texture_size,
                self.input.max_texture_dimension_2d,
                self.input.icons,
            ),
            WorkspacePane::Meshes => mesh_panel::draw_mesh_panel(
                ui,
                self.input.localization,
                self.input.state,
                self.input.icons,
            ),
            WorkspacePane::Layers => layer_panel::draw_layer_panel(
                ui,
                self.input.localization,
                self.input.state,
                self.input.icons,
                self.layer_panel,
            ),
        }
    }
}

impl egui_tiles::Behavior<WorkspacePane> for WorkspaceBehavior<'_, '_> {
    fn tab_title_for_pane(&mut self, pane: &WorkspacePane) -> egui::WidgetText {
        self.input.localization.text(pane.title_key()).into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: TileId,
        pane: &mut WorkspacePane,
    ) -> UiResponse {
        let pane_output = if pane_uses_inner_margin(*pane) {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(PANEL_INNER_MARGIN))
                .show(ui, |ui| self.draw_pane_content(ui, *pane))
                .inner
        } else {
            self.draw_pane_content(ui, *pane)
        };
        self.output.extend(pane_output);
        Default::default()
    }

    fn min_size(&self) -> f32 {
        48.0
    }

    fn is_tile_draggable(&self, tiles: &Tiles<WorkspacePane>, tile_id: TileId) -> bool {
        tiles.get_pane(&tile_id).is_some()
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            prune_single_child_tabs: false,
            ..Default::default()
        }
    }

    fn tab_bg_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<WorkspacePane>,
        _tile_id: TileId,
        _state: &TabState,
    ) -> egui::Color32 {
        visuals.panel_fill
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<WorkspacePane>,
        _tile_id: TileId,
        _state: &TabState,
    ) -> egui::Stroke {
        egui::Stroke::NONE
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::NONE
    }

    fn tab_text_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &Tiles<WorkspacePane>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Color32 {
        if state.active {
            visuals.strong_text_color()
        } else {
            visuals.weak_text_color()
        }
    }
}

fn draw_tool_properties(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    action_context: EditorActionContext,
    render_resources: &UiRenderResources,
    image_assets: &ImageAssetCatalog,
    selected_decal_asset_id: Option<&str>,
    icons: &UiIconRegistry,
    ui_state: &mut tool_options::ToolOptionsUiState,
    shortcut_profile: &ShortcutProfile,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    egui::ScrollArea::vertical()
        .id_salt("workspace_tool_properties_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            output.extend(tool_options::draw_tool_options(
                ui,
                l10n,
                state,
                action_context,
                render_resources,
                image_assets,
                selected_decal_asset_id,
                icons,
                ui_state,
                shortcut_profile,
            ));
        });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_layout_restores_embedded_default() {
        let mut state = WorkspaceState::default();
        state.set_pane_visible(WorkspacePane::Meshes, false);
        state.set_pane_visible(WorkspacePane::Layers, false);

        state.reset_layout();

        let builtin = WorkspaceFileV1::builtin().expect("builtin workspace");
        let captured =
            WorkspaceFileV1::capture(&state, &builtin.window).expect("capture reset workspace");
        assert_eq!(captured.root, builtin.root);
    }

    #[test]
    fn hiding_one_scene_list_pane_keeps_the_other_visible() {
        let mut state = WorkspaceState::default();

        state.set_pane_visible(WorkspacePane::MaterialsTextures, false);
        assert!(!state.pane_visible(WorkspacePane::MaterialsTextures));
        assert!(state.pane_visible(WorkspacePane::Meshes));

        state.set_pane_visible(WorkspacePane::MaterialsTextures, true);
        assert!(state.pane_visible(WorkspacePane::MaterialsTextures));
        assert!(state.pane_visible(WorkspacePane::Meshes));
    }
}
