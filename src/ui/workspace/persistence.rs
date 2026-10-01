use std::{collections::HashSet, sync::LazyLock};

use eframe::egui;
use egui_tiles::{Container, Grid, GridLayout, Linear, LinearDir, Tabs, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use crate::persistence::parse_ron;

use super::{WorkspacePane, WorkspaceState};

pub(crate) const WORKSPACE_SCHEMA_VERSION: u32 = 1;
const BUILTIN_WORKSPACE_RESOURCE: &str = "workspace/default.workspace.ron";
const SHARE_PRECISION: f32 = 10_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceFileV1 {
    pub schema_version: u32,
    pub window: WorkspaceWindowV1,
    pub root: WorkspaceTileV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceWindowV1 {
    pub position: Option<WorkspaceWindowPositionV1>,
    pub inner_size: Option<WorkspaceWindowSizeV1>,
    pub maximized: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceWindowPositionV1 {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceWindowSizeV1 {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum WorkspaceTileV1 {
    Pane {
        pane: WorkspacePaneIdV1,
        visible: bool,
    },
    Tabs {
        children: Vec<WorkspaceTileV1>,
        active: Option<usize>,
        visible: bool,
    },
    Linear {
        direction: WorkspaceLinearDirectionV1,
        children: Vec<WorkspaceTileV1>,
        shares: Vec<f32>,
        visible: bool,
    },
    Grid {
        layout: WorkspaceGridLayoutV1,
        children: Vec<WorkspaceTileV1>,
        column_shares: Vec<f32>,
        row_shares: Vec<f32>,
        visible: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspacePaneIdV1 {
    ToolList,
    ToolProperties,
    Viewport3d,
    UvView,
    Color,
    MaterialsTextures,
    Meshes,
    Layers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceLinearDirectionV1 {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WorkspaceGridLayoutV1 {
    Auto,
    Columns(usize),
}

static BUILTIN_WORKSPACE: LazyLock<Result<WorkspaceFileV1, String>> = LazyLock::new(|| {
    let workspace: WorkspaceFileV1 = parse_ron(
        crate::embedded_resources::text(BUILTIN_WORKSPACE_RESOURCE)?,
        "embedded default workspace",
    )?;
    workspace.validate()?;
    Ok(workspace)
});

impl WorkspaceFileV1 {
    pub(crate) fn builtin() -> Result<Self, String> {
        (*BUILTIN_WORKSPACE).clone()
    }

    pub(crate) fn capture(
        state: &WorkspaceState,
        window: &WorkspaceWindowV1,
    ) -> Result<Self, String> {
        let root_id = state
            .tree
            .root
            .ok_or_else(|| "workspace tree has no root".to_owned())?;
        let root = capture_tile(&state.tree, root_id)?;
        let file = Self {
            schema_version: WORKSPACE_SCHEMA_VERSION,
            window: window.clone(),
            root,
        };
        file.validate()?;
        Ok(file)
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != WORKSPACE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported workspace schema version {}",
                self.schema_version
            ));
        }

        self.window.validate()?;

        let mut panes = HashSet::new();
        validate_tile(&self.root, &mut panes)?;
        if panes.len() != WorkspacePane::ALL.len() {
            return Err(format!(
                "workspace must contain exactly {} panes, found {}",
                WorkspacePane::ALL.len(),
                panes.len()
            ));
        }
        for pane in WorkspacePane::ALL {
            let id = WorkspacePaneIdV1::from(pane);
            if !panes.contains(&id) {
                return Err(format!("workspace is missing pane {:?}", pane));
            }
        }
        Ok(())
    }

    pub(crate) fn build_state(&self) -> Result<WorkspaceState, String> {
        self.validate()?;
        let mut tiles = Tiles::default();
        let root = build_tile(&self.root, &mut tiles)?;
        Ok(WorkspaceState {
            tree: Tree::new("holopainter_workspace", root, tiles),
            layer_panel: Default::default(),
            adjustment_editor: Default::default(),
            tool_options: Default::default(),
            toolbox: Default::default(),
        })
    }
}

impl WorkspaceWindowV1 {
    fn validate(&self) -> Result<(), String> {
        if let Some(position) = self.position
            && (!position.x.is_finite() || !position.y.is_finite())
        {
            return Err("workspace window position must be finite".to_owned());
        }
        if let Some(inner_size) = self.inner_size
            && (!inner_size.width.is_finite()
                || !inner_size.height.is_finite()
                || inner_size.width <= 0.0
                || inner_size.height <= 0.0)
        {
            return Err("workspace window inner size must be finite and positive".to_owned());
        }
        Ok(())
    }

    pub(crate) fn observe_viewport(&mut self, viewport: &egui::ViewportInfo) {
        if viewport.minimized != Some(false) || viewport.fullscreen == Some(true) {
            return;
        }

        match viewport.maximized {
            Some(true) => {
                self.maximized = Some(true);
            }
            Some(false) => {
                if let Some(inner_rect) = viewport.inner_rect {
                    let size = inner_rect.size();
                    if size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0 {
                        self.inner_size = Some(WorkspaceWindowSizeV1 {
                            width: size.x,
                            height: size.y,
                        });
                    }
                }
                if let Some(outer_rect) = viewport.outer_rect {
                    if outer_rect.min.x.is_finite() && outer_rect.min.y.is_finite() {
                        self.position = Some(WorkspaceWindowPositionV1 {
                            x: outer_rect.min.x,
                            y: outer_rect.min.y,
                        });
                    }
                }
                self.maximized = Some(false);
            }
            None => {}
        }
    }
}

fn capture_tile(tree: &Tree<WorkspacePane>, tile_id: TileId) -> Result<WorkspaceTileV1, String> {
    let visible = tree.is_visible(tile_id);
    let tile = tree
        .tiles
        .get(tile_id)
        .ok_or_else(|| format!("workspace tile {:?} is missing", tile_id))?;
    match tile {
        Tile::Pane(pane) => Ok(WorkspaceTileV1::Pane {
            pane: (*pane).into(),
            visible,
        }),
        Tile::Container(Container::Tabs(tabs)) => {
            let children = capture_children(tree, &tabs.children)?;
            let active = tabs
                .active
                .and_then(|active| tabs.children.iter().position(|child| *child == active));
            Ok(WorkspaceTileV1::Tabs {
                children,
                active,
                visible,
            })
        }
        Tile::Container(Container::Linear(linear)) => {
            let children = capture_children(tree, &linear.children)?;
            let shares = linear
                .children
                .iter()
                .map(|child| stable_share(linear.shares[*child]))
                .collect();
            Ok(WorkspaceTileV1::Linear {
                direction: linear.dir.into(),
                children,
                shares,
                visible,
            })
        }
        Tile::Container(Container::Grid(grid)) => {
            let child_ids: Vec<_> = grid.children().copied().collect();
            let children = capture_children(tree, &child_ids)?;
            Ok(WorkspaceTileV1::Grid {
                layout: grid.layout.into(),
                children,
                column_shares: grid.col_shares.iter().copied().map(stable_share).collect(),
                row_shares: grid.row_shares.iter().copied().map(stable_share).collect(),
                visible,
            })
        }
    }
}

fn capture_children(
    tree: &Tree<WorkspacePane>,
    children: &[TileId],
) -> Result<Vec<WorkspaceTileV1>, String> {
    children
        .iter()
        .map(|child| capture_tile(tree, *child))
        .collect()
}

fn stable_share(value: f32) -> f32 {
    (value * SHARE_PRECISION).round() / SHARE_PRECISION
}

fn validate_tile(
    tile: &WorkspaceTileV1,
    panes: &mut HashSet<WorkspacePaneIdV1>,
) -> Result<(), String> {
    match tile {
        WorkspaceTileV1::Pane { pane, .. } => {
            if !panes.insert(*pane) {
                return Err(format!("workspace contains duplicate pane {pane:?}"));
            }
        }
        WorkspaceTileV1::Tabs {
            children, active, ..
        } => {
            validate_children(children, panes)?;
            if children.is_empty() {
                return Err("workspace tabs container must not be empty".to_owned());
            }
            if active.is_some_and(|index| index >= children.len()) {
                return Err("workspace tabs active index is out of range".to_owned());
            }
        }
        WorkspaceTileV1::Linear {
            children, shares, ..
        } => {
            validate_children(children, panes)?;
            if children.is_empty() {
                return Err("workspace linear container must not be empty".to_owned());
            }
            if shares.len() != children.len() {
                return Err("workspace linear share count does not match children".to_owned());
            }
            validate_shares(shares, "linear")?;
        }
        WorkspaceTileV1::Grid {
            layout,
            children,
            column_shares,
            row_shares,
            ..
        } => {
            validate_children(children, panes)?;
            if children.is_empty() {
                return Err("workspace grid container must not be empty".to_owned());
            }
            if matches!(layout, WorkspaceGridLayoutV1::Columns(0)) {
                return Err("workspace grid column count must be positive".to_owned());
            }
            validate_shares(column_shares, "grid column")?;
            validate_shares(row_shares, "grid row")?;
        }
    }
    Ok(())
}

fn validate_children(
    children: &[WorkspaceTileV1],
    panes: &mut HashSet<WorkspacePaneIdV1>,
) -> Result<(), String> {
    for child in children {
        validate_tile(child, panes)?;
    }
    Ok(())
}

fn validate_shares(shares: &[f32], label: &str) -> Result<(), String> {
    if shares
        .iter()
        .any(|share| !share.is_finite() || *share <= 0.0)
    {
        return Err(format!(
            "workspace {label} shares must be finite and positive"
        ));
    }
    Ok(())
}

fn build_tile(tile: &WorkspaceTileV1, tiles: &mut Tiles<WorkspacePane>) -> Result<TileId, String> {
    let (id, visible) = match tile {
        WorkspaceTileV1::Pane { pane, visible } => (tiles.insert_pane((*pane).into()), *visible),
        WorkspaceTileV1::Tabs {
            children,
            active,
            visible,
        } => {
            let child_ids = build_children(children, tiles)?;
            let active_id = active.map(|index| child_ids[index]);
            let id = tiles.insert_container(Tabs {
                children: child_ids,
                active: active_id,
            });
            (id, *visible)
        }
        WorkspaceTileV1::Linear {
            direction,
            children,
            shares,
            visible,
        } => {
            let child_ids = build_children(children, tiles)?;
            let mut linear = Linear::new((*direction).into(), child_ids.clone());
            for (child, share) in child_ids.into_iter().zip(shares.iter().copied()) {
                linear.shares.set_share(child, share);
            }
            (tiles.insert_container(linear), *visible)
        }
        WorkspaceTileV1::Grid {
            layout,
            children,
            column_shares,
            row_shares,
            visible,
        } => {
            let child_ids = build_children(children, tiles)?;
            let mut grid = Grid::new(child_ids);
            grid.layout = (*layout).into();
            grid.col_shares = column_shares.clone();
            grid.row_shares = row_shares.clone();
            (tiles.insert_container(grid), *visible)
        }
    };
    tiles.set_visible(id, visible);
    Ok(id)
}

fn build_children(
    children: &[WorkspaceTileV1],
    tiles: &mut Tiles<WorkspacePane>,
) -> Result<Vec<TileId>, String> {
    children
        .iter()
        .map(|child| build_tile(child, tiles))
        .collect()
}

impl From<WorkspacePane> for WorkspacePaneIdV1 {
    fn from(value: WorkspacePane) -> Self {
        match value {
            WorkspacePane::ToolList => Self::ToolList,
            WorkspacePane::ToolProperties => Self::ToolProperties,
            WorkspacePane::Viewport3d => Self::Viewport3d,
            WorkspacePane::UvView => Self::UvView,
            WorkspacePane::Color => Self::Color,
            WorkspacePane::MaterialsTextures => Self::MaterialsTextures,
            WorkspacePane::Meshes => Self::Meshes,
            WorkspacePane::Layers => Self::Layers,
        }
    }
}

impl From<WorkspacePaneIdV1> for WorkspacePane {
    fn from(value: WorkspacePaneIdV1) -> Self {
        match value {
            WorkspacePaneIdV1::ToolList => Self::ToolList,
            WorkspacePaneIdV1::ToolProperties => Self::ToolProperties,
            WorkspacePaneIdV1::Viewport3d => Self::Viewport3d,
            WorkspacePaneIdV1::UvView => Self::UvView,
            WorkspacePaneIdV1::Color => Self::Color,
            WorkspacePaneIdV1::MaterialsTextures => Self::MaterialsTextures,
            WorkspacePaneIdV1::Meshes => Self::Meshes,
            WorkspacePaneIdV1::Layers => Self::Layers,
        }
    }
}

impl From<LinearDir> for WorkspaceLinearDirectionV1 {
    fn from(value: LinearDir) -> Self {
        match value {
            LinearDir::Horizontal => Self::Horizontal,
            LinearDir::Vertical => Self::Vertical,
        }
    }
}

impl From<WorkspaceLinearDirectionV1> for LinearDir {
    fn from(value: WorkspaceLinearDirectionV1) -> Self {
        match value {
            WorkspaceLinearDirectionV1::Horizontal => Self::Horizontal,
            WorkspaceLinearDirectionV1::Vertical => Self::Vertical,
        }
    }
}

impl From<GridLayout> for WorkspaceGridLayoutV1 {
    fn from(value: GridLayout) -> Self {
        match value {
            GridLayout::Auto => Self::Auto,
            GridLayout::Columns(columns) => Self::Columns(columns),
        }
    }
}

impl From<WorkspaceGridLayoutV1> for GridLayout {
    fn from(value: WorkspaceGridLayoutV1) -> Self {
        match value {
            WorkspaceGridLayoutV1::Auto => Self::Auto,
            WorkspaceGridLayoutV1::Columns(columns) => Self::Columns(columns),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin_window() -> WorkspaceWindowV1 {
        WorkspaceFileV1::builtin()
            .expect("builtin workspace")
            .window
    }

    fn window_with_geometry() -> WorkspaceWindowV1 {
        WorkspaceWindowV1 {
            position: Some(WorkspaceWindowPositionV1 { x: 120.0, y: 80.0 }),
            inner_size: Some(WorkspaceWindowSizeV1 {
                width: 1400.0,
                height: 900.0,
            }),
            maximized: Some(false),
        }
    }

    #[test]
    fn builtin_workspace_is_valid_and_roundtrips() {
        let workspace = WorkspaceFileV1::builtin().expect("builtin workspace");
        let encoded = ron::ser::to_string(&workspace).expect("serialize workspace");
        let decoded: WorkspaceFileV1 = ron::from_str(&encoded).expect("deserialize workspace");
        assert_eq!(decoded, workspace);
        decoded.validate().expect("roundtripped workspace validate");
        assert_eq!(WorkspacePane::ALL.len(), 8);
        assert_eq!(workspace.schema_version, 1);
        assert_eq!(workspace.window.position, None);
        assert_eq!(
            workspace.window.inner_size,
            Some(WorkspaceWindowSizeV1 {
                width: 1200.0,
                height: 720.0,
            })
        );
        assert_eq!(workspace.window.maximized, None);
    }

    #[test]
    fn workspace_default_matches_embedded_resource() {
        let builtin = WorkspaceFileV1::builtin().expect("builtin workspace");
        let state = WorkspaceState::default();
        let captured =
            WorkspaceFileV1::capture(&state, &builtin.window).expect("capture default workspace");
        assert_eq!(captured, builtin);
    }

    #[test]
    fn default_layout_round_trips_through_persistent_snapshot() {
        let state = WorkspaceState::default();
        let window = window_with_geometry();
        let snapshot =
            WorkspaceFileV1::capture(&state, &window).expect("capture default workspace");
        let restored = snapshot.build_state().expect("restore default workspace");
        let restored_snapshot =
            WorkspaceFileV1::capture(&restored, &window).expect("capture restored workspace");
        assert_eq!(snapshot, restored_snapshot);
    }

    #[test]
    fn pane_visibility_round_trips() {
        let mut state = WorkspaceState::default();
        state.set_pane_visible(WorkspacePane::Meshes, false);
        state.set_pane_visible(WorkspacePane::Layers, false);

        let snapshot =
            WorkspaceFileV1::capture(&state, &builtin_window()).expect("capture workspace");
        let restored = snapshot.build_state().expect("restore workspace");

        assert!(!restored.pane_visible(WorkspacePane::Meshes));
        assert!(!restored.pane_visible(WorkspacePane::Layers));
    }

    #[test]
    fn window_validation_accepts_valid_values_and_rejects_invalid_geometry() {
        let valid = window_with_geometry();
        valid.validate().expect("valid window geometry");

        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mut window = valid.clone();
            window.inner_size.as_mut().unwrap().width = width;
            assert!(window.validate().is_err(), "width {width:?} must fail");
        }
        for height in [0.0, -1.0, f32::NAN, f32::NEG_INFINITY] {
            let mut window = valid.clone();
            window.inner_size.as_mut().unwrap().height = height;
            assert!(window.validate().is_err(), "height {height:?} must fail");
        }
        for x in [f32::NAN, f32::INFINITY] {
            let mut window = valid.clone();
            window.position.as_mut().unwrap().x = x;
            assert!(window.validate().is_err(), "position x {x:?} must fail");
        }
        for y in [f32::NAN, f32::NEG_INFINITY] {
            let mut window = valid.clone();
            window.position.as_mut().unwrap().y = y;
            assert!(window.validate().is_err(), "position y {y:?} must fail");
        }
    }

    #[test]
    fn normal_viewport_updates_window_geometry() {
        let mut window = builtin_window();
        let viewport = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(
                egui::pos2(130.0, 110.0),
                egui::vec2(1400.0, 900.0),
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                egui::pos2(120.0, 80.0),
                egui::vec2(1420.0, 940.0),
            )),
            minimized: Some(false),
            maximized: Some(false),
            ..Default::default()
        };

        window.observe_viewport(&viewport);

        assert_eq!(window, window_with_geometry());
    }

    #[test]
    fn maximized_and_minimized_viewports_preserve_normal_geometry() {
        let mut window = window_with_geometry();
        let maximized = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2560.0, 1440.0),
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2560.0, 1440.0),
            )),
            minimized: Some(false),
            maximized: Some(true),
            ..Default::default()
        };
        window.observe_viewport(&maximized);

        let mut expected = window_with_geometry();
        expected.maximized = Some(true);
        assert_eq!(window, expected);

        let minimized = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(
                egui::pos2(-32000.0, -32000.0),
                egui::vec2(160.0, 28.0),
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                egui::pos2(-32000.0, -32000.0),
                egui::vec2(160.0, 28.0),
            )),
            minimized: Some(true),
            maximized: Some(false),
            ..Default::default()
        };
        window.observe_viewport(&minimized);
        assert_eq!(window, expected);
    }

    #[test]
    fn missing_viewport_values_do_not_discard_saved_window_state() {
        let expected = window_with_geometry();
        let mut window = expected.clone();
        window.observe_viewport(&egui::ViewportInfo {
            minimized: Some(false),
            maximized: None,
            inner_rect: None,
            outer_rect: None,
            ..Default::default()
        });
        assert_eq!(window, expected);
    }

    #[test]
    fn partial_normal_viewport_updates_only_valid_available_geometry() {
        let mut window = window_with_geometry();
        window.observe_viewport(&egui::ViewportInfo {
            minimized: Some(false),
            maximized: Some(false),
            inner_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(0.0, 720.0),
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                egui::pos2(240.0, 160.0),
                egui::vec2(1000.0, 760.0),
            )),
            ..Default::default()
        });

        assert_eq!(
            window.inner_size,
            window_with_geometry().inner_size,
            "invalid transient size must not replace normal size"
        );
        assert_eq!(
            window.position,
            Some(WorkspaceWindowPositionV1 { x: 240.0, y: 160.0 })
        );
        assert_eq!(window.maximized, Some(false));
    }

    #[test]
    fn fullscreen_viewport_does_not_replace_normal_geometry() {
        let expected = window_with_geometry();
        let mut window = expected.clone();
        window.observe_viewport(&egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2560.0, 1440.0),
            )),
            outer_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2560.0, 1440.0),
            )),
            minimized: Some(false),
            maximized: Some(false),
            fullscreen: Some(true),
            ..Default::default()
        });
        assert_eq!(window, expected);
    }

    #[test]
    fn capture_includes_window_and_dock_layout() {
        let mut state = WorkspaceState::default();
        state.set_pane_visible(WorkspacePane::Meshes, false);
        let window = window_with_geometry();

        let snapshot = WorkspaceFileV1::capture(&state, &window).expect("capture workspace");

        assert_eq!(snapshot.window, window);
        let restored = snapshot.build_state().expect("restore workspace");
        assert!(!restored.pane_visible(WorkspacePane::Meshes));
    }

    #[test]
    fn invalid_workspace_rejects_duplicate_panes_and_bad_shares() {
        let state = WorkspaceState::default();
        let window = builtin_window();
        let mut snapshot = WorkspaceFileV1::capture(&state, &window).expect("capture workspace");

        let WorkspaceTileV1::Linear { shares, .. } = &mut snapshot.root else {
            panic!("default root must be linear");
        };
        shares[0] = f32::NAN;
        assert!(snapshot.validate().is_err());

        let mut duplicate = WorkspaceFileV1::capture(&state, &window).expect("capture workspace");
        duplicate.root = WorkspaceTileV1::Tabs {
            children: vec![
                WorkspaceTileV1::Pane {
                    pane: WorkspacePaneIdV1::ToolList,
                    visible: true,
                },
                WorkspaceTileV1::Pane {
                    pane: WorkspacePaneIdV1::ToolList,
                    visible: true,
                },
            ],
            active: Some(0),
            visible: true,
        };
        assert!(duplicate.validate().is_err());
    }
}
