/// Presentation resources that must be synchronized after renderer work.
///
/// Mutation logs derive this from changed renderer state, while queued
/// presentation output requests derive the same dirty channels from outputs that
/// need to be prepared and published to the UI.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentDirty {
    material_textures: bool,
    viewport: bool,
    uv_view: bool,
    tool_preview: bool,
    selection: bool,
}

impl PresentDirty {
    pub(crate) fn request_material_textures(&mut self) {
        self.material_textures = true;
    }

    pub(crate) fn request_viewport(&mut self) {
        self.viewport = true;
    }

    pub(crate) fn request_uv_view(&mut self) {
        self.uv_view = true;
    }

    pub(crate) fn request_tool_preview(&mut self) {
        self.tool_preview = true;
    }

    pub(crate) fn request_selection(&mut self) {
        self.selection = true;
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.material_textures |= other.material_textures;
        self.viewport |= other.viewport;
        self.uv_view |= other.uv_view;
        self.tool_preview |= other.tool_preview;
        self.selection |= other.selection;
    }

    pub(crate) fn material_textures(self) -> bool {
        self.material_textures
    }

    pub(crate) fn viewport(self) -> bool {
        self.viewport
    }

    pub(crate) fn uv_view(self) -> bool {
        self.uv_view
    }

    pub(crate) fn tool_preview(self) -> bool {
        self.tool_preview
    }

    pub(crate) fn selection(self) -> bool {
        self.selection
    }
}
