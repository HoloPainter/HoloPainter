use std::collections::{BTreeSet, HashMap, HashSet};

use eframe::egui;

use super::{color_panel::draw_color_panel_content, material_swatch::paint_material_swatch};
use crate::{
    application::{AppState, Command},
    core::{
        adjustment::{Adjustment, AdjustmentKind},
        composite::{GroupCompositeMode, LayerBlendMode},
        document::{ActiveLayerTarget, Document, MaterialData, MaterialId, MaterialUiColor},
        document_tile_store::SurfaceThumbnailMode,
        surface::{
            LayerContent, LayerId, LayerMaskInitMode, LayerMaskProperties, LayerMaterialMask,
            LayerTree, PaintSurfaceId,
        },
    },
    localization::Localization,
    ui::{
        icons::UiIconRegistry,
        view_output::{ScreenEyedropperTarget, UiRequest, ViewOutput},
    },
};

const LAYER_PANEL_HEADER_MARGIN: i8 = 6;
const HEADER_BUTTON_SIZE: f32 = 30.0;
const HEADER_ICON_SIZE: f32 = 22.0;
const HEADER_BUTTON_GAP: f32 = 0.0;
const FULL_HEADER_BUTTON_COUNT: usize = 7;
const COMPACT_HEADER_BUTTON_COUNT: usize = 5;
const COMPOSITE_CONTROLS_WIDE_WIDTH: f32 = 250.0;
const ADD_LAYER_POPUP_WIDTH: f32 = 190.0;
const ADD_MASK_POPUP_WIDTH: f32 = 190.0;
const ROW_INDENT: f32 = 14.0;
const ROW_THUMBNAIL_SIZE: f32 = 28.0;
const ROW_MASK_THUMBNAIL_SIZE: f32 = 28.0;
const ROW_FILL_BADGE_SIZE: f32 = 11.0;
const ROW_FILL_BADGE_MARGIN: f32 = 1.0;
const ROW_ICON_SIZE: f32 = 22.0;
const ROW_ICON_VISIBILITY_SIZE: f32 = 20.0;
const ROW_BUTTON_SIZE: f32 = 28.0;
const ROW_BUTTON_VISIBILITY_SIZE: f32 = 22.0;
const ROW_MIN_HEIGHT: f32 = 38.0;
const ROW_VISIBILITY_COLUMN_WIDTH: f32 = 24.0;
const ROW_LOCK_COLUMN_WIDTH: f32 = 18.0;
const ROW_LOCK_ICON_SIZE: f32 = 16.0;
const ROW_MATERIAL_MASK_COLUMN_WIDTH: f32 = 34.0;
const ROW_MATERIAL_MASK_BADGE_SIZE: f32 = 24.0;
const ROW_MATERIAL_MASK_RING_WIDTH: f32 = 4.0;
const ROW_MATERIAL_MASK_RING_SEGMENTS: usize = 48;
const ROW_MATERIAL_MASK_MAX_SWATCHES: usize = 8;
const MATERIAL_MASK_POPUP_WIDTH: f32 = 280.0;
const MATERIAL_MASK_POPUP_MAX_LIST_HEIGHT: f32 = 360.0;
const MATERIAL_MASK_POPUP_SWATCH_SIZE: f32 = 12.0;
const ROW_TREE_GAP: f32 = 4.0;
const ROW_CONTENT_GAP: f32 = 4.0;
const ROW_CORNER_RADIUS: f32 = 0.0;
const LAYER_DRAG_AUTOSCROLL_EDGE: f32 = 28.0;
const LAYER_DRAG_AUTOSCROLL_SPEED: f32 = 12.0;

const ICON_ADD_LAYER: &str = "builtin.icon.new_layer";
const ICON_ADD_GROUP: &str = "builtin.icon.new_folder";
const ICON_SOLID_FILL: &str = "builtin.icon.colors";
const ICON_ADJUSTMENT_BRIGHTNESS_CONTRAST: &str = "builtin.icon.adjustment_brightness_contrast";
const ICON_ADJUSTMENT_LEVELS: &str = "builtin.icon.adjustment_levels";
const ICON_ADJUSTMENT_HSV: &str = "builtin.icon.adjustment_hsv";
const ICON_ADJUSTMENT_CURVES: &str = "builtin.icon.adjustment_curves";
const ICON_ADJUSTMENT_INVERT: &str = "builtin.icon.adjustment_invert";
const ICON_ADJUSTMENT_GRADIENT_MAP: &str = "builtin.icon.adjustment_gradient_map";
const ICON_UV_MIRROR: &str = "builtin.icon.flip";
const ICON_DUPLICATE_LAYER: &str = "builtin.icon.content_copy";
const ICON_MERGE_LAYER: &str = "builtin.icon.layer_merge";
const ICON_ADD_MASK: &str = "builtin.icon.square_dot";
const ICON_DELETE: &str = "builtin.icon.delete";
const ICON_LOCK: &str = "builtin.icon.lock";
const ICON_VISIBILITY: &str = "builtin.icon.visibility";
const ICON_VISIBILITY_OFF: &str = "builtin.icon.visibility_off";
const ICON_FOLDER: &str = "builtin.icon.folder";
const ICON_FOLDER_OPEN: &str = "builtin.icon.folder_open";
const ICON_IMAGE: &str = "builtin.icon.image";

#[derive(Default)]
pub struct LayerPanelUiState {
    collapsed_groups: HashSet<LayerId>,
    selected_layer_ids: HashSet<LayerId>,
    selection_anchor: Option<LayerId>,
    drag: Option<LayerPanelDragState>,
    rename: Option<LayerRenameState>,
    solid_fill_color_popup: Option<LayerId>,
    solid_fill_color_session: Option<SolidFillColorSession>,
    next_solid_fill_edit_session: u64,
    thumbnails: LayerThumbnailCache,
}

impl LayerPanelUiState {
    pub(crate) fn collapsed_group_ids(&self) -> Vec<LayerId> {
        self.collapsed_groups.iter().copied().collect()
    }

    pub(crate) fn restore_collapsed_groups(
        &mut self,
        layer_ids: impl IntoIterator<Item = LayerId>,
    ) {
        self.collapsed_groups = layer_ids.into_iter().collect();
    }

    pub(crate) fn single_selected_layer(
        &self,
        tree: &LayerTree,
        active_layer: LayerId,
    ) -> Option<LayerId> {
        if self.selected_layer_ids.is_empty() {
            return (tree.contains(active_layer) && active_layer != tree.root())
                .then_some(active_layer);
        }
        let mut selected = self
            .selected_layer_ids
            .iter()
            .copied()
            .filter(|layer_id| tree.contains(*layer_id) && *layer_id != tree.root());
        let layer_id = selected.next()?;
        selected.next().is_none().then_some(layer_id)
    }

    fn prune(&mut self, tree: &LayerTree, active_layer: LayerId) {
        self.collapsed_groups
            .retain(|layer_id| tree.contains(*layer_id) && tree.is_group(*layer_id));
        self.selected_layer_ids
            .retain(|layer_id| tree.contains(*layer_id) && *layer_id != tree.root());
        if tree.contains(active_layer) && active_layer != tree.root() {
            if self.selected_layer_ids.is_empty() {
                self.selected_layer_ids.insert(active_layer);
            } else if !self.selected_layer_ids.contains(&active_layer) {
                self.selected_layer_ids.clear();
                self.selected_layer_ids.insert(active_layer);
                self.selection_anchor = Some(active_layer);
            }
        }
        if self
            .selection_anchor
            .is_some_and(|layer_id| !tree.contains(layer_id))
        {
            self.selection_anchor = Some(active_layer);
        }
        if self.drag.as_ref().is_some_and(|drag| match drag {
            LayerPanelDragState::Layers(drag) => drag
                .layer_ids
                .iter()
                .any(|layer_id| !tree.contains(*layer_id)),
            LayerPanelDragState::LayerMask(drag) => {
                !tree.contains(drag.source_layer_id) || !tree.has_layer_mask(drag.source_layer_id)
            }
        }) {
            self.drag = None;
        }
        if let Some(rename) = &self.rename
            && !tree.contains(rename.layer_id)
        {
            self.rename = None;
        }
        if self
            .solid_fill_color_popup
            .is_some_and(|layer_id| !tree.is_solid_fill(layer_id))
        {
            self.solid_fill_color_popup = None;
        }
        if let Some(session) = self.solid_fill_color_session
            && !tree.is_solid_fill(session.layer_id)
        {
            self.solid_fill_color_session = None;
        }
    }

    fn toggle_solid_fill_color_popup(&mut self, layer_id: LayerId) {
        if self.solid_fill_color_popup == Some(layer_id) {
            self.solid_fill_color_popup = None;
            return;
        }

        self.solid_fill_color_popup = Some(layer_id);
        self.next_solid_fill_edit_session =
            self.next_solid_fill_edit_session.wrapping_add(1).max(1);
        self.solid_fill_color_session = Some(SolidFillColorSession {
            layer_id,
            edit_session: self.next_solid_fill_edit_session,
        });
    }
}

#[derive(Debug, Clone, Copy)]
struct SolidFillColorSession {
    layer_id: LayerId,
    edit_session: u64,
}

#[derive(Debug, Clone)]
struct LayerRenameState {
    layer_id: LayerId,
    buffer: String,
    focus_requested: bool,
}

#[derive(Debug, Clone)]
enum LayerPanelDragState {
    Layers(LayerDragState),
    LayerMask(LayerMaskDragState),
}

#[derive(Debug, Clone)]
struct LayerDragState {
    layer_ids: Vec<LayerId>,
    drop_target: Option<LayerDropTarget>,
}

#[derive(Debug, Clone, Copy)]
struct LayerMaskDragState {
    source_layer_id: LayerId,
    drop_target: Option<LayerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LayerDropTarget {
    target_id: LayerId,
    placement: DropPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropPlacement {
    Above,
    Below,
    Into,
}

#[derive(Debug, Clone)]
struct LayerRowVm {
    layer_id: LayerId,
    depth: usize,
    content: LayerContent,
    name: String,
    visible: bool,
    hidden_by_parent: bool,
    locked: bool,
    locked_by_parent: bool,
    opacity: f32,
    blend_mode: LayerBlendMode,
    group_composite_mode: GroupCompositeMode,
    mask: Option<LayerMaskProperties>,
    material_mask: LayerMaterialMask,
    collapsed: bool,
    child_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterialMaskSummary {
    specified: bool,
    specified_count: usize,
    representative_colors: Vec<MaterialUiColor>,
    effective_count: usize,
    show_as_unrestricted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerPanelCompositeMode {
    PassThrough,
    Blend(LayerBlendMode),
}

impl LayerPanelCompositeMode {
    fn localized_label(self, l10n: &Localization) -> String {
        match self {
            Self::PassThrough => group_composite_mode_label(l10n, GroupCompositeMode::PassThrough),
            Self::Blend(mode) => layer_blend_mode_label(l10n, mode),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteAction {
    Layer,
    Mask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayerToolbarLayout {
    Full,
    Compact,
    OverflowOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositeControlsLayout {
    Horizontal,
    Vertical,
}

fn header_buttons_width(button_count: usize) -> f32 {
    HEADER_BUTTON_SIZE * button_count as f32
        + HEADER_BUTTON_GAP * button_count.saturating_sub(1) as f32
}

fn layer_toolbar_layout(available_width: f32) -> LayerToolbarLayout {
    if available_width >= header_buttons_width(FULL_HEADER_BUTTON_COUNT) {
        LayerToolbarLayout::Full
    } else if available_width >= header_buttons_width(COMPACT_HEADER_BUTTON_COUNT) {
        LayerToolbarLayout::Compact
    } else {
        LayerToolbarLayout::OverflowOnly
    }
}

fn composite_controls_layout(available_width: f32) -> CompositeControlsLayout {
    if available_width >= COMPOSITE_CONTROLS_WIDE_WIDTH {
        CompositeControlsLayout::Horizontal
    } else {
        CompositeControlsLayout::Vertical
    }
}

struct LayerSelectionContext {
    selected_layers: Vec<LayerId>,
    effective_roots: Vec<LayerId>,
    all_locally_locked: bool,
    can_duplicate: bool,
    can_merge: bool,
    can_delete: bool,
}

impl LayerSelectionContext {
    fn new(tree: &LayerTree, selected_layer_ids: &HashSet<LayerId>) -> Self {
        let selected_layers = tree
            .rows()
            .into_iter()
            .map(|row| row.layer_id)
            .filter(|layer_id| selected_layer_ids.contains(layer_id))
            .filter(|layer_id| *layer_id != tree.root())
            .collect::<Vec<_>>();
        let effective_roots = effective_selection_roots(tree, selected_layer_ids);
        let all_locally_locked = !selected_layers.is_empty()
            && selected_layers
                .iter()
                .all(|layer_id| tree.is_locked(*layer_id));
        let can_duplicate = !effective_roots.is_empty() && !effective_roots.contains(&tree.root());
        let can_merge = tree.layer_merge_plan(&effective_roots).is_some();
        let can_delete = can_delete_layers(tree, &effective_roots);
        Self {
            selected_layers,
            effective_roots,
            all_locally_locked,
            can_duplicate,
            can_merge,
            can_delete,
        }
    }

    fn is_multi_selection(&self) -> bool {
        self.selected_layers.len() > 1
    }

    fn duplicate_command(&self, primary_layer_id: LayerId) -> Option<Command> {
        self.can_duplicate.then(|| {
            if self.effective_roots.len() == 1 {
                Command::DuplicateLayer {
                    layer_id: self.effective_roots[0],
                }
            } else {
                Command::DuplicateLayers {
                    layer_ids: self.effective_roots.clone(),
                    primary_layer_id,
                }
            }
        })
    }

    fn merge_command(&self) -> Option<Command> {
        self.can_merge.then(|| Command::MergeLayers {
            layer_ids: self.effective_roots.clone(),
        })
    }

    fn lock_command(&self) -> Option<Command> {
        let locked = !self.all_locally_locked;
        match self.selected_layers.as_slice() {
            [] => None,
            [layer_id] => Some(Command::SetLayerLocked {
                layer_id: *layer_id,
                locked,
            }),
            layer_ids => Some(Command::SetLayersLocked {
                layer_ids: layer_ids.to_vec(),
                locked,
            }),
        }
    }

    fn delete_command(&self) -> Option<Command> {
        self.can_delete.then(|| {
            if self.effective_roots.len() == 1 {
                Command::DeleteLayer {
                    layer_id: self.effective_roots[0],
                }
            } else {
                Command::DeleteLayers {
                    layer_ids: self.effective_roots.clone(),
                }
            }
        })
    }
}

struct LayerRowResponses {
    drag_source: egui::Response,
    layer_context: Vec<egui::Response>,
    mask: Option<(LayerMaskProperties, egui::Response)>,
}

#[derive(Default)]
struct LayerThumbnailCache {
    generation: u64,
    entries: HashMap<LayerThumbnailCacheKey, LayerThumbnailEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LayerThumbnailCacheKey {
    surface: PaintSurfaceId,
    mode: SurfaceThumbnailMode,
}

struct LayerThumbnailEntry {
    surface_revision: u64,
    image_size: [usize; 2],
    texture: egui::TextureHandle,
}

impl LayerThumbnailCache {
    fn clear(&mut self) {
        self.entries.clear();
    }

    fn invalidate_surface(&mut self, surface: PaintSurfaceId) {
        self.entries.retain(|key, _| key.surface != surface);
    }

    fn prune(
        &mut self,
        document_generation: u64,
        focused_material_index: usize,
        rows: &[LayerRowVm],
    ) {
        if self.generation != document_generation {
            self.entries.clear();
            self.generation = document_generation;
        }

        let mut live_keys = HashSet::new();
        for row in rows {
            if matches!(row.content, LayerContent::Raster) {
                live_keys.insert(LayerThumbnailCacheKey {
                    surface: PaintSurfaceId::raster(focused_material_index.into(), row.layer_id),
                    mode: SurfaceThumbnailMode::Rgba,
                });
            }
            if row.mask.is_some() {
                live_keys.insert(LayerThumbnailCacheKey {
                    surface: PaintSurfaceId::layer_mask(
                        focused_material_index.into(),
                        row.layer_id,
                    ),
                    mode: SurfaceThumbnailMode::MaskRedAsGray,
                });
            }
        }
        self.entries.retain(|key, _| live_keys.contains(key));
    }

    fn texture_id_for_surface(
        &mut self,
        ctx: &egui::Context,
        document: &Document,
        surface: PaintSurfaceId,
        mode: SurfaceThumbnailMode,
        max_extent: u32,
    ) -> Option<egui::TextureId> {
        let surface_revision = document.tiles.surface_revision(surface)?;
        let key = LayerThumbnailCacheKey { surface, mode };
        let image_size = [max_extent as usize, max_extent as usize];
        let needs_update = self.entries.get(&key).is_none_or(|entry| {
            entry.surface_revision != surface_revision || entry.image_size != image_size
        });

        if needs_update {
            let thumbnail = document
                .tiles
                .read_surface_thumbnail(surface, max_extent, mode)
                .ok()?;
            let image =
                egui::ColorImage::from_rgba_unmultiplied(thumbnail.image_size, &thumbnail.rgba8);
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.texture.set(image, egui::TextureOptions::LINEAR);
                entry.surface_revision = thumbnail.surface_revision;
                entry.image_size = thumbnail.image_size;
            } else {
                let texture_name = format!(
                    "layer_panel_thumbnail_{:?}_{:?}_{}",
                    key.surface, key.mode, self.generation
                );
                let texture = ctx.load_texture(texture_name, image, egui::TextureOptions::LINEAR);
                self.entries.insert(
                    key,
                    LayerThumbnailEntry {
                        surface_revision: thumbnail.surface_revision,
                        image_size: thumbnail.image_size,
                        texture,
                    },
                );
            }
        }

        self.entries.get(&key).map(|entry| entry.texture.id())
    }
}

pub fn draw_layer_panel(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let Some(document) = state.document() else {
        ui_state.thumbnails.clear();
        egui::Frame::NONE
            .inner_margin(egui::Margin::same(LAYER_PANEL_HEADER_MARGIN))
            .show(ui, |ui| {
                ui.label(l10n.text("layer-panel-no-document"));
            });
        return output;
    };

    let active_layer = state.active_layer_id();
    let active_target = state.active_layer_target();
    let document_generation = state.document_generation();
    let focused_material_index = state.focused_material_index();
    let current_color = state.current_color();
    ui_state.prune(&document.layer_tree, active_layer);
    let rows = visible_rows_for_panel(&document.layer_tree, &ui_state.collapsed_groups);
    ui_state
        .thumbnails
        .prune(document_generation, focused_material_index, &rows);
    let ui_enabled = !state.is_document_edit_interacting();
    let mut command: Option<Command> = None;
    let mut ui_request: Option<UiRequest> = None;

    ui.add_enabled_ui(ui_enabled, |ui| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::same(LAYER_PANEL_HEADER_MARGIN))
            .show(ui, |ui| {
                draw_header(
                    ui,
                    l10n,
                    document,
                    active_layer,
                    active_target,
                    &ui_state.selected_layer_ids,
                    icons,
                    current_color,
                    &mut command,
                );
            });

        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("layer_panel_rows_scroll")
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, _viewport| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                let scroll_rect = ui.clip_rect();
                draw_rows(
                    ui,
                    l10n,
                    document,
                    &rows,
                    active_layer,
                    active_target,
                    focused_material_index,
                    icons,
                    ui_state,
                    &mut command,
                    &mut ui_request,
                    scroll_rect,
                );
            });
    });

    if !ui_enabled {
        egui::Frame::NONE
            .inner_margin(egui::Margin::same(LAYER_PANEL_HEADER_MARGIN))
            .show(ui, |ui| {
                ui.label(l10n.text("layer-panel-editing-disabled"));
            });
    }

    if let Some(command) = command {
        output.push(command);
        output.request_repaint();
    }
    if let Some(request) = ui_request {
        output.request_ui(request);
        output.request_repaint();
    }
    output
}

fn draw_header(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    selected_layer_ids: &HashSet<LayerId>,
    icons: &UiIconRegistry,
    current_color: [f32; 3],
    command: &mut Option<Command>,
) {
    let tree = &document.layer_tree;
    let active_node = tree.get(active_layer);
    let has_mask = tree.has_layer_mask(active_layer);
    let selection = LayerSelectionContext::new(tree, selected_layer_ids);
    let selected_layers = &selection.selected_layers;
    let is_multi_selection = selection.is_multi_selection();
    let can_duplicate_layer = selection.can_duplicate;
    let can_merge_layers = selection.can_merge;
    let can_add_mask = !is_multi_selection && tree.can_add_layer_mask(active_layer);
    let can_add_mask_from_selection = can_add_mask && document.active_selection.is_active();
    let all_locally_locked = selection.all_locally_locked;
    let parent_locked_only = selected_layers.len() == 1
        && !tree.is_locked(selected_layers[0])
        && tree.is_effectively_locked(selected_layers[0]);
    let delete_action =
        if !is_multi_selection && active_target == ActiveLayerTarget::LayerMask && has_mask {
            DeleteAction::Mask
        } else {
            DeleteAction::Layer
        };
    let can_delete = match delete_action {
        DeleteAction::Mask => !tree.is_effectively_locked(active_layer),
        DeleteAction::Layer => selection.can_delete,
    };
    let toolbar_layout = layer_toolbar_layout(ui.available_width());
    match toolbar_layout {
        LayerToolbarLayout::Full => {
            egui::Popup::close_id(ui.ctx(), egui::Id::new("layer_panel_header_overflow_popup"));
        }
        LayerToolbarLayout::Compact => {}
        LayerToolbarLayout::OverflowOnly => {
            egui::Popup::close_id(ui.ctx(), egui::Id::new("layer_panel_add_layer_type_popup"));
            egui::Popup::close_id(ui.ctx(), egui::Id::new("layer_panel_add_mask_type_popup"));
        }
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = HEADER_BUTTON_GAP;
        if toolbar_layout != LayerToolbarLayout::OverflowOnly {
            let popup_id = egui::Id::new("layer_panel_add_layer_type_popup");
            let popup_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
            let response = icon_button(
                ui,
                icons,
                ICON_ADD_LAYER,
                HEADER_BUTTON_SIZE,
                HEADER_ICON_SIZE,
                popup_open,
            )
            .on_hover_text(l10n.text("layer-panel-add-layer"));
            egui::Popup::menu(&response)
                .id(popup_id)
                .width(ADD_LAYER_POPUP_WIDTH)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    draw_add_layer_menu_contents(
                        ui,
                        l10n,
                        icons,
                        current_color,
                        Some(popup_id),
                        command,
                    );
                });
            if icon_button(
                ui,
                icons,
                ICON_ADD_GROUP,
                HEADER_BUTTON_SIZE,
                HEADER_ICON_SIZE,
                false,
            )
            .on_hover_text(l10n.text("layer-panel-add-group"))
            .clicked()
            {
                *command = Some(Command::AddGroup);
            }
        }
        if toolbar_layout == LayerToolbarLayout::Full {
            if ui
                .add_enabled_ui(can_duplicate_layer, |ui| {
                    icon_button(
                        ui,
                        icons,
                        ICON_DUPLICATE_LAYER,
                        HEADER_BUTTON_SIZE,
                        HEADER_ICON_SIZE,
                        false,
                    )
                    .on_hover_text(l10n.text("layer-panel-duplicate"))
                    .clicked()
                })
                .inner
            {
                *command = selection.duplicate_command(active_layer);
            }
            if ui
                .add_enabled_ui(can_merge_layers, |ui| {
                    icon_button(
                        ui,
                        icons,
                        ICON_MERGE_LAYER,
                        HEADER_BUTTON_SIZE,
                        HEADER_ICON_SIZE,
                        false,
                    )
                    .on_hover_text(l10n.text("layer-panel-merge"))
                    .clicked()
                })
                .inner
            {
                *command = selection.merge_command();
            }
            let lock_tint_alpha = if parent_locked_only { 96 } else { 255 };
            let lock_tooltip = if all_locally_locked {
                l10n.text("layer-panel-unlock-selected")
            } else if parent_locked_only {
                l10n.text("layer-panel-locked-by-parent-click")
            } else {
                l10n.text("layer-panel-lock-selected")
            };
            if ui
                .add_enabled_ui(!selected_layers.is_empty(), |ui| {
                    icon_button_with_tint_alpha(
                        ui,
                        icons,
                        ICON_LOCK,
                        HEADER_BUTTON_SIZE,
                        HEADER_ICON_SIZE,
                        all_locally_locked,
                        lock_tint_alpha,
                    )
                    .on_hover_text(lock_tooltip)
                    .clicked()
                })
                .inner
            {
                *command = selection.lock_command();
            }
        }
        if toolbar_layout != LayerToolbarLayout::OverflowOnly {
            let popup_id = egui::Id::new("layer_panel_add_mask_type_popup");
            if !can_add_mask {
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
            ui.add_enabled_ui(can_add_mask, |ui| {
                let popup_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
                let response = icon_button(
                    ui,
                    icons,
                    ICON_ADD_MASK,
                    HEADER_BUTTON_SIZE,
                    HEADER_ICON_SIZE,
                    popup_open,
                )
                .on_hover_text(l10n.text("layer-panel-add-mask"));
                egui::Popup::menu(&response)
                    .id(popup_id)
                    .width(ADD_MASK_POPUP_WIDTH)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        draw_add_mask_menu_contents(
                            ui,
                            l10n,
                            active_layer,
                            can_add_mask_from_selection,
                            Some(popup_id),
                            command,
                        );
                    });
            });
            let delete_hover = match delete_action {
                DeleteAction::Layer => l10n.text("layer-panel-delete-layer"),
                DeleteAction::Mask => l10n.text("layer-panel-delete-mask"),
            };
            if ui
                .add_enabled_ui(can_delete, |ui| {
                    icon_button(
                        ui,
                        icons,
                        ICON_DELETE,
                        HEADER_BUTTON_SIZE,
                        HEADER_ICON_SIZE,
                        false,
                    )
                    .on_hover_text(delete_hover)
                    .clicked()
                })
                .inner
            {
                *command = Some(match delete_action {
                    DeleteAction::Layer => selection
                        .delete_command()
                        .expect("enabled layer deletion must have a command"),
                    DeleteAction::Mask => Command::DeleteLayerMask {
                        layer_id: active_layer,
                    },
                });
            }
        }
        if toolbar_layout != LayerToolbarLayout::Full {
            draw_header_overflow_menu(
                ui,
                l10n,
                active_layer,
                &selection,
                icons,
                current_color,
                toolbar_layout == LayerToolbarLayout::OverflowOnly,
                can_add_mask,
                can_add_mask_from_selection,
                parent_locked_only,
                delete_action,
                can_delete,
                command,
            );
        }
    });

    ui.add_space(4.0);

    let controls_layout = composite_controls_layout(ui.available_width());
    let mut draw_controls = |ui: &mut egui::Ui| {
        let Some(active_node) = active_node else {
            ui.add_enabled(false, egui::Label::new(l10n.text("layer-panel-blend")));
            ui.add_enabled(false, egui::Label::new(l10n.text("field-opacity")));
            return;
        };

        let can_edit_blend = !selected_layers.is_empty()
            && selected_layers
                .iter()
                .all(|layer_id| tree.can_set_layer_blend_mode(*layer_id));
        let composite_mode = common_composite_mode(tree, selected_layers);
        let can_edit_composite = can_edit_blend && composite_mode.is_some();
        let can_edit_group_composite = !selected_layers.is_empty()
            && selected_layers
                .iter()
                .all(|layer_id| tree.can_set_group_composite_mode(*layer_id));
        let can_edit_opacity = !selected_layers.is_empty()
            && selected_layers
                .iter()
                .all(|layer_id| tree.can_set_layer_opacity(*layer_id));
        let response = ui.add_enabled_ui(!selected_layers.is_empty(), |ui| {
            let mut new_composite_mode = composite_mode.unwrap_or_else(|| {
                layer_panel_composite_mode(
                    active_node.props.blend_mode,
                    tree.group_composite_mode(active_layer)
                        .unwrap_or(GroupCompositeMode::Isolated),
                    matches!(active_node.content, LayerContent::Group { .. }),
                )
            });
            let mut combo = egui::ComboBox::from_id_salt("layer_blend_mode")
                .selected_text(new_composite_mode.localized_label(l10n));
            if controls_layout == CompositeControlsLayout::Vertical {
                combo = combo.width(ui.available_width());
            }
            combo.show_ui(ui, |ui| {
                ui.add_enabled_ui(can_edit_composite, |ui| {
                    if can_edit_group_composite {
                        ui.selectable_value(
                            &mut new_composite_mode,
                            LayerPanelCompositeMode::PassThrough,
                            group_composite_mode_label(l10n, GroupCompositeMode::PassThrough),
                        );
                    }
                    for mode in LayerBlendMode::ALL {
                        ui.selectable_value(
                            &mut new_composite_mode,
                            LayerPanelCompositeMode::Blend(mode),
                            layer_blend_mode_label(l10n, mode),
                        );
                    }
                });
            });
            if can_edit_composite && Some(new_composite_mode) != composite_mode {
                *command = Some(match new_composite_mode {
                    LayerPanelCompositeMode::PassThrough => {
                        if selected_layers.len() == 1 {
                            Command::SetLayerGroupCompositeMode {
                                layer_id: selected_layers[0],
                                mode: GroupCompositeMode::PassThrough,
                            }
                        } else {
                            Command::SetLayersGroupCompositeMode {
                                layer_ids: selected_layers.clone(),
                                mode: GroupCompositeMode::PassThrough,
                            }
                        }
                    }
                    LayerPanelCompositeMode::Blend(blend_mode) => {
                        if selected_layers.len() == 1 {
                            Command::SetLayerBlendMode {
                                layer_id: selected_layers[0],
                                blend_mode,
                            }
                        } else {
                            Command::SetLayersBlendMode {
                                layer_ids: selected_layers.clone(),
                                blend_mode,
                            }
                        }
                    }
                });
            }

            let mut new_opacity = active_node.effective_opacity();
            let opacity_response = ui.add_enabled_ui(can_edit_opacity, |ui| {
                let slider = egui::Slider::new(&mut new_opacity, 0.0..=1.0).show_value(true);
                if controls_layout == CompositeControlsLayout::Vertical {
                    ui.add_sized([ui.available_width(), ui.spacing().interact_size.y], slider)
                        .changed()
                } else {
                    ui.add(slider).changed()
                }
            });
            if opacity_response.inner {
                *command = Some(if selected_layers.len() == 1 {
                    Command::SetLayerOpacity {
                        layer_id: selected_layers[0],
                        opacity: new_opacity,
                    }
                } else {
                    Command::SetLayersOpacity {
                        layer_ids: selected_layers.clone(),
                        opacity: new_opacity,
                    }
                });
            }
            if !selected_layers.is_empty() && !can_edit_opacity {
                let tooltip = if selected_layers.iter().any(|layer_id| {
                    matches!(tree.adjustment(*layer_id), Some(Adjustment::UvMirror(_)))
                }) {
                    l10n.text("layer-panel-uv-mirror-opacity-fixed")
                } else {
                    l10n.text("layer-panel-locked-opacity")
                };
                opacity_response.response.on_hover_text(tooltip);
            }
        });
        if selected_layers.is_empty() {
            response
                .response
                .on_hover_text(l10n.text("layer-panel-select-for-composite"));
        } else if !can_edit_composite {
            response
                .response
                .on_hover_text(l10n.text("layer-panel-mixed-composite"));
        }
    };
    match controls_layout {
        CompositeControlsLayout::Horizontal => {
            ui.horizontal(&mut draw_controls);
        }
        CompositeControlsLayout::Vertical => {
            ui.vertical(&mut draw_controls);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_header_overflow_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    active_layer: LayerId,
    selection: &LayerSelectionContext,
    icons: &UiIconRegistry,
    current_color: [f32; 3],
    include_all_actions: bool,
    can_add_mask: bool,
    can_add_mask_from_selection: bool,
    parent_locked_only: bool,
    delete_action: DeleteAction,
    can_delete: bool,
    command: &mut Option<Command>,
) {
    let popup_id = egui::Id::new("layer_panel_header_overflow_popup");
    let popup_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let response = ui
        .add_sized(
            [HEADER_BUTTON_SIZE, HEADER_BUTTON_SIZE],
            egui::Button::new(egui::RichText::new("…").size(HEADER_ICON_SIZE)).selected(popup_open),
        )
        .on_hover_text(l10n.text("layer-panel-more-actions"));
    egui::Popup::menu(&response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if include_all_actions {
                ui.menu_button(l10n.text("layer-panel-add-layer"), |ui| {
                    draw_add_layer_menu_contents(
                        ui,
                        l10n,
                        icons,
                        current_color,
                        Some(popup_id),
                        command,
                    );
                });
                if ui.button(l10n.text("layer-panel-add-group")).clicked() {
                    *command = Some(Command::AddGroup);
                    close_header_menu(ui, Some(popup_id));
                }
                ui.separator();
            }

            if ui
                .add_enabled(
                    selection.can_duplicate,
                    egui::Button::new(l10n.text("layer-panel-duplicate")),
                )
                .clicked()
            {
                *command = selection.duplicate_command(active_layer);
                close_header_menu(ui, Some(popup_id));
            }
            if ui
                .add_enabled(
                    selection.can_merge,
                    egui::Button::new(l10n.text("layer-panel-merge")),
                )
                .clicked()
            {
                *command = selection.merge_command();
                close_header_menu(ui, Some(popup_id));
            }

            let lock_label = if selection.all_locally_locked {
                l10n.text("layer-panel-unlock-selected")
            } else {
                l10n.text("layer-panel-lock-selected")
            };
            let lock_response = ui.add_enabled(
                !selection.selected_layers.is_empty(),
                egui::Button::new(lock_label),
            );
            let lock_response = if parent_locked_only {
                lock_response.on_hover_text(l10n.text("layer-panel-locked-by-parent-click"))
            } else {
                lock_response
            };
            if lock_response.clicked() {
                *command = selection.lock_command();
                close_header_menu(ui, Some(popup_id));
            }

            if include_all_actions {
                ui.separator();
                ui.add_enabled_ui(can_add_mask, |ui| {
                    ui.menu_button(l10n.text("layer-panel-add-mask"), |ui| {
                        draw_add_mask_menu_contents(
                            ui,
                            l10n,
                            active_layer,
                            can_add_mask_from_selection,
                            Some(popup_id),
                            command,
                        );
                    });
                });
                let delete_label = match delete_action {
                    DeleteAction::Layer => l10n.text("layer-panel-delete-layer"),
                    DeleteAction::Mask => l10n.text("layer-panel-delete-mask"),
                };
                if ui
                    .add_enabled(can_delete, egui::Button::new(delete_label))
                    .clicked()
                {
                    *command = Some(match delete_action {
                        DeleteAction::Layer => selection
                            .delete_command()
                            .expect("enabled layer deletion must have a command"),
                        DeleteAction::Mask => Command::DeleteLayerMask {
                            layer_id: active_layer,
                        },
                    });
                    close_header_menu(ui, Some(popup_id));
                }
            }
        });
}

fn draw_add_layer_menu_contents(
    ui: &mut egui::Ui,
    l10n: &Localization,
    icons: &UiIconRegistry,
    current_color: [f32; 3],
    close_popup_id: Option<egui::Id>,
    command: &mut Option<Command>,
) {
    ui.set_min_width(ADD_LAYER_POPUP_WIDTH - 20.0);
    if ui.button(l10n.text("layer-panel-raster-layer")).clicked() {
        *command = Some(Command::AddLayer);
        close_header_menu(ui, close_popup_id);
    }
    if ui
        .button(l10n.text("layer-panel-solid-fill-layer"))
        .clicked()
    {
        *command = Some(Command::AddSolidFillLayer {
            color: current_color,
        });
        close_header_menu(ui, close_popup_id);
    }
    ui.separator();
    ui.menu_button(l10n.text("layer-panel-adjustment-layer"), |ui| {
        for kind in AdjustmentKind::ALL {
            if adjustment_layer_menu_item(ui, l10n, icons, kind).clicked() {
                *command = Some(Command::AddAdjustmentLayer { kind });
                close_header_menu(ui, close_popup_id);
            }
        }
    });
}

fn draw_add_mask_menu_contents(
    ui: &mut egui::Ui,
    l10n: &Localization,
    active_layer: LayerId,
    can_add_mask_from_selection: bool,
    close_popup_id: Option<egui::Id>,
    command: &mut Option<Command>,
) {
    ui.set_min_width(ADD_MASK_POPUP_WIDTH - 20.0);
    if ui.button(l10n.text("layer-panel-add-white-mask")).clicked() {
        *command = Some(Command::AddLayerMask {
            layer_id: active_layer,
            mode: LayerMaskInitMode::RevealAll,
        });
        close_header_menu(ui, close_popup_id);
    }
    if ui.button(l10n.text("layer-panel-add-black-mask")).clicked() {
        *command = Some(Command::AddLayerMask {
            layer_id: active_layer,
            mode: LayerMaskInitMode::HideAll,
        });
        close_header_menu(ui, close_popup_id);
    }
    if ui
        .add_enabled(
            can_add_mask_from_selection,
            egui::Button::new(l10n.text("layer-panel-add-mask-from-selection")),
        )
        .clicked()
    {
        *command = Some(Command::AddLayerMaskFromSelection {
            layer_id: active_layer,
        });
        close_header_menu(ui, close_popup_id);
    }
}

fn close_header_menu(ui: &egui::Ui, popup_id: Option<egui::Id>) {
    if let Some(popup_id) = popup_id {
        egui::Popup::close_id(ui.ctx(), popup_id);
    } else {
        ui.close();
    }
}

fn draw_layer_row_context_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
) {
    let tree = &document.layer_tree;
    let selection = LayerSelectionContext::new(tree, &ui_state.selected_layer_ids);
    let is_multi_selection = selection.is_multi_selection();

    if ui
        .add_enabled(
            !is_multi_selection,
            egui::Button::new(l10n.text("layer-panel-context-rename")),
        )
        .clicked()
    {
        begin_layer_rename(ui_state, row);
        ui.close();
    }

    ui.separator();

    let duplicate_label = if is_multi_selection {
        l10n.text("layer-panel-context-duplicate-selected")
    } else {
        l10n.text("layer-panel-context-duplicate")
    };
    if ui
        .add_enabled(selection.can_duplicate, egui::Button::new(duplicate_label))
        .clicked()
    {
        *command = selection.duplicate_command(row.layer_id);
        ui.close();
    }

    if is_multi_selection {
        if ui
            .add_enabled(
                selection.can_merge,
                egui::Button::new(l10n.text("layer-panel-context-merge-selected")),
            )
            .clicked()
        {
            *command = selection.merge_command();
            ui.close();
        }
    } else if matches!(row.content, LayerContent::Group { .. })
        && ui
            .add_enabled(
                selection.can_merge,
                egui::Button::new(l10n.text("layer-panel-context-merge-group")),
            )
            .clicked()
    {
        *command = selection.merge_command();
        ui.close();
    }

    if matches!(
        row.content,
        LayerContent::EmbeddedImage { .. } | LayerContent::SolidFill { .. }
    ) && ui
        .add_enabled(
            tree.can_rasterize_layer(row.layer_id),
            egui::Button::new(l10n.text("layer-panel-context-rasterize")),
        )
        .clicked()
    {
        *command = Some(Command::RasterizeLayer {
            layer_id: row.layer_id,
        });
        ui.close();
    }

    ui.separator();

    ui.menu_button(l10n.text("layer-panel-context-layer-mask"), |ui| {
        if let Some(mask_props) = row.mask {
            draw_layer_mask_context_menu(ui, l10n, document, row.layer_id, mask_props, command);
            return;
        }

        let can_add_mask = !is_multi_selection && tree.can_add_layer_mask(row.layer_id);
        if ui
            .add_enabled(
                can_add_mask,
                egui::Button::new(l10n.text("layer-panel-add-white-mask")),
            )
            .clicked()
        {
            *command = Some(Command::AddLayerMask {
                layer_id: row.layer_id,
                mode: LayerMaskInitMode::RevealAll,
            });
            ui.close();
        }
        if ui
            .add_enabled(
                can_add_mask,
                egui::Button::new(l10n.text("layer-panel-add-black-mask")),
            )
            .clicked()
        {
            *command = Some(Command::AddLayerMask {
                layer_id: row.layer_id,
                mode: LayerMaskInitMode::HideAll,
            });
            ui.close();
        }
        if ui
            .add_enabled(
                can_add_mask && document.active_selection.is_active(),
                egui::Button::new(l10n.text("layer-panel-add-mask-from-selection")),
            )
            .clicked()
        {
            *command = Some(Command::AddLayerMaskFromSelection {
                layer_id: row.layer_id,
            });
            ui.close();
        }
    });

    if matches!(row.content, LayerContent::Raster)
        && ui
            .button(l10n.text("layer-panel-context-select-transparency"))
            .clicked()
    {
        *command = Some(Command::SelectFromLayerTransparency {
            layer_id: row.layer_id,
        });
        ui.close();
    }

    ui.separator();

    let lock_label = match (is_multi_selection, selection.all_locally_locked) {
        (true, true) => l10n.text("layer-panel-context-unlock-selected"),
        (true, false) => l10n.text("layer-panel-context-lock-selected"),
        (false, true) => l10n.text("layer-panel-context-unlock"),
        (false, false) => l10n.text("layer-panel-context-lock"),
    };
    if ui
        .add_enabled(
            !selection.selected_layers.is_empty(),
            egui::Button::new(lock_label),
        )
        .clicked()
    {
        *command = selection.lock_command();
        ui.close();
    }

    ui.separator();

    let delete_label = if is_multi_selection {
        l10n.text("layer-panel-context-delete-selected")
    } else {
        l10n.text("layer-panel-context-delete")
    };
    if ui
        .add_enabled(selection.can_delete, egui::Button::new(delete_label))
        .clicked()
    {
        *command = selection.delete_command();
        ui.close();
    }
}

fn draw_layer_mask_context_menu(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    layer_id: LayerId,
    mask_props: LayerMaskProperties,
    command: &mut Option<Command>,
) {
    let tree = &document.layer_tree;
    let can_edit_mask = tree.has_layer_mask(layer_id) && !tree.is_effectively_locked(layer_id);
    let toggle_label = if mask_props.enabled {
        l10n.text("layer-panel-context-disable-mask")
    } else {
        l10n.text("layer-panel-context-enable-mask")
    };
    if ui
        .add_enabled(can_edit_mask, egui::Button::new(toggle_label))
        .clicked()
    {
        *command = Some(Command::SetLayerMaskEnabled {
            layer_id,
            enabled: !mask_props.enabled,
        });
        ui.close();
    }

    ui.separator();

    if ui
        .add_enabled(
            tree.can_apply_layer_mask(layer_id),
            egui::Button::new(l10n.text("layer-panel-context-apply-mask")),
        )
        .clicked()
    {
        *command = Some(Command::ApplyLayerMask { layer_id });
        ui.close();
    }
    if ui
        .add_enabled(
            can_edit_mask,
            egui::Button::new(l10n.text("layer-panel-context-delete-mask")),
        )
        .clicked()
    {
        *command = Some(Command::DeleteLayerMask { layer_id });
        ui.close();
    }
}

fn draw_rows(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    rows: &[LayerRowVm],
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    focused_material_index: usize,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    ui_request: &mut Option<UiRequest>,
    scroll_rect: egui::Rect,
) {
    if let Some(drag) = &mut ui_state.drag {
        match drag {
            LayerPanelDragState::Layers(drag) => drag.drop_target = None,
            LayerPanelDragState::LayerMask(drag) => drag.drop_target = None,
        }
    }

    for row in rows {
        ui.push_id(row.layer_id, |ui| {
            let mut row_responses = None;
            let row_selected = ui_state.selected_layer_ids.contains(&row.layer_id);
            let row_primary = row.layer_id == active_layer;
            let is_layer_drag_source = ui_state.drag.as_ref().is_some_and(|drag| match drag {
                LayerPanelDragState::Layers(drag) => drag.layer_ids.contains(&row.layer_id),
                LayerPanelDragState::LayerMask(_) => false,
            });
            let is_mask_drag_source = ui_state.drag.as_ref().is_some_and(|drag| match drag {
                LayerPanelDragState::Layers(_) => false,
                LayerPanelDragState::LayerMask(drag) => drag.source_layer_id == row.layer_id,
            });
            let is_drag_source = is_layer_drag_source || is_mask_drag_source;
            let inner = egui::Frame::NONE
                .fill(row_background_fill(
                    ui,
                    row_selected,
                    row_primary,
                    is_drag_source,
                ))
                .show(ui, |ui| {
                    ui.set_min_height(ROW_MIN_HEIGHT);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = ROW_CONTENT_GAP;
                        row_responses = Some(match row.content.clone() {
                            LayerContent::Raster => {
                                let raster_thumbnail = ui_state.thumbnails.texture_id_for_surface(
                                    ui.ctx(),
                                    document,
                                    PaintSurfaceId::raster(
                                        focused_material_index.into(),
                                        row.layer_id,
                                    ),
                                    SurfaceThumbnailMode::Rgba,
                                    ROW_THUMBNAIL_SIZE as u32,
                                );
                                let mask_thumbnail = row.mask.and_then(|_| {
                                    ui_state.thumbnails.texture_id_for_surface(
                                        ui.ctx(),
                                        document,
                                        PaintSurfaceId::layer_mask(
                                            focused_material_index.into(),
                                            row.layer_id,
                                        ),
                                        SurfaceThumbnailMode::MaskRedAsGray,
                                        ROW_MASK_THUMBNAIL_SIZE as u32,
                                    )
                                });
                                draw_raster_row(
                                    ui,
                                    l10n,
                                    document,
                                    row,
                                    active_layer,
                                    active_target,
                                    raster_thumbnail,
                                    mask_thumbnail,
                                    icons,
                                    ui_state,
                                    command,
                                    is_layer_drag_source,
                                )
                            }
                            LayerContent::EmbeddedImage { .. } => {
                                let mask_thumbnail = row.mask.and_then(|_| {
                                    ui_state.thumbnails.texture_id_for_surface(
                                        ui.ctx(),
                                        document,
                                        PaintSurfaceId::layer_mask(
                                            focused_material_index.into(),
                                            row.layer_id,
                                        ),
                                        SurfaceThumbnailMode::MaskRedAsGray,
                                        ROW_MASK_THUMBNAIL_SIZE as u32,
                                    )
                                });
                                draw_embedded_image_row(
                                    ui,
                                    l10n,
                                    document,
                                    row,
                                    active_layer,
                                    active_target,
                                    mask_thumbnail,
                                    icons,
                                    ui_state,
                                    command,
                                    is_layer_drag_source,
                                )
                            }
                            LayerContent::SolidFill { color } => {
                                let mask_thumbnail = row.mask.and_then(|_| {
                                    ui_state.thumbnails.texture_id_for_surface(
                                        ui.ctx(),
                                        document,
                                        PaintSurfaceId::layer_mask(
                                            focused_material_index.into(),
                                            row.layer_id,
                                        ),
                                        SurfaceThumbnailMode::MaskRedAsGray,
                                        ROW_MASK_THUMBNAIL_SIZE as u32,
                                    )
                                });
                                draw_solid_fill_row(
                                    ui,
                                    l10n,
                                    document,
                                    row,
                                    active_layer,
                                    active_target,
                                    color,
                                    mask_thumbnail,
                                    icons,
                                    ui_state,
                                    command,
                                    ui_request,
                                    is_layer_drag_source,
                                )
                            }
                            LayerContent::Adjustment { adjustment } => {
                                let mask_thumbnail = row.mask.and_then(|_| {
                                    ui_state.thumbnails.texture_id_for_surface(
                                        ui.ctx(),
                                        document,
                                        PaintSurfaceId::layer_mask(
                                            focused_material_index.into(),
                                            row.layer_id,
                                        ),
                                        SurfaceThumbnailMode::MaskRedAsGray,
                                        ROW_MASK_THUMBNAIL_SIZE as u32,
                                    )
                                });
                                draw_adjustment_row(
                                    ui,
                                    l10n,
                                    document,
                                    row,
                                    active_layer,
                                    active_target,
                                    adjustment,
                                    mask_thumbnail,
                                    icons,
                                    ui_state,
                                    command,
                                    ui_request,
                                    is_layer_drag_source,
                                )
                            }
                            LayerContent::Group { .. } => {
                                let mask_thumbnail = row.mask.and_then(|_| {
                                    ui_state.thumbnails.texture_id_for_surface(
                                        ui.ctx(),
                                        document,
                                        PaintSurfaceId::layer_mask(
                                            focused_material_index.into(),
                                            row.layer_id,
                                        ),
                                        SurfaceThumbnailMode::MaskRedAsGray,
                                        ROW_MASK_THUMBNAIL_SIZE as u32,
                                    )
                                });
                                draw_group_row(
                                    ui,
                                    l10n,
                                    document,
                                    row,
                                    active_layer,
                                    active_target,
                                    mask_thumbnail,
                                    icons,
                                    ui_state,
                                    command,
                                    is_layer_drag_source,
                                )
                            }
                        });
                    });
                });

            let LayerRowResponses {
                drag_source: row_response,
                layer_context,
                mask: mask_response,
            } = row_responses
                .take()
                .expect("layer rows must expose interaction responses");
            let layer_context_response = layer_context
                .into_iter()
                .chain(std::iter::once(row_response.clone()))
                .reduce(|combined, response| combined.union(response))
                .expect("layer rows must expose a context-menu response");
            let mask_secondary_clicked = mask_response
                .as_ref()
                .is_some_and(|(_, response)| response.secondary_clicked());
            let layer_secondary_clicked = layer_context_response.secondary_clicked();
            let pointer_over_row = ui
                .input(|input| input.pointer.hover_pos())
                .is_some_and(|pointer_pos| inner.response.rect.contains(pointer_pos));

            paint_row_interaction_state(ui, inner.response.rect, pointer_over_row, is_drag_source);

            if command.is_none() && row_response.clicked() {
                *command = update_selection_from_click(
                    ui,
                    row,
                    rows,
                    active_layer,
                    &mut ui_state.selected_layer_ids,
                    &mut ui_state.selection_anchor,
                );
            }

            if mask_secondary_clicked {
                update_selection_from_secondary_click(
                    row,
                    &mut ui_state.selected_layer_ids,
                    &mut ui_state.selection_anchor,
                );
                *command = Some(Command::SelectLayerMask {
                    layer_id: row.layer_id,
                });
            } else if layer_secondary_clicked {
                *command = Some(update_selection_from_secondary_click(
                    row,
                    &mut ui_state.selected_layer_ids,
                    &mut ui_state.selection_anchor,
                ));
            }

            if let Some((mask_props, response)) = &mask_response {
                response.context_menu(|ui| {
                    draw_layer_mask_context_menu(
                        ui,
                        l10n,
                        document,
                        row.layer_id,
                        *mask_props,
                        command,
                    );
                });
            }
            if !mask_secondary_clicked {
                layer_context_response.context_menu(|ui| {
                    draw_layer_row_context_menu(ui, l10n, document, row, ui_state, command);
                });
            }

            update_drag_state(
                ui,
                document,
                row,
                &row_response,
                inner.response.rect,
                pointer_over_row,
                ui_state,
                command,
            );

            match ui_state.drag.as_ref() {
                Some(LayerPanelDragState::Layers(drag)) => {
                    if let Some(target) = drag.drop_target
                        && target.target_id == row.layer_id
                    {
                        paint_drop_marker(
                            ui,
                            l10n,
                            inner.response.rect,
                            target.placement,
                            drag.layer_ids.len(),
                        );
                    }
                }
                Some(LayerPanelDragState::LayerMask(drag)) => {
                    if drag.drop_target == Some(row.layer_id) {
                        paint_layer_mask_drop_marker(ui, inner.response.rect);
                    }
                }
                None => {}
            }
        });
    }

    update_drag_auto_scroll(ui, scroll_rect, ui_state);

    if ui.input(|input| input.pointer.any_released())
        && let Some(drag) = ui_state.drag.take()
    {
        match drag {
            LayerPanelDragState::Layers(drag) => {
                if let Some(target) = drag.drop_target
                    && let Some((new_parent, new_index)) = drop_target_to_move_many(
                        &document.layer_tree,
                        &drag.layer_ids,
                        target.target_id,
                        target.placement,
                    )
                {
                    *command = Some(if drag.layer_ids.len() == 1 {
                        Command::MoveLayer {
                            layer_id: drag.layer_ids[0],
                            new_parent,
                            new_index,
                        }
                    } else {
                        Command::MoveLayers {
                            layer_ids: drag.layer_ids,
                            new_parent,
                            new_index,
                        }
                    });
                }
            }
            LayerPanelDragState::LayerMask(drag) => {
                if let Some(target_layer_id) = drag.drop_target {
                    ui_state
                        .thumbnails
                        .invalidate_surface(PaintSurfaceId::layer_mask(
                            focused_material_index.into(),
                            target_layer_id,
                        ));
                    *command = Some(Command::MoveLayerMask {
                        source_layer_id: drag.source_layer_id,
                        target_layer_id,
                    });
                }
            }
        }
    }
}

fn draw_embedded_image_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    mask_thumbnail: Option<egui::TextureId>,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    is_drag_source: bool,
) -> LayerRowResponses {
    let selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::EmbeddedImage;
    let mask_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::LayerMask;
    draw_visibility_cell(ui, row, icons, command);
    ui.add_space(ROW_TREE_GAP + row.depth.saturating_sub(1) as f32 * ROW_INDENT);
    let icon_response = icon_button(
        ui,
        icons,
        ICON_IMAGE,
        ROW_BUTTON_SIZE,
        ROW_ICON_SIZE,
        selected,
    )
    .on_hover_text(l10n.text("layer-panel-select-embedded-image"));
    if icon_response.clicked() {
        *command = Some(Command::SelectLayerForStructure {
            layer_id: row.layer_id,
        });
    }
    let mask = if let Some(mask_props) = row.mask {
        let response = thumbnail_button(
            ui,
            ROW_MASK_THUMBNAIL_SIZE,
            mask_selected,
            true,
            !mask_props.enabled,
            mask_thumbnail,
        )
        .on_hover_text(l10n.text("layer-panel-select-mask-help"));
        Some((
            mask_props,
            handle_layer_mask_thumbnail_response(ui, row, mask_props, response, ui_state, command),
        ))
    } else {
        None
    };
    let text_width = layer_text_width(ui);
    let response = draw_layer_text(ui, l10n, row, ui_state, command, is_drag_source, text_width);
    draw_row_trailing_cells(ui, l10n, document, row, icons, command);
    LayerRowResponses {
        drag_source: response,
        layer_context: vec![icon_response],
        mask,
    }
}

fn row_background_fill(
    ui: &egui::Ui,
    row_selected: bool,
    row_primary: bool,
    is_drag_source: bool,
) -> egui::Color32 {
    let selection_color = ui.visuals().selection.bg_fill;
    if is_drag_source {
        translucent(selection_color, 42)
    } else if row_primary {
        translucent(selection_color, 96)
    } else if row_selected {
        translucent(selection_color, 52)
    } else {
        egui::Color32::TRANSPARENT
    }
}

fn paint_row_interaction_state(
    ui: &egui::Ui,
    rect: egui::Rect,
    pointer_over_row: bool,
    is_drag_source: bool,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();

    if pointer_over_row && !is_drag_source {
        painter.rect_stroke(
            rect.shrink(0.5),
            ROW_CORNER_RADIUS,
            egui::Stroke::new(1.0, ui.visuals().widgets.hovered.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    }

    if is_drag_source {
        painter.rect_stroke(
            rect.shrink(0.5),
            ROW_CORNER_RADIUS,
            egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
}

fn update_drag_auto_scroll(
    ui: &mut egui::Ui,
    scroll_rect: egui::Rect,
    ui_state: &LayerPanelUiState,
) {
    if ui_state.drag.is_none() {
        return;
    }

    let Some(pointer_pos) = ui.input(|input| input.pointer.interact_pos()) else {
        return;
    };
    if !scroll_rect
        .expand(LAYER_DRAG_AUTOSCROLL_EDGE)
        .contains(pointer_pos)
    {
        return;
    }

    let delta_y = if pointer_pos.y < scroll_rect.top() + LAYER_DRAG_AUTOSCROLL_EDGE {
        LAYER_DRAG_AUTOSCROLL_SPEED
    } else if pointer_pos.y > scroll_rect.bottom() - LAYER_DRAG_AUTOSCROLL_EDGE {
        -LAYER_DRAG_AUTOSCROLL_SPEED
    } else {
        0.0
    };

    if delta_y != 0.0 {
        ui.scroll_with_delta(egui::vec2(0.0, delta_y));
        ui.ctx().request_repaint();
    }
}

fn draw_visibility_cell(
    ui: &mut egui::Ui,
    row: &LayerRowVm,
    icons: &UiIconRegistry,
    command: &mut Option<Command>,
) {
    let output = ui.allocate_ui_with_layout(
        egui::vec2(ROW_VISIBILITY_COLUMN_WIDTH, ROW_MIN_HEIGHT),
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
        |ui| {
            let visibility_icon = if row.visible {
                ICON_VISIBILITY
            } else {
                ICON_VISIBILITY_OFF
            };
            let response = if row.hidden_by_parent {
                icon_button_with_tint_alpha(
                    ui,
                    icons,
                    visibility_icon,
                    ROW_BUTTON_VISIBILITY_SIZE,
                    ROW_ICON_VISIBILITY_SIZE,
                    false,
                    96,
                )
            } else {
                icon_button(
                    ui,
                    icons,
                    visibility_icon,
                    ROW_BUTTON_VISIBILITY_SIZE,
                    ROW_ICON_VISIBILITY_SIZE,
                    false,
                )
            };
            /*let hover_text = if row.hidden_by_parent {
                "Hidden by parent group"
            } else {
                "Visible"
            };*/
            if response.clicked() {
                *command = Some(Command::SetLayerVisible {
                    layer_id: row.layer_id,
                    visible: !row.visible,
                });
            }
        },
    );

    let separator_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    ui.painter().vline(
        output.response.rect.right(),
        output.response.rect.top()..=output.response.rect.bottom(),
        egui::Stroke::new(1.0, translucent(separator_color, 96)),
    );
}

fn handle_layer_mask_thumbnail_response(
    ui: &egui::Ui,
    row: &LayerRowVm,
    mask_props: LayerMaskProperties,
    response: egui::Response,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
) -> egui::Response {
    let locked = row.locked || row.locked_by_parent;
    let cursor = if locked {
        egui::CursorIcon::Default
    } else if response.dragged() {
        egui::CursorIcon::Grabbing
    } else {
        egui::CursorIcon::Grab
    };
    let response = response.on_hover_cursor(cursor);

    if response.drag_started() && !locked {
        ui_state.drag = Some(LayerPanelDragState::LayerMask(LayerMaskDragState {
            source_layer_id: row.layer_id,
            drop_target: None,
        }));
    }
    if response.clicked_by(egui::PointerButton::Primary) {
        let shift_down = ui.input(|input| input.modifiers.shift);
        if shift_down && !locked {
            *command = Some(Command::SetLayerMaskEnabled {
                layer_id: row.layer_id,
                enabled: !mask_props.enabled,
            });
        } else {
            *command = Some(Command::SelectLayerMask {
                layer_id: row.layer_id,
            });
        }
    }

    response
}

fn draw_raster_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    raster_thumbnail: Option<egui::TextureId>,
    mask_thumbnail: Option<egui::TextureId>,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    is_drag_source: bool,
) -> LayerRowResponses {
    let raster_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::Raster;
    let mask_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::LayerMask;

    draw_visibility_cell(ui, row, icons, command);
    ui.add_space(ROW_TREE_GAP + row.depth.saturating_sub(1) as f32 * ROW_INDENT);

    let raster_response = thumbnail_button(
        ui,
        ROW_THUMBNAIL_SIZE,
        raster_selected,
        false,
        false,
        raster_thumbnail,
    )
    .on_hover_text(l10n.text("layer-panel-select-layer"));
    if raster_response.clicked() {
        *command = Some(Command::SelectLayer {
            layer_id: row.layer_id,
        });
    }

    let mask = if let Some(mask_props) = row.mask {
        let mask_response = thumbnail_button(
            ui,
            ROW_MASK_THUMBNAIL_SIZE,
            mask_selected,
            true,
            !mask_props.enabled,
            mask_thumbnail,
        )
        .on_hover_text(l10n.text("layer-panel-select-mask-help"));
        Some((
            mask_props,
            handle_layer_mask_thumbnail_response(
                ui,
                row,
                mask_props,
                mask_response,
                ui_state,
                command,
            ),
        ))
    } else {
        None
    };

    let text_width = layer_text_width(ui);
    let response = draw_layer_text(ui, l10n, row, ui_state, command, is_drag_source, text_width);
    draw_row_trailing_cells(ui, l10n, document, row, icons, command);
    LayerRowResponses {
        drag_source: response,
        layer_context: vec![raster_response],
        mask,
    }
}

fn draw_solid_fill_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    color: [f32; 3],
    mask_thumbnail: Option<egui::TextureId>,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    ui_request: &mut Option<UiRequest>,
    is_drag_source: bool,
) -> LayerRowResponses {
    let fill_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::SolidFill;
    let mask_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::LayerMask;

    draw_visibility_cell(ui, row, icons, command);
    ui.add_space(ROW_TREE_GAP + row.depth.saturating_sub(1) as f32 * ROW_INDENT);

    let popup_id = egui::Id::new(("layer_panel_solid_fill_color_popup", row.layer_id));
    let fill_response = ui
        .add_enabled_ui(!(row.locked || row.locked_by_parent), |ui| {
            solid_fill_thumbnail_button(ui, ROW_THUMBNAIL_SIZE, fill_selected, color, icons)
        })
        .inner
        .on_hover_text(if row.locked || row.locked_by_parent {
            l10n.text("layer-panel-fill-color-locked-short")
        } else {
            l10n.text("layer-panel-edit-fill-color")
        });
    if fill_response.clicked() {
        ui_state.toggle_solid_fill_color_popup(row.layer_id);
        *command = Some(Command::SelectSolidFillLayer {
            layer_id: row.layer_id,
        });
    }

    let mut popup_open = ui_state.solid_fill_color_popup == Some(row.layer_id);
    let edit_session = ui_state
        .solid_fill_color_session
        .filter(|session| session.layer_id == row.layer_id)
        .map(|session| session.edit_session)
        .unwrap_or(0);
    // Keep the parent outside egui's single memory-managed popup slot so the nested color
    // detail popup can open without replacing it.
    egui::Popup::menu(&fill_response)
        .id(popup_id)
        .open_bool(&mut popup_open)
        .width(280.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(260.0);
            ui.add_enabled_ui(!(row.locked || row.locked_by_parent), |ui| {
                let mut edited_color = color;
                let content = draw_color_panel_content(
                    ui,
                    popup_id.with("color_panel"),
                    &mut edited_color,
                    icons,
                    l10n,
                );
                if content.changed {
                    *command = Some(Command::SetSolidFillColor {
                        layer_id: row.layer_id,
                        color: edited_color,
                        edit_session,
                    });
                }
                if content.start_screen_eyedropper {
                    *ui_request = Some(UiRequest::StartScreenEyedropper(
                        ScreenEyedropperTarget::SolidFill {
                            layer_id: row.layer_id,
                            edit_session,
                        },
                    ));
                }
            });
            if row.locked || row.locked_by_parent {
                ui.label(l10n.text("layer-panel-fill-color-locked"));
            }
        });
    if !popup_open && ui_state.solid_fill_color_popup == Some(row.layer_id) {
        ui_state.solid_fill_color_popup = None;
    }

    let mask = if let Some(mask_props) = row.mask {
        let mask_response = thumbnail_button(
            ui,
            ROW_MASK_THUMBNAIL_SIZE,
            mask_selected,
            true,
            !mask_props.enabled,
            mask_thumbnail,
        )
        .on_hover_text(l10n.text("layer-panel-select-mask-help"));
        Some((
            mask_props,
            handle_layer_mask_thumbnail_response(
                ui,
                row,
                mask_props,
                mask_response,
                ui_state,
                command,
            ),
        ))
    } else {
        None
    };

    let text_width = layer_text_width(ui);
    let response = draw_layer_text(ui, l10n, row, ui_state, command, is_drag_source, text_width);
    draw_row_trailing_cells(ui, l10n, document, row, icons, command);
    LayerRowResponses {
        drag_source: response,
        layer_context: vec![fill_response],
        mask,
    }
}

fn draw_adjustment_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    adjustment: Adjustment,
    mask_thumbnail: Option<egui::TextureId>,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    ui_request: &mut Option<UiRequest>,
    is_drag_source: bool,
) -> LayerRowResponses {
    let adjustment_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::Adjustment;
    let mask_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::LayerMask;

    draw_visibility_cell(ui, row, icons, command);
    ui.add_space(ROW_TREE_GAP + row.depth.saturating_sub(1) as f32 * ROW_INDENT);

    let icon_response = ui
        .add_enabled_ui(!(row.locked || row.locked_by_parent), |ui| {
            icon_button(
                ui,
                icons,
                adjustment_icon(adjustment.kind()),
                ROW_BUTTON_SIZE,
                ROW_ICON_SIZE,
                adjustment_selected,
            )
        })
        .inner
        .on_hover_text(if row.locked || row.locked_by_parent {
            l10n.text("layer-panel-adjustment-locked")
        } else {
            let mut args = fluent::FluentArgs::new();
            args.set("name", adjustment_kind_label(l10n, adjustment.kind()));
            l10n.format("layer-panel-edit-adjustment", Some(&args))
        });
    if icon_response.clicked() {
        *command = Some(Command::SelectAdjustmentLayer {
            layer_id: row.layer_id,
        });
        *ui_request = Some(UiRequest::OpenAdjustmentEditor {
            layer_id: row.layer_id,
        });
    }

    let mask = if let Some(mask_props) = row.mask {
        let mask_response = thumbnail_button(
            ui,
            ROW_MASK_THUMBNAIL_SIZE,
            mask_selected,
            true,
            !mask_props.enabled,
            mask_thumbnail,
        )
        .on_hover_text(l10n.text("layer-panel-select-mask-help"));
        Some((
            mask_props,
            handle_layer_mask_thumbnail_response(
                ui,
                row,
                mask_props,
                mask_response,
                ui_state,
                command,
            ),
        ))
    } else {
        None
    };

    let text_width = layer_text_width(ui);
    let response = draw_layer_text(ui, l10n, row, ui_state, command, is_drag_source, text_width);
    draw_row_trailing_cells(ui, l10n, document, row, icons, command);
    LayerRowResponses {
        drag_source: response,
        layer_context: vec![icon_response],
        mask,
    }
}

fn draw_group_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    active_layer: LayerId,
    active_target: ActiveLayerTarget,
    mask_thumbnail: Option<egui::TextureId>,
    icons: &UiIconRegistry,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    is_drag_source: bool,
) -> LayerRowResponses {
    let selected = row.layer_id == active_layer && active_target == ActiveLayerTarget::Structure;
    let mask_selected =
        row.layer_id == active_layer && active_target == ActiveLayerTarget::LayerMask;

    draw_visibility_cell(ui, row, icons, command);
    ui.add_space(ROW_TREE_GAP + row.depth.saturating_sub(1) as f32 * ROW_INDENT);

    let folder_icon = if row.collapsed {
        ICON_FOLDER
    } else {
        ICON_FOLDER_OPEN
    };
    let folder_response = icon_button(
        ui,
        icons,
        folder_icon,
        ROW_BUTTON_SIZE,
        ROW_ICON_SIZE,
        selected,
    )
    .on_hover_text(if row.collapsed {
        l10n.text("layer-panel-expand-group")
    } else {
        l10n.text("layer-panel-collapse-group")
    });
    if folder_response.clicked() {
        if row.collapsed {
            ui_state.collapsed_groups.remove(&row.layer_id);
        } else if row.child_count > 0 {
            ui_state.collapsed_groups.insert(row.layer_id);
        }
    }

    let mask = if let Some(mask_props) = row.mask {
        let mask_response = thumbnail_button(
            ui,
            ROW_MASK_THUMBNAIL_SIZE,
            mask_selected,
            true,
            !mask_props.enabled,
            mask_thumbnail,
        )
        .on_hover_text(l10n.text("layer-panel-select-group-mask-help"));
        Some((
            mask_props,
            handle_layer_mask_thumbnail_response(
                ui,
                row,
                mask_props,
                mask_response,
                ui_state,
                command,
            ),
        ))
    } else {
        None
    };

    let text_width = layer_text_width(ui);
    let response = draw_layer_text(ui, l10n, row, ui_state, command, is_drag_source, text_width);
    draw_row_trailing_cells(ui, l10n, document, row, icons, command);
    LayerRowResponses {
        drag_source: response,
        layer_context: vec![folder_response],
        mask,
    }
}

fn visible_rows_for_panel(
    tree: &LayerTree,
    collapsed_groups: &HashSet<LayerId>,
) -> Vec<LayerRowVm> {
    let mut rows = Vec::new();
    if let Some(children) = tree.children(tree.root()) {
        for &child in children.iter().rev() {
            collect_visible_rows(tree, child, 1, true, false, collapsed_groups, &mut rows);
        }
    }
    rows
}

fn collect_visible_rows(
    tree: &LayerTree,
    layer_id: LayerId,
    depth: usize,
    parent_visible: bool,
    parent_locked: bool,
    collapsed_groups: &HashSet<LayerId>,
    rows: &mut Vec<LayerRowVm>,
) {
    let Some(node) = tree.get(layer_id) else {
        return;
    };
    let collapsed =
        matches!(node.content, LayerContent::Group { .. }) && collapsed_groups.contains(&layer_id);
    rows.push(LayerRowVm {
        layer_id,
        depth,
        content: node.content.clone(),
        name: node.props.name.clone(),
        visible: node.props.visible,
        hidden_by_parent: !parent_visible,
        locked: node.props.locked,
        locked_by_parent: parent_locked,
        opacity: node.props.opacity,
        blend_mode: node.props.blend_mode,
        group_composite_mode: tree
            .group_composite_mode(layer_id)
            .unwrap_or(GroupCompositeMode::Isolated),
        mask: node.mask,
        material_mask: node.material_mask.clone(),
        collapsed,
        child_count: tree.children(layer_id).map_or(0, <[LayerId]>::len),
    });

    if collapsed {
        return;
    }

    let child_parent_visible = parent_visible && node.props.visible;
    let child_parent_locked = parent_locked || node.props.locked;
    for &child in tree.children(layer_id).unwrap_or_default().iter().rev() {
        collect_visible_rows(
            tree,
            child,
            depth + 1,
            child_parent_visible,
            child_parent_locked,
            collapsed_groups,
            rows,
        );
    }
}

fn can_delete_layers(tree: &LayerTree, layer_ids: &[LayerId]) -> bool {
    if layer_ids.is_empty()
        || layer_ids
            .iter()
            .any(|layer_id| *layer_id == tree.root() || !tree.contains(*layer_id))
    {
        return false;
    }
    let removing_rasters = layer_ids
        .iter()
        .map(|layer_id| tree.raster_layers_in_subtree(*layer_id).len())
        .sum::<usize>();
    removing_rasters < tree.ordered_raster_layers().len()
}

fn effective_selection_roots(
    tree: &LayerTree,
    selected_layer_ids: &HashSet<LayerId>,
) -> Vec<LayerId> {
    tree.rows()
        .into_iter()
        .map(|row| row.layer_id)
        .filter(|layer_id| selected_layer_ids.contains(layer_id))
        .filter(|layer_id| {
            let mut current = tree.get(*layer_id).and_then(|node| node.parent);
            while let Some(parent) = current {
                if selected_layer_ids.contains(&parent) {
                    return false;
                }
                current = tree.get(parent).and_then(|node| node.parent);
            }
            true
        })
        .collect()
}

fn common_composite_mode(
    tree: &LayerTree,
    layer_ids: &[LayerId],
) -> Option<LayerPanelCompositeMode> {
    let mut modes = layer_ids.iter().filter_map(|layer_id| {
        let node = tree.get(*layer_id)?;
        Some(layer_panel_composite_mode(
            node.props.blend_mode,
            tree.group_composite_mode(*layer_id)
                .unwrap_or(GroupCompositeMode::Isolated),
            matches!(node.content, LayerContent::Group { .. }),
        ))
    });
    let first = modes.next()?;
    modes.all(|mode| mode == first).then_some(first)
}

fn layer_panel_composite_mode(
    blend_mode: LayerBlendMode,
    group_mode: GroupCompositeMode,
    is_group: bool,
) -> LayerPanelCompositeMode {
    if is_group && group_mode == GroupCompositeMode::PassThrough {
        LayerPanelCompositeMode::PassThrough
    } else {
        LayerPanelCompositeMode::Blend(blend_mode)
    }
}

fn group_composite_mode_label(l10n: &Localization, mode: GroupCompositeMode) -> String {
    l10n.text(match mode {
        GroupCompositeMode::Isolated => "composite-group-isolated",
        GroupCompositeMode::PassThrough => "composite-group-pass-through",
    })
}

fn layer_blend_mode_label(l10n: &Localization, mode: LayerBlendMode) -> String {
    l10n.text(match mode {
        LayerBlendMode::Normal => "composite-layer-normal",
        LayerBlendMode::Darken => "composite-layer-darken",
        LayerBlendMode::Multiply => "composite-layer-multiply",
        LayerBlendMode::Lighten => "composite-layer-lighten",
        LayerBlendMode::Screen => "composite-layer-screen",
        LayerBlendMode::ColorDodge => "composite-layer-color-dodge",
        LayerBlendMode::LinearDodge => "composite-layer-linear-dodge",
        LayerBlendMode::Overlay => "composite-layer-overlay",
        LayerBlendMode::SoftLight => "composite-layer-soft-light",
        LayerBlendMode::HardLight => "composite-layer-hard-light",
        LayerBlendMode::Color => "composite-layer-color",
    })
}

fn adjustment_kind_label(l10n: &Localization, kind: AdjustmentKind) -> String {
    l10n.text(match kind {
        AdjustmentKind::BrightnessContrast => "adjustment-brightness-contrast",
        AdjustmentKind::Levels => "adjustment-levels",
        AdjustmentKind::Curves => "adjustment-curves",
        AdjustmentKind::HueSaturation => "adjustment-hue-saturation",
        AdjustmentKind::Invert => "adjustment-invert",
        AdjustmentKind::GradientMap => "adjustment-gradient-map",
        AdjustmentKind::UvMirror => "adjustment-uv-mirror",
    })
}

fn adjustment_layer_menu_item(
    ui: &mut egui::Ui,
    l10n: &Localization,
    icons: &UiIconRegistry,
    kind: AdjustmentKind,
) -> egui::Response {
    let button = (if let Some(texture) = icons.texture(adjustment_icon(kind)) {
        egui::Button::image_and_text(
            egui::Image::new(texture).fit_to_exact_size(egui::Vec2::splat(ROW_ICON_SIZE)),
            adjustment_kind_label(l10n, kind),
        )
        .image_tint_follows_text_color(true)
    } else {
        egui::Button::new(adjustment_kind_label(l10n, kind))
    })
    // A trailing grow atom keeps full-width menu contents aligned to the left.
    .right_text(());

    ui.add_sized(egui::vec2(ui.available_width(), ROW_BUTTON_SIZE), button)
}

fn adjustment_icon(kind: AdjustmentKind) -> &'static str {
    match kind {
        AdjustmentKind::BrightnessContrast => ICON_ADJUSTMENT_BRIGHTNESS_CONTRAST,
        AdjustmentKind::Levels => ICON_ADJUSTMENT_LEVELS,
        AdjustmentKind::Curves => ICON_ADJUSTMENT_CURVES,
        AdjustmentKind::HueSaturation => ICON_ADJUSTMENT_HSV,
        AdjustmentKind::Invert => ICON_ADJUSTMENT_INVERT,
        AdjustmentKind::GradientMap => ICON_ADJUSTMENT_GRADIENT_MAP,
        AdjustmentKind::UvMirror => ICON_UV_MIRROR,
    }
}

fn selection_command(row: &LayerRowVm) -> Command {
    match row.content {
        LayerContent::Raster => Command::SelectLayer {
            layer_id: row.layer_id,
        },
        LayerContent::EmbeddedImage { .. } => Command::SelectLayerForStructure {
            layer_id: row.layer_id,
        },
        LayerContent::SolidFill { .. } => Command::SelectSolidFillLayer {
            layer_id: row.layer_id,
        },
        LayerContent::Adjustment { .. } => Command::SelectAdjustmentLayer {
            layer_id: row.layer_id,
        },
        LayerContent::Group { .. } => Command::SelectLayerForStructure {
            layer_id: row.layer_id,
        },
    }
}

fn update_selection_from_click(
    ui: &egui::Ui,
    row: &LayerRowVm,
    rows: &[LayerRowVm],
    active_layer: LayerId,
    selected_layer_ids: &mut HashSet<LayerId>,
    selection_anchor: &mut Option<LayerId>,
) -> Option<Command> {
    let (shift, toggle) = ui.input(|input| {
        (
            input.modifiers.shift,
            input.modifiers.ctrl || input.modifiers.command,
        )
    });

    if shift {
        let anchor = selection_anchor.unwrap_or(active_layer);
        let range = layer_range(rows, anchor, row.layer_id);
        if !toggle {
            selected_layer_ids.clear();
        }
        selected_layer_ids.extend(range);
        selected_layer_ids.insert(row.layer_id);
        *selection_anchor = Some(row.layer_id);
        return Some(selection_command(row));
    }

    if toggle {
        if selected_layer_ids.contains(&row.layer_id) && row.layer_id != active_layer {
            selected_layer_ids.remove(&row.layer_id);
            return None;
        }
        selected_layer_ids.insert(row.layer_id);
        *selection_anchor = Some(row.layer_id);
        return Some(selection_command(row));
    }

    selected_layer_ids.clear();
    selected_layer_ids.insert(row.layer_id);
    *selection_anchor = Some(row.layer_id);
    Some(selection_command(row))
}

fn update_selection_from_secondary_click(
    row: &LayerRowVm,
    selected_layer_ids: &mut HashSet<LayerId>,
    selection_anchor: &mut Option<LayerId>,
) -> Command {
    if !selected_layer_ids.contains(&row.layer_id) {
        selected_layer_ids.clear();
        selected_layer_ids.insert(row.layer_id);
    }
    *selection_anchor = Some(row.layer_id);
    selection_command(row)
}

fn layer_range(rows: &[LayerRowVm], from: LayerId, to: LayerId) -> Vec<LayerId> {
    let Some(from_index) = rows.iter().position(|row| row.layer_id == from) else {
        return vec![to];
    };
    let Some(to_index) = rows.iter().position(|row| row.layer_id == to) else {
        return vec![to];
    };
    let (start, end) = if from_index <= to_index {
        (from_index, to_index)
    } else {
        (to_index, from_index)
    };
    rows[start..=end].iter().map(|row| row.layer_id).collect()
}

fn update_drag_state(
    ui: &egui::Ui,
    document: &Document,
    row: &LayerRowVm,
    drag_source_response: &egui::Response,
    row_rect: egui::Rect,
    pointer_over_row: bool,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
) {
    if drag_source_response.drag_started()
        && !matches!(
            ui_state.drag.as_ref(),
            Some(LayerPanelDragState::LayerMask(_))
        )
    {
        if !ui_state.selected_layer_ids.contains(&row.layer_id) {
            ui_state.selected_layer_ids.clear();
            ui_state.selected_layer_ids.insert(row.layer_id);
            ui_state.selection_anchor = Some(row.layer_id);
            if command.is_none() {
                *command = Some(selection_command(row));
            }
        }
        let mut layer_ids =
            effective_selection_roots(&document.layer_tree, &ui_state.selected_layer_ids);
        if layer_ids.is_empty() {
            layer_ids.push(row.layer_id);
        }
        ui_state.drag = Some(LayerPanelDragState::Layers(LayerDragState {
            layer_ids,
            drop_target: None,
        }));
    }

    let Some(drag) = ui_state.drag.clone() else {
        return;
    };
    match drag {
        LayerPanelDragState::Layers(mut drag) => {
            if drag.layer_ids.contains(&row.layer_id) || !pointer_over_row {
                ui_state.drag = Some(LayerPanelDragState::Layers(drag));
                return;
            }

            let Some(pointer_pos) = ui.input(|input| input.pointer.interact_pos()) else {
                ui_state.drag = Some(LayerPanelDragState::Layers(drag));
                return;
            };
            let placement = drop_placement_for_pointer(row, row_rect, pointer_pos);
            let target = LayerDropTarget {
                target_id: row.layer_id,
                placement,
            };
            drag.drop_target = drop_target_to_move_many(
                &document.layer_tree,
                &drag.layer_ids,
                target.target_id,
                target.placement,
            )
            .is_some()
            .then_some(target);
            ui_state.drag = Some(LayerPanelDragState::Layers(drag));
        }
        LayerPanelDragState::LayerMask(mut drag) => {
            if drag.source_layer_id == row.layer_id || !pointer_over_row {
                ui_state.drag = Some(LayerPanelDragState::LayerMask(drag));
                return;
            }
            drag.drop_target = document
                .layer_tree
                .can_move_layer_mask(drag.source_layer_id, row.layer_id)
                .then_some(row.layer_id);
            ui_state.drag = Some(LayerPanelDragState::LayerMask(drag));
        }
    }
}

fn drop_placement_for_pointer(
    row: &LayerRowVm,
    row_rect: egui::Rect,
    pointer_pos: egui::Pos2,
) -> DropPlacement {
    if matches!(row.content, LayerContent::Group { .. }) {
        let top_band = row_rect.top() + row_rect.height() * 0.25;
        let bottom_band = row_rect.bottom() - row_rect.height() * 0.25;
        if pointer_pos.y < top_band {
            DropPlacement::Above
        } else if pointer_pos.y > bottom_band {
            DropPlacement::Below
        } else {
            DropPlacement::Into
        }
    } else if pointer_pos.y < row_rect.center().y {
        DropPlacement::Above
    } else {
        DropPlacement::Below
    }
}

#[cfg(test)]
fn drop_target_to_move(
    tree: &LayerTree,
    dragged: LayerId,
    target: LayerId,
    placement: DropPlacement,
) -> Option<(LayerId, usize)> {
    drop_target_to_move_many(tree, &[dragged], target, placement)
}

fn drop_target_to_move_many(
    tree: &LayerTree,
    dragged: &[LayerId],
    target: LayerId,
    placement: DropPlacement,
) -> Option<(LayerId, usize)> {
    if dragged.is_empty()
        || dragged.contains(&target)
        || dragged
            .iter()
            .any(|layer_id| *layer_id == tree.root() || !tree.contains(*layer_id))
        || !tree.contains(target)
    {
        return None;
    }

    let (new_parent, mut new_index) = match placement {
        DropPlacement::Into => {
            if !tree.is_group(target)
                || dragged
                    .iter()
                    .any(|layer_id| is_descendant(tree, target, *layer_id))
            {
                return None;
            }
            let child_count = tree.children(target)?.len();
            (target, child_count)
        }
        DropPlacement::Above | DropPlacement::Below => {
            if dragged
                .iter()
                .any(|layer_id| is_descendant(tree, target, *layer_id))
            {
                return None;
            }
            let (parent, target_index) = tree.parent_and_index(target)?;
            let index = match placement {
                DropPlacement::Above => target_index + 1,
                DropPlacement::Below => target_index,
                DropPlacement::Into => unreachable!(),
            };
            (parent, index)
        }
    };

    if dragged.len() == 1 {
        let layer_id = dragged[0];
        if let Some((old_parent, old_index)) = tree.parent_and_index(layer_id)
            && old_parent == new_parent
        {
            if old_index < new_index {
                new_index = new_index.saturating_sub(1);
            }
            if old_index == new_index {
                return None;
            }
        }
    }

    let mut moved = tree.clone();
    if dragged.len() == 1 {
        moved
            .move_layer(dragged[0], new_parent, new_index)
            .then_some((new_parent, new_index))
    } else {
        moved
            .move_layers(dragged, new_parent, new_index)
            .then_some((new_parent, new_index))
    }
}

fn is_descendant(tree: &LayerTree, layer_id: LayerId, possible_ancestor: LayerId) -> bool {
    let mut current = tree.get(layer_id).and_then(|node| node.parent);
    while let Some(parent) = current {
        if parent == possible_ancestor {
            return true;
        }
        current = tree.get(parent).and_then(|node| node.parent);
    }
    false
}

fn paint_drop_marker(
    ui: &egui::Ui,
    l10n: &Localization,
    rect: egui::Rect,
    placement: DropPlacement,
    layer_count: usize,
) {
    let color = ui.visuals().selection.stroke.color;
    let fill = translucent(color, 28);
    let stroke = egui::Stroke::new(3.0, color);
    let painter = ui.painter();

    match placement {
        DropPlacement::Above | DropPlacement::Below => {
            let y = if placement == DropPlacement::Above {
                rect.top()
            } else {
                rect.bottom()
            };
            let band = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 2.0, y - 5.0),
                egui::pos2(rect.right() - 2.0, y + 5.0),
            );
            painter.rect_filled(band, 4.0, fill);
            painter.hline(rect.left() + 6.0..=rect.right() - 6.0, y, stroke);
            painter.circle_filled(egui::pos2(rect.left() + 8.0, y), 4.0, color);
            painter.circle_filled(egui::pos2(rect.right() - 8.0, y), 4.0, color);
        }
        DropPlacement::Into => {
            let marker_rect = rect.shrink(2.0);
            painter.rect_filled(marker_rect, 0.0, fill);
            painter.rect_stroke(marker_rect, 0.0, stroke, egui::StrokeKind::Inside);
            painter.text(
                marker_rect.center(),
                egui::Align2::CENTER_CENTER,
                {
                    let mut args = fluent::FluentArgs::new();
                    args.set("count", layer_count);
                    l10n.format("layer-panel-drop-into-group", Some(&args))
                },
                egui::FontId::proportional(12.0),
                color,
            );
        }
    }
}

fn paint_layer_mask_drop_marker(ui: &egui::Ui, rect: egui::Rect) {
    let color = ui.visuals().selection.stroke.color;
    let marker_rect = rect.shrink(2.0);
    ui.painter()
        .rect_filled(marker_rect, 0.0, translucent(color, 28));
    ui.painter().rect_stroke(
        marker_rect,
        0.0,
        egui::Stroke::new(3.0, color),
        egui::StrokeKind::Inside,
    );
}

fn layer_text_width(ui: &egui::Ui) -> f32 {
    (ui.available_width()
        - ROW_LOCK_COLUMN_WIDTH
        - ROW_MATERIAL_MASK_COLUMN_WIDTH
        - ui.spacing().item_spacing.x)
        .max(0.0)
}

fn summarize_material_mask(
    material_mask: &LayerMaterialMask,
    materials: &[MaterialData],
    mut effective_allowed: impl FnMut(MaterialId) -> bool,
) -> MaterialMaskSummary {
    let specified = matches!(material_mask, LayerMaterialMask::Specified(_));
    let mut specified_count = 0;
    let mut representative_colors = Vec::with_capacity(ROW_MATERIAL_MASK_MAX_SWATCHES);
    let mut effective_count = 0;

    for material in materials {
        if specified && material_mask.allows(material.id) {
            specified_count += 1;
            if representative_colors.len() < ROW_MATERIAL_MASK_MAX_SWATCHES {
                representative_colors.push(material.ui_color);
            }
        }
        if effective_allowed(material.id) {
            effective_count += 1;
        }
    }

    MaterialMaskSummary {
        specified,
        specified_count,
        representative_colors,
        effective_count,
        show_as_unrestricted: !specified
            || (!materials.is_empty() && specified_count == materials.len()),
    }
}

fn draw_row_trailing_cells(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    icons: &UiIconRegistry,
    command: &mut Option<Command>,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            ROW_LOCK_COLUMN_WIDTH + ROW_MATERIAL_MASK_COLUMN_WIDTH,
            ROW_MIN_HEIGHT,
        ),
        egui::Sense::hover(),
    );
    let lock_rect =
        egui::Rect::from_min_size(rect.min, egui::vec2(ROW_LOCK_COLUMN_WIDTH, rect.height()));
    let material_mask_rect =
        egui::Rect::from_min_max(egui::pos2(lock_rect.right(), rect.top()), rect.max);

    draw_lock_cell(ui, l10n, row, icons, lock_rect);
    draw_material_mask_cell(ui, l10n, document, row, command, material_mask_rect);
}

fn draw_lock_cell(
    ui: &mut egui::Ui,
    l10n: &Localization,
    row: &LayerRowVm,
    icons: &UiIconRegistry,
    rect: egui::Rect,
) {
    let response = ui.interact(
        rect,
        ui.id().with(("layer_lock_cell", row.layer_id)),
        egui::Sense::hover(),
    );
    let alpha = if row.locked {
        Some(255)
    } else if row.locked_by_parent {
        Some(96)
    } else {
        None
    };
    if let Some(alpha) = alpha
        && let Some(texture) = icons.texture(ICON_LOCK)
    {
        let icon_rect =
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(ROW_LOCK_ICON_SIZE));
        let tint = translucent(ui.visuals().widgets.noninteractive.fg_stroke.color, alpha);
        ui.painter().image(
            texture.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
        response.on_hover_text(l10n.text(if row.locked {
            "layer-panel-layer-locked"
        } else {
            "layer-panel-locked-by-parent"
        }));
    }
}

fn draw_material_mask_cell(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    command: &mut Option<Command>,
    rect: egui::Rect,
) {
    let summary = summarize_material_mask(&row.material_mask, &document.materials, |material_id| {
        document
            .layer_tree
            .effective_material_allowed(row.layer_id, material_id)
    });
    let response = ui.interact(
        rect,
        ui.id().with(("layer_material_mask_cell", row.layer_id)),
        egui::Sense::click(),
    );
    let row_hovered = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pointer_pos| {
            pointer_pos.y >= rect.top()
                && pointer_pos.y <= rect.bottom()
                && ui.clip_rect().contains(pointer_pos)
        });

    if ui.is_rect_visible(rect) {
        paint_material_mask_summary(ui, rect, &summary, row_hovered);
    }

    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_ui(|ui| {
            draw_material_mask_tooltip(ui, l10n, document, row, &summary);
        });
    let popup_id = egui::Id::new(("layer_material_mask_popup", row.layer_id));
    egui::Popup::menu(&response)
        .id(popup_id)
        .width(MATERIAL_MASK_POPUP_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            draw_material_mask_popup(ui, l10n, document, row, command);
        });
}

fn draw_material_mask_popup(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    command: &mut Option<Command>,
) {
    ui.set_min_width(MATERIAL_MASK_POPUP_WIDTH - 20.0);
    if row.content.requires_single_material() {
        draw_single_material_popup(ui, l10n, document, row, command);
        return;
    }

    ui.label(l10n.text("layer-panel-material-mask"));
    ui.separator();

    if ui.button(l10n.text("action-select-all")).clicked() {
        *command = Some(Command::SetLayerMaterialMask {
            layer_id: row.layer_id,
            material_mask: specified_all_materials(&document.materials),
        });
    }
    if ui.button(l10n.text("action-clear-all")).clicked() {
        *command = Some(Command::SetLayerMaterialMask {
            layer_id: row.layer_id,
            material_mask: LayerMaterialMask::Specified(BTreeSet::new()),
        });
    }

    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("layer_material_mask_materials", row.layer_id))
        .max_height(MATERIAL_MASK_POPUP_MAX_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (index, material) in document.materials.iter().enumerate() {
                let mut selected = row.material_mask.allows(material.id);
                let excluded_by_parent = !document
                    .layer_tree
                    .ancestor_material_allowed(row.layer_id, material.id);
                ui.horizontal(|ui| {
                    let changed = ui.checkbox(&mut selected, "").changed();
                    let (swatch_rect, _) = ui.allocate_exact_size(
                        egui::Vec2::splat(MATERIAL_MASK_POPUP_SWATCH_SIZE),
                        egui::Sense::hover(),
                    );
                    paint_material_swatch(ui, swatch_rect, material.ui_color);
                    ui.label(material_display_name(l10n, index, material));
                    if excluded_by_parent {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(l10n.text("layer-panel-excluded-by-parent"));
                        });
                    }
                    if changed {
                        *command = Some(Command::SetLayerMaterialMask {
                            layer_id: row.layer_id,
                            material_mask: toggle_material_in_mask(
                                &row.material_mask,
                                &document.materials,
                                material.id,
                            ),
                        });
                    }
                });
            }
        });
}

fn draw_single_material_popup(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    command: &mut Option<Command>,
) {
    ui.label(
        if matches!(row.content, LayerContent::EmbeddedImage { .. }) {
            l10n.text("layer-panel-embedded-image-material")
        } else {
            l10n.text("layer-panel-uv-mirror-material")
        },
    );
    ui.separator();
    let selected_material = match &row.material_mask {
        LayerMaterialMask::Specified(material_ids) if material_ids.len() == 1 => {
            material_ids.iter().next().copied()
        }
        _ => None,
    };

    egui::ScrollArea::vertical()
        .id_salt(("single_layer_materials", row.layer_id))
        .max_height(MATERIAL_MASK_POPUP_MAX_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (index, material) in document.materials.iter().enumerate() {
                let excluded_by_parent = !document
                    .layer_tree
                    .ancestor_material_allowed(row.layer_id, material.id);
                ui.horizontal(|ui| {
                    let selected = selected_material == Some(material.id);
                    if ui.radio(selected, "").clicked() {
                        *command = Some(Command::SetLayerMaterialMask {
                            layer_id: row.layer_id,
                            material_mask: LayerMaterialMask::Specified(
                                [material.id].into_iter().collect(),
                            ),
                        });
                    }
                    let (swatch_rect, _) = ui.allocate_exact_size(
                        egui::Vec2::splat(MATERIAL_MASK_POPUP_SWATCH_SIZE),
                        egui::Sense::hover(),
                    );
                    paint_material_swatch(ui, swatch_rect, material.ui_color);
                    ui.label(material_display_name(l10n, index, material));
                    if excluded_by_parent {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.weak(l10n.text("layer-panel-excluded-by-parent"));
                        });
                    }
                });
            }
        });
}

fn specified_all_materials(materials: &[MaterialData]) -> LayerMaterialMask {
    LayerMaterialMask::Specified(materials.iter().map(|material| material.id).collect())
}

fn toggle_material_in_mask(
    current: &LayerMaterialMask,
    materials: &[MaterialData],
    target: MaterialId,
) -> LayerMaterialMask {
    let mut selected = match current {
        LayerMaterialMask::Unspecified => materials
            .iter()
            .map(|material| material.id)
            .collect::<BTreeSet<_>>(),
        LayerMaterialMask::Specified(selected) => selected.clone(),
    };
    if !selected.remove(&target) {
        selected.insert(target);
    }
    LayerMaterialMask::Specified(selected)
}

fn paint_material_mask_summary(
    ui: &egui::Ui,
    rect: egui::Rect,
    summary: &MaterialMaskSummary,
    row_hovered: bool,
) {
    if summary.show_as_unrestricted && !row_hovered {
        return;
    }

    let center = rect.center();
    let radius = (ROW_MATERIAL_MASK_BADGE_SIZE - ROW_MATERIAL_MASK_RING_WIDTH) * 0.5;
    let outline = ui.visuals().widgets.noninteractive.fg_stroke.color;

    if summary.show_as_unrestricted {
        ui.painter().circle_stroke(
            center,
            radius,
            egui::Stroke::new(ROW_MATERIAL_MASK_RING_WIDTH, translucent(outline, 72)),
        );
        return;
    }

    ui.painter().circle_stroke(
        center,
        radius,
        egui::Stroke::new(ROW_MATERIAL_MASK_RING_WIDTH, translucent(outline, 72)),
    );
    paint_material_mask_ring(ui.painter(), center, radius, &summary.representative_colors);

    let count = compact_material_count(summary.specified_count);
    let font_size = if count.len() > 2 { 9.0 } else { 10.0 };
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        count,
        egui::FontId::proportional(font_size),
        ui.visuals().text_color(),
    );
}

fn paint_material_mask_ring(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    colors: &[MaterialUiColor],
) {
    if colors.is_empty() {
        return;
    }
    if colors.len() == 1 {
        let color = colors[0];
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(
                ROW_MATERIAL_MASK_RING_WIDTH,
                egui::Color32::from_rgb(color.rgb[0], color.rgb[1], color.rgb[2]),
            ),
        );
        return;
    }

    let segment_count = ROW_MATERIAL_MASK_RING_SEGMENTS.max(colors.len());
    for segment in 0..segment_count {
        let color = colors[segment * colors.len() / segment_count];
        let start_angle = -std::f32::consts::FRAC_PI_2
            + std::f32::consts::TAU * segment as f32 / segment_count as f32;
        let end_angle = -std::f32::consts::FRAC_PI_2
            + std::f32::consts::TAU * (segment + 1) as f32 / segment_count as f32;
        let start = center + egui::vec2(start_angle.cos() * radius, start_angle.sin() * radius);
        let end = center + egui::vec2(end_angle.cos() * radius, end_angle.sin() * radius);
        painter.line_segment(
            [start, end],
            egui::Stroke::new(
                ROW_MATERIAL_MASK_RING_WIDTH,
                egui::Color32::from_rgb(color.rgb[0], color.rgb[1], color.rgb[2]),
            ),
        );
    }
}

fn compact_material_count(count: usize) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

fn draw_material_mask_tooltip(
    ui: &mut egui::Ui,
    l10n: &Localization,
    document: &Document,
    row: &LayerRowVm,
    summary: &MaterialMaskSummary,
) {
    if !summary.specified {
        ui.label(l10n.text("layer-panel-all-future-materials"));
        if summary.effective_count != document.materials.len() {
            ui.separator();
            let mut args = fluent::FluentArgs::new();
            args.set("count", summary.effective_count as i64);
            ui.label(l10n.format("layer-panel-after-parent-groups", Some(&args)));
        }
        return;
    }

    if summary.show_as_unrestricted {
        ui.label(l10n.text("layer-panel-all-current-materials"));
        ui.weak(l10n.text("layer-panel-new-materials-not-selected"));
    } else if summary.specified_count == 0 {
        ui.label(l10n.text("layer-panel-no-materials-selected"));
    } else {
        let mut args = fluent::FluentArgs::new();
        args.set("count", summary.specified_count as i64);
        ui.label(l10n.format("layer-panel-materials-selected", Some(&args)));
        for (index, material) in document
            .materials
            .iter()
            .enumerate()
            .filter(|(_, material)| row.material_mask.allows(material.id))
            .take(10)
        {
            ui.label(material_display_name(l10n, index, material));
        }
        if summary.specified_count > 10 {
            let mut args = fluent::FluentArgs::new();
            args.set("count", (summary.specified_count - 10) as i64);
            ui.label(l10n.format("layer-panel-and-more", Some(&args)));
        }
    }

    if summary.effective_count != summary.specified_count {
        ui.separator();
        let mut args = fluent::FluentArgs::new();
        args.set("count", summary.effective_count as i64);
        ui.label(l10n.format("layer-panel-after-parent-groups", Some(&args)));
    }
}

fn material_display_name(l10n: &Localization, index: usize, material: &MaterialData) -> String {
    if material.name.is_empty() {
        let mut args = fluent::FluentArgs::new();
        args.set("index", index as i64);
        l10n.format("material-fallback-name", Some(&args))
    } else {
        material.name.clone()
    }
}

fn translucent(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn draw_layer_text(
    ui: &mut egui::Ui,
    l10n: &Localization,
    row: &LayerRowVm,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    is_drag_source: bool,
    width: f32,
) -> egui::Response {
    if ui_state
        .rename
        .as_ref()
        .is_some_and(|rename| rename.layer_id == row.layer_id)
    {
        return draw_layer_rename_editor(ui, row, ui_state, command, width);
    }

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ROW_MIN_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let response = response.on_hover_cursor(if is_drag_source {
        egui::CursorIcon::Grabbing
    } else {
        egui::CursorIcon::Default
    });

    if response.double_clicked() {
        begin_layer_rename(ui_state, row);
    }

    if ui.is_rect_visible(rect) {
        let text_pos = egui::pos2(rect.left() + 4.0, rect.top() + 4.0);
        let detail = format!(
            "{} {}%",
            layer_panel_composite_mode(
                row.blend_mode,
                row.group_composite_mode,
                matches!(row.content, LayerContent::Group { .. }),
            )
            .localized_label(l10n),
            (row.opacity.clamp(0.0, 1.0) * 100.0).round() as u32
        );
        let text_color = if is_drag_source {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        };
        let painter = ui.painter().with_clip_rect(rect);
        painter.text(
            text_pos,
            egui::Align2::LEFT_TOP,
            row.name.as_str(),
            egui::TextStyle::Body.resolve(ui.style()),
            text_color,
        );
        painter.text(
            egui::pos2(text_pos.x, text_pos.y + 15.0),
            egui::Align2::LEFT_TOP,
            detail,
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
    }

    response
}

fn begin_layer_rename(ui_state: &mut LayerPanelUiState, row: &LayerRowVm) {
    ui_state.rename = Some(LayerRenameState {
        layer_id: row.layer_id,
        buffer: row.name.clone(),
        focus_requested: false,
    });
}

fn draw_layer_rename_editor(
    ui: &mut egui::Ui,
    row: &LayerRowVm,
    ui_state: &mut LayerPanelUiState,
    command: &mut Option<Command>,
    width: f32,
) -> egui::Response {
    let (response, should_commit, should_cancel) = {
        let rename = ui_state
            .rename
            .as_mut()
            .expect("rename state must exist for active rename row");
        let response = ui.add_sized(
            egui::vec2(width, ROW_MIN_HEIGHT),
            egui::TextEdit::singleline(&mut rename.buffer).desired_width(width),
        );
        let mut focus_requested = false;
        if !rename.focus_requested {
            response.request_focus();
            rename.focus_requested = true;
            focus_requested = true;
        }

        let should_commit = (response.lost_focus() && !focus_requested)
            || ui.input(|input| input.key_pressed(egui::Key::Enter));
        let should_cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
        (response, should_commit, should_cancel)
    };

    if should_cancel {
        ui_state.rename = None;
        return response;
    }

    if should_commit {
        if let Some(rename) = ui_state.rename.take() {
            let new_name = rename.buffer.trim();
            if command.is_none() && !new_name.is_empty() && new_name != row.name {
                *command = Some(Command::RenameLayer {
                    layer_id: row.layer_id,
                    name: new_name.to_owned(),
                });
            }
        }
    }

    response
}

fn solid_fill_thumbnail_button(
    ui: &mut egui::Ui,
    size: f32,
    selected: bool,
    color: [f32; 3],
    icons: &UiIconRegistry,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        paint_thumbnail_background(ui, rect, false);
        let color = egui::Color32::from_rgb(
            (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        );
        ui.painter().rect_filled(rect.shrink(1.0), 0.0, color);
        ui.painter().rect_stroke(
            rect,
            0.0,
            if selected {
                visuals.bg_stroke
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            },
            egui::StrokeKind::Inside,
        );
        if selected {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                0.0,
                egui::Stroke::new(2.0, visuals.fg_stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        let badge_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.right() - ROW_FILL_BADGE_SIZE - ROW_FILL_BADGE_MARGIN,
                rect.top() + ROW_FILL_BADGE_MARGIN,
            ),
            egui::Vec2::splat(ROW_FILL_BADGE_SIZE),
        );
        ui.painter().rect_filled(
            badge_rect.expand(1.0),
            1.0,
            egui::Color32::from_black_alpha(128),
        );
        if let Some(texture) = icons.texture(ICON_SOLID_FILL) {
            ui.painter().image(
                texture.id(),
                badge_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
    response
}

fn icon_button(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
    button_size: f32,
    icon_size: f32,
    selected: bool,
) -> egui::Response {
    icon_button_with_tint_alpha(ui, icons, icon_id, button_size, icon_size, selected, 255)
}

fn icon_button_with_tint_alpha(
    ui: &mut egui::Ui,
    icons: &UiIconRegistry,
    icon_id: &str,
    button_size: f32,
    icon_size: f32,
    selected: bool,
    tint_alpha: u8,
) -> egui::Response {
    let button_size = egui::Vec2::splat(button_size);
    let icon_size = egui::Vec2::splat(icon_size);
    let (rect, response) = ui.allocate_exact_size(button_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let rect = rect.expand(visuals.expansion);
        let show_frame = selected
            || response.hovered()
            || response.highlighted()
            || response.has_focus()
            || response.is_pointer_button_down_on();

        if show_frame {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
            ui.painter().rect_stroke(
                rect,
                visuals.corner_radius,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }

        if let Some(texture) = icons.texture(icon_id) {
            let tint = if ui.is_enabled() {
                visuals.fg_stroke.color
            } else {
                ui.visuals().widgets.noninteractive.fg_stroke.color
            };
            let tint = translucent(tint, tint_alpha);
            let icon_rect = egui::Rect::from_center_size(rect.center(), icon_size);
            ui.painter().image(
                texture.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                tint,
            );
        }
    }

    response
}

fn thumbnail_button(
    ui: &mut egui::Ui,
    size: f32,
    selected: bool,
    is_mask: bool,
    disabled: bool,
    texture_id: Option<egui::TextureId>,
) -> egui::Response {
    let sense = if is_mask {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        paint_thumbnail_background(ui, rect, is_mask);
        if let Some(texture_id) = texture_id {
            ui.painter().image(
                texture_id,
                rect.shrink(1.0),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        ui.painter().rect_stroke(
            rect,
            0.0,
            if selected {
                visuals.bg_stroke
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            },
            egui::StrokeKind::Inside,
        );
        if selected {
            ui.painter().rect_stroke(
                rect.expand(2.0),
                0.0,
                egui::Stroke::new(2.0, visuals.fg_stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        if is_mask && disabled {
            let slash_rect = rect.shrink(4.0);
            ui.painter().line_segment(
                [slash_rect.left_bottom(), slash_rect.right_top()],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(220, 45, 55)),
            );
        }
    }
    response
}

fn paint_thumbnail_background(ui: &egui::Ui, rect: egui::Rect, is_mask: bool) {
    if is_mask {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        return;
    }

    let base = ui.visuals().widgets.noninteractive.weak_bg_fill;
    let alternate = ui.visuals().extreme_bg_color;
    ui.painter().rect_filled(rect, 3.0, base);

    let cell = 4.0;
    let mut y = rect.top();
    let mut row = 0usize;
    while y < rect.bottom() {
        let mut x = rect.left();
        let mut col = 0usize;
        while x < rect.right() {
            if (row + col) % 2 == 0 {
                let cell_rect = egui::Rect::from_min_max(
                    egui::pos2(x, y),
                    egui::pos2((x + cell).min(rect.right()), (y + cell).min(rect.bottom())),
                );
                ui.painter().rect_filled(cell_rect, 0.0, alternate);
            }
            x += cell;
            col += 1;
        }
        y += cell;
        row += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_toolbar_layout_uses_button_derived_boundaries() {
        let full_width = header_buttons_width(FULL_HEADER_BUTTON_COUNT);
        let compact_width = header_buttons_width(COMPACT_HEADER_BUTTON_COUNT);

        assert_eq!(layer_toolbar_layout(full_width), LayerToolbarLayout::Full);
        assert_eq!(
            layer_toolbar_layout(full_width - 0.1),
            LayerToolbarLayout::Compact
        );
        assert_eq!(
            layer_toolbar_layout(compact_width),
            LayerToolbarLayout::Compact
        );
        assert_eq!(
            layer_toolbar_layout(compact_width - 0.1),
            LayerToolbarLayout::OverflowOnly
        );
    }

    #[test]
    fn composite_controls_stack_below_wide_width() {
        assert_eq!(
            composite_controls_layout(COMPOSITE_CONTROLS_WIDE_WIDTH),
            CompositeControlsLayout::Horizontal
        );
        assert_eq!(
            composite_controls_layout(COMPOSITE_CONTROLS_WIDE_WIDTH - 0.1),
            CompositeControlsLayout::Vertical
        );
    }

    struct LayerTreeFixture {
        tree: LayerTree,
        base: LayerId,
        group: LayerId,
        child: LayerId,
        empty_group: LayerId,
        top: LayerId,
    }

    fn fixture() -> LayerTreeFixture {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let group = tree.add_group_above(base, "Group").unwrap();
        let child = tree.add_raster_layer_to_group(group, "Child").unwrap();
        let empty_group = tree.add_group_to_group(tree.root(), "Empty Group").unwrap();
        let top = tree.add_raster_layer_to_root("Top").unwrap();
        LayerTreeFixture {
            tree,
            base,
            group,
            child,
            empty_group,
            top,
        }
    }

    fn row_vm(layer_id: LayerId, content: LayerContent) -> LayerRowVm {
        LayerRowVm {
            layer_id,
            depth: 1,
            content,
            name: "Layer".to_owned(),
            visible: true,
            hidden_by_parent: false,
            locked: false,
            locked_by_parent: false,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
            group_composite_mode: GroupCompositeMode::Isolated,
            mask: None,
            material_mask: LayerMaterialMask::Unspecified,
            collapsed: false,
            child_count: 0,
        }
    }

    fn row_for(rows: &[LayerRowVm], layer_id: LayerId) -> &LayerRowVm {
        rows.iter()
            .find(|row| row.layer_id == layer_id)
            .expect("row should exist")
    }

    #[test]
    fn panel_composite_mode_distinguishes_pass_through_groups() {
        assert_eq!(
            layer_panel_composite_mode(
                LayerBlendMode::Multiply,
                GroupCompositeMode::PassThrough,
                true,
            ),
            LayerPanelCompositeMode::PassThrough
        );
        assert_eq!(
            layer_panel_composite_mode(
                LayerBlendMode::Multiply,
                GroupCompositeMode::PassThrough,
                false,
            ),
            LayerPanelCompositeMode::Blend(LayerBlendMode::Multiply)
        );
    }

    #[test]
    fn visible_rows_marks_children_locked_by_parent_group() {
        let mut fixture = fixture();
        assert!(fixture.tree.set_locked(fixture.group, true));

        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());

        let group_row = row_for(&rows, fixture.group);
        assert!(group_row.locked);
        assert!(!group_row.locked_by_parent);
        let child_row = row_for(&rows, fixture.child);
        assert!(!child_row.locked);
        assert!(child_row.locked_by_parent);
    }

    #[test]
    fn local_lock_takes_precedence_over_inherited_icon_state() {
        let mut fixture = fixture();
        assert!(fixture.tree.set_locked(fixture.group, true));
        assert!(fixture.tree.set_locked(fixture.child, true));

        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());
        let child_row = row_for(&rows, fixture.child);

        assert!(child_row.locked);
        assert!(child_row.locked_by_parent);
    }

    #[test]
    fn visible_rows_marks_child_hidden_by_parent_group() {
        let mut fixture = fixture();
        fixture.tree.set_visible(fixture.group, false);
        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());

        let group_row = row_for(&rows, fixture.group);
        assert!(!group_row.visible);
        assert!(!group_row.hidden_by_parent);

        let child_row = row_for(&rows, fixture.child);
        assert!(child_row.visible);
        assert!(child_row.hidden_by_parent);
    }

    #[test]
    fn visible_rows_preserves_child_hidden_state_when_parent_hidden() {
        let mut fixture = fixture();
        fixture.tree.set_visible(fixture.group, false);
        fixture.tree.set_visible(fixture.child, false);
        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());

        let child_row = row_for(&rows, fixture.child);
        assert!(!child_row.visible);
        assert!(child_row.hidden_by_parent);
    }

    #[test]
    fn visible_rows_propagates_hidden_by_ancestor_group() {
        let mut fixture = fixture();
        let nested_group = fixture
            .tree
            .add_group_to_group(fixture.group, "Nested Group")
            .unwrap();
        let nested_child = fixture
            .tree
            .add_raster_layer_to_group(nested_group, "Nested Child")
            .unwrap();
        fixture.tree.set_visible(fixture.group, false);
        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());

        assert!(row_for(&rows, nested_group).hidden_by_parent);
        assert!(row_for(&rows, nested_child).hidden_by_parent);
    }

    #[test]
    fn prune_resets_selection_when_active_changes_outside_selection() {
        let fixture = fixture();
        let mut ui_state = LayerPanelUiState::default();
        ui_state.selected_layer_ids.insert(fixture.base);
        ui_state.selected_layer_ids.insert(fixture.child);
        ui_state.selection_anchor = Some(fixture.child);

        ui_state.prune(&fixture.tree, fixture.top);

        assert_eq!(
            ui_state.selected_layer_ids,
            [fixture.top].into_iter().collect::<HashSet<_>>()
        );
        assert_eq!(ui_state.selection_anchor, Some(fixture.top));
    }

    #[test]
    fn prune_keeps_multi_selection_when_active_is_already_selected() {
        let fixture = fixture();
        let mut ui_state = LayerPanelUiState::default();
        ui_state.selected_layer_ids.insert(fixture.base);
        ui_state.selected_layer_ids.insert(fixture.top);
        ui_state.selection_anchor = Some(fixture.base);

        ui_state.prune(&fixture.tree, fixture.top);

        assert_eq!(
            ui_state.selected_layer_ids,
            [fixture.base, fixture.top]
                .into_iter()
                .collect::<HashSet<_>>()
        );
        assert_eq!(ui_state.selection_anchor, Some(fixture.base));
    }

    #[test]
    fn solid_fill_color_popup_has_one_open_layer_and_new_sessions_per_open() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let fill_a = tree
            .add_solid_fill_layer_above(base, "Fill A", [1.0, 0.0, 0.0])
            .unwrap();
        let fill_b = tree
            .add_solid_fill_layer_above(fill_a, "Fill B", [0.0, 1.0, 0.0])
            .unwrap();
        let mut ui_state = LayerPanelUiState::default();

        ui_state.toggle_solid_fill_color_popup(fill_a);
        let first_session = ui_state.solid_fill_color_session.unwrap();
        assert_eq!(ui_state.solid_fill_color_popup, Some(fill_a));
        assert_eq!(first_session.layer_id, fill_a);
        assert_ne!(first_session.edit_session, 0);

        ui_state.toggle_solid_fill_color_popup(fill_a);
        assert_eq!(ui_state.solid_fill_color_popup, None);
        assert_eq!(
            ui_state.solid_fill_color_session.unwrap().edit_session,
            first_session.edit_session
        );

        ui_state.toggle_solid_fill_color_popup(fill_a);
        let reopened_session = ui_state.solid_fill_color_session.unwrap();
        assert_eq!(ui_state.solid_fill_color_popup, Some(fill_a));
        assert_ne!(reopened_session.edit_session, first_session.edit_session);

        ui_state.toggle_solid_fill_color_popup(fill_b);
        let switched_session = ui_state.solid_fill_color_session.unwrap();
        assert_eq!(ui_state.solid_fill_color_popup, Some(fill_b));
        assert_eq!(switched_session.layer_id, fill_b);
        assert_ne!(switched_session.edit_session, reopened_session.edit_session);
    }

    #[test]
    fn prune_clears_solid_fill_popup_and_session_when_layer_is_removed() {
        let mut tree = LayerTree::new_default_raster();
        let base = tree.default_raster_layer().unwrap();
        let fill = tree
            .add_solid_fill_layer_above(base, "Fill", [0.25, 0.5, 0.75])
            .unwrap();
        let mut ui_state = LayerPanelUiState::default();
        ui_state.toggle_solid_fill_color_popup(fill);

        tree.remove_layer(fill);
        ui_state.prune(&tree, base);

        assert_eq!(ui_state.solid_fill_color_popup, None);
        assert!(ui_state.solid_fill_color_session.is_none());
    }

    #[test]
    fn drop_target_rejects_root_drag() {
        let fixture = fixture();
        assert_eq!(
            drop_target_to_move(
                &fixture.tree,
                fixture.tree.root(),
                fixture.base,
                DropPlacement::Above,
            ),
            None
        );
    }

    #[test]
    fn drop_target_rejects_self_drop() {
        let fixture = fixture();
        assert_eq!(
            drop_target_to_move(
                &fixture.tree,
                fixture.base,
                fixture.base,
                DropPlacement::Below,
            ),
            None
        );
    }

    #[test]
    fn drop_target_rejects_drop_into_descendant() {
        let fixture = fixture();
        assert_eq!(
            drop_target_to_move(
                &fixture.tree,
                fixture.group,
                fixture.child,
                DropPlacement::Into,
            ),
            None
        );
    }

    #[test]
    fn effective_selection_roots_excludes_children_of_selected_groups() {
        let fixture = fixture();
        let selected = [fixture.group, fixture.child, fixture.top]
            .into_iter()
            .collect::<HashSet<_>>();

        assert_eq!(
            effective_selection_roots(&fixture.tree, &selected),
            vec![fixture.group, fixture.top]
        );
    }

    #[test]
    fn secondary_click_replaces_selection_for_an_unselected_row() {
        let fixture = fixture();
        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());
        let row = row_for(&rows, fixture.child);
        let mut selected = [fixture.base, fixture.top]
            .into_iter()
            .collect::<HashSet<_>>();
        let mut anchor = Some(fixture.base);

        let command = update_selection_from_secondary_click(row, &mut selected, &mut anchor);

        assert_eq!(selected, [fixture.child].into_iter().collect());
        assert_eq!(anchor, Some(fixture.child));
        assert!(matches!(
            command,
            Command::SelectLayer { layer_id } if layer_id == fixture.child
        ));
    }

    #[test]
    fn secondary_click_preserves_selection_for_a_selected_row() {
        let fixture = fixture();
        let rows = visible_rows_for_panel(&fixture.tree, &HashSet::new());
        let row = row_for(&rows, fixture.top);
        let mut selected = [fixture.base, fixture.top]
            .into_iter()
            .collect::<HashSet<_>>();
        let expected = selected.clone();
        let mut anchor = Some(fixture.base);

        let command = update_selection_from_secondary_click(row, &mut selected, &mut anchor);

        assert_eq!(selected, expected);
        assert_eq!(anchor, Some(fixture.top));
        assert!(matches!(
            command,
            Command::SelectLayer { layer_id } if layer_id == fixture.top
        ));
    }

    #[test]
    fn selection_context_uses_effective_roots_for_duplicate_and_delete() {
        let fixture = fixture();
        let selected = [fixture.group, fixture.child, fixture.top]
            .into_iter()
            .collect::<HashSet<_>>();
        let context = LayerSelectionContext::new(&fixture.tree, &selected);

        assert_eq!(context.effective_roots, vec![fixture.group, fixture.top]);
        assert!(matches!(
            context.duplicate_command(fixture.top),
            Some(Command::DuplicateLayers {
                layer_ids,
                primary_layer_id,
            }) if layer_ids == vec![fixture.group, fixture.top]
                && primary_layer_id == fixture.top
        ));
        assert!(matches!(
            context.delete_command(),
            Some(Command::DeleteLayers { layer_ids })
                if layer_ids == vec![fixture.group, fixture.top]
        ));
    }

    #[test]
    fn selection_context_allows_merge_for_a_single_group_only() {
        let fixture = fixture();
        let group_selection = [fixture.group].into_iter().collect::<HashSet<_>>();
        let layer_selection = [fixture.top].into_iter().collect::<HashSet<_>>();

        let group_context = LayerSelectionContext::new(&fixture.tree, &group_selection);
        let layer_context = LayerSelectionContext::new(&fixture.tree, &layer_selection);

        assert!(group_context.can_merge);
        assert!(matches!(
            group_context.merge_command(),
            Some(Command::MergeLayers { layer_ids }) if layer_ids == vec![fixture.group]
        ));
        assert!(!layer_context.can_merge);
        assert!(layer_context.merge_command().is_none());
    }

    #[test]
    fn selection_context_unlocks_only_when_every_selected_layer_is_locally_locked() {
        let mut fixture = fixture();
        let selected = [fixture.base, fixture.top]
            .into_iter()
            .collect::<HashSet<_>>();
        assert!(fixture.tree.set_locked(fixture.base, true));

        let mixed_context = LayerSelectionContext::new(&fixture.tree, &selected);
        assert!(!mixed_context.all_locally_locked);
        assert!(matches!(
            mixed_context.lock_command(),
            Some(Command::SetLayersLocked { locked: true, .. })
        ));

        assert!(fixture.tree.set_locked(fixture.top, true));
        let locked_context = LayerSelectionContext::new(&fixture.tree, &selected);
        assert!(locked_context.all_locally_locked);
        assert!(matches!(
            locked_context.lock_command(),
            Some(Command::SetLayersLocked { locked: false, .. })
        ));
    }

    #[test]
    fn selection_context_keeps_the_last_raster_layer_undeletable() {
        let tree = LayerTree::new_default_raster();
        let raster = tree.default_raster_layer().unwrap();
        let selected = [raster].into_iter().collect::<HashSet<_>>();
        let context = LayerSelectionContext::new(&tree, &selected);

        assert!(!context.can_delete);
        assert!(context.delete_command().is_none());
    }

    #[test]
    fn drop_target_moves_multiple_roots_into_group() {
        let fixture = fixture();

        assert_eq!(
            drop_target_to_move_many(
                &fixture.tree,
                &[fixture.base, fixture.top],
                fixture.empty_group,
                DropPlacement::Into,
            ),
            Some((fixture.empty_group, 0))
        );
    }

    #[test]
    fn drop_target_rejects_multiple_drop_inside_moved_group() {
        let fixture = fixture();

        assert_eq!(
            drop_target_to_move_many(
                &fixture.tree,
                &[fixture.group, fixture.top],
                fixture.child,
                DropPlacement::Below,
            ),
            None
        );
    }

    #[test]
    fn drop_target_allows_drop_into_empty_group() {
        let fixture = fixture();
        assert_eq!(
            drop_target_to_move(
                &fixture.tree,
                fixture.base,
                fixture.empty_group,
                DropPlacement::Into,
            ),
            Some((fixture.empty_group, 0))
        );
    }

    #[test]
    fn drop_target_noops_for_same_visual_position_above() {
        let fixture = fixture();
        assert_eq!(
            drop_target_to_move(
                &fixture.tree,
                fixture.top,
                fixture.empty_group,
                DropPlacement::Above,
            ),
            None
        );
    }

    #[test]
    fn drop_target_maps_visual_above_to_higher_internal_index() {
        let fixture = fixture();
        assert_eq!(
            drop_target_to_move(
                &fixture.tree,
                fixture.base,
                fixture.empty_group,
                DropPlacement::Above,
            ),
            Some((fixture.tree.root(), 2))
        );
    }

    #[test]
    fn group_drop_placement_uses_middle_band_for_into() {
        let fixture = fixture();
        let row = row_vm(
            fixture.group,
            LayerContent::Group {
                children: Vec::new(),
                composite_mode: GroupCompositeMode::Isolated,
            },
        );
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));

        assert_eq!(
            drop_placement_for_pointer(&row, rect, egui::pos2(50.0, 10.0)),
            DropPlacement::Above
        );
        assert_eq!(
            drop_placement_for_pointer(&row, rect, egui::pos2(50.0, 50.0)),
            DropPlacement::Into
        );
        assert_eq!(
            drop_placement_for_pointer(&row, rect, egui::pos2(50.0, 90.0)),
            DropPlacement::Below
        );
    }

    #[test]
    fn raster_drop_placement_uses_two_bands() {
        let fixture = fixture();
        let row = row_vm(fixture.base, LayerContent::Raster);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));

        assert_eq!(
            drop_placement_for_pointer(&row, rect, egui::pos2(50.0, 10.0)),
            DropPlacement::Above
        );
        assert_eq!(
            drop_placement_for_pointer(&row, rect, egui::pos2(50.0, 90.0)),
            DropPlacement::Below
        );
    }

    fn material(id: u64, name: &str, rgb: [u8; 3]) -> MaterialData {
        MaterialData {
            id: MaterialId(id),
            name: name.to_owned(),
            texture_size: [8, 8],
            ui_color: MaterialUiColor { rgb },
            render_settings: Default::default(),
            export_image_file_name: None,
            export_psd_file_name: None,
        }
    }

    #[test]
    fn material_mask_summary_hides_unspecified_and_explicit_all_in_layer_rows() {
        let materials = vec![
            material(1, "A", [10, 20, 30]),
            material(2, "B", [40, 50, 60]),
        ];
        let unspecified =
            summarize_material_mask(&LayerMaterialMask::Unspecified, &materials, |_| true);
        let explicit_all = summarize_material_mask(
            &LayerMaterialMask::Specified(std::collections::BTreeSet::from([
                MaterialId(1),
                MaterialId(2),
            ])),
            &materials,
            |_| true,
        );
        let explicit_partial = summarize_material_mask(
            &LayerMaterialMask::Specified(std::collections::BTreeSet::from([MaterialId(1)])),
            &materials,
            |_| true,
        );
        let explicit_none = summarize_material_mask(
            &LayerMaterialMask::Specified(BTreeSet::new()),
            &materials,
            |_| false,
        );

        assert!(!unspecified.specified);
        assert_eq!(unspecified.specified_count, 0);
        assert!(unspecified.representative_colors.is_empty());
        assert_eq!(unspecified.effective_count, 2);
        assert!(unspecified.show_as_unrestricted);
        assert!(explicit_all.specified);
        assert_eq!(explicit_all.specified_count, 2);
        assert!(explicit_all.show_as_unrestricted);
        assert_eq!(
            explicit_all.representative_colors,
            vec![
                MaterialUiColor { rgb: [10, 20, 30] },
                MaterialUiColor { rgb: [40, 50, 60] },
            ]
        );
        assert!(!explicit_partial.show_as_unrestricted);
        assert!(!explicit_none.show_as_unrestricted);
    }

    #[test]
    fn material_mask_summary_uses_material_order_and_limits_representative_colors() {
        let materials = vec![
            material(3, "C", [30, 0, 0]),
            material(1, "A", [10, 0, 0]),
            material(4, "D", [40, 0, 0]),
            material(2, "B", [20, 0, 0]),
            material(5, "E", [50, 0, 0]),
            material(6, "F", [60, 0, 0]),
            material(7, "G", [70, 0, 0]),
            material(8, "H", [80, 0, 0]),
            material(9, "I", [90, 0, 0]),
        ];
        let summary = summarize_material_mask(
            &LayerMaterialMask::Specified(std::collections::BTreeSet::from([
                MaterialId(1),
                MaterialId(2),
                MaterialId(3),
                MaterialId(4),
                MaterialId(5),
                MaterialId(6),
                MaterialId(7),
                MaterialId(8),
                MaterialId(9),
            ])),
            &materials,
            |material_id| material_id == MaterialId(1),
        );

        assert_eq!(summary.specified_count, 9);
        assert_eq!(summary.effective_count, 1);
        assert!(summary.show_as_unrestricted);
        assert_eq!(
            summary.representative_colors,
            vec![
                MaterialUiColor { rgb: [30, 0, 0] },
                MaterialUiColor { rgb: [10, 0, 0] },
                MaterialUiColor { rgb: [40, 0, 0] },
                MaterialUiColor { rgb: [20, 0, 0] },
                MaterialUiColor { rgb: [50, 0, 0] },
                MaterialUiColor { rgb: [60, 0, 0] },
                MaterialUiColor { rgb: [70, 0, 0] },
                MaterialUiColor { rgb: [80, 0, 0] },
            ]
        );
    }

    #[test]
    fn material_mask_toggle_converts_unspecified_to_explicit_selection() {
        let materials = vec![
            material(1, "A", [10, 20, 30]),
            material(2, "B", [40, 50, 60]),
            material(3, "C", [70, 80, 90]),
        ];

        assert_eq!(
            toggle_material_in_mask(&LayerMaterialMask::Unspecified, &materials, MaterialId(2),),
            LayerMaterialMask::Specified(BTreeSet::from([MaterialId(1), MaterialId(3),]))
        );
    }

    #[test]
    fn material_mask_toggle_keeps_explicit_empty_state() {
        let materials = vec![material(1, "A", [10, 20, 30])];
        let selected = toggle_material_in_mask(
            &LayerMaterialMask::Specified(BTreeSet::new()),
            &materials,
            MaterialId(1),
        );
        assert_eq!(
            selected,
            LayerMaterialMask::Specified(BTreeSet::from([MaterialId(1)]))
        );
        assert_eq!(
            toggle_material_in_mask(&selected, &materials, MaterialId(1)),
            LayerMaterialMask::Specified(BTreeSet::new())
        );
    }

    #[test]
    fn selecting_all_current_materials_remains_explicit() {
        let materials = vec![
            material(1, "A", [10, 20, 30]),
            material(2, "B", [40, 50, 60]),
        ];

        assert_eq!(
            specified_all_materials(&materials),
            LayerMaterialMask::Specified(BTreeSet::from([MaterialId(1), MaterialId(2),]))
        );
    }
}
