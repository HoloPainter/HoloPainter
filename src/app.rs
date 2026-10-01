use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use eframe::egui;

use crate::{
    application::{
        AppState, ApplicationRuntime, Command, CopiedLayerContent, DecodedRgba8Image, EditorAction,
        EditorActionBlockReason, EditorActionContext, HoloPackImportPlan,
        LayerMaskClipboardPayload, PreparedLayerCutAction, SPATIAL_BLUR_MAX_ANGLE_DEGREES,
        SPATIAL_BLUR_MAX_RADIUS_PX, SPATIAL_BLUR_MIN_ANGLE_DEGREES, SPATIAL_BLUR_MIN_RADIUS_PX,
        SUPPORTED_RASTER_IMAGE_EXTENSIONS, SURFACE_BLUR_MAX_RADIUS_PX, SURFACE_BLUR_MIN_RADIUS_PX,
        StatusMessage, ToolCancelReason, ToolInputEvent, can_cut_layer_content, copy_layer_content,
        decode_raster_image, imported_layer_name, is_supported_raster_image_path,
        load_decoded_image_as_command, load_rgba8_as_paste_command_at, prepare_cut_layer_content,
    },
    cli::StartupRequest,
    color_sampler_runtime::ColorSamplerRuntime,
    core::{
        adjustment::AdjustmentKind,
        decal::{DecalImageAsset, DecalImageId},
        image::unpremultiply_rgba8,
        image_asset::{
            IMAGE_ASSET_BRUSH_TIP_TAG, IMAGE_ASSET_DECAL_TAG, ImageAssetCatalog, ImageMaskRecipe,
        },
        surface_filter::SpatialBlurOrientation,
        texture::TextureResourceDefinition,
        tool_catalog::load_tools_from_resources_with_engines,
    },
    editor_session::{
        EditorSessionFileV1, current_user_editor_session_path, load_editor_session_file,
    },
    export::psd::{
        PsdExportSnapshot, PsdExportWarning, serialize_psd, warnings_for_document,
        write_psd_files_transactionally,
    },
    import::SUPPORTED_MODEL_EXTENSIONS,
    licensing::{ProLicenseRuntime, ProLicenseState},
    localization::Localization,
    persistence::{RON_AUTOSAVE_INTERVAL, RonAutosave},
    project::{LoadedProject, open_project, save_project},
    render_host::RenderHost,
    renderer::{ColorSampleRequest, ViewRequest},
    screen_eyedropper_runtime::{ScreenEyedropperRuntime, ScreenEyedropperUpdate},
    settings::{
        AppTheme, BrushPresetStore, ProjectHistoryFileV1, SettingsFileV1,
        current_user_brush_preset_dir, current_user_image_asset_dir,
        current_user_project_history_path, current_user_settings_path,
        current_user_shortcut_profile_path, current_user_tool_layout_path, load_effective_settings,
        load_effective_tool_layout, load_project_history, save_project_history,
    },
    ui::{
        about_window::draw_about_window,
        adjustment_filter_dialog::{AdjustmentFilterDialogState, draw_adjustment_filter_dialog},
        asset_library_window::{
            AssetLibraryAction, AssetLibraryUiState, draw_asset_library_window,
        },
        export_image_dialog::{
            ExportImageDialogAction, ExportImageDraft, ExportImagePlan, OverwriteConfirmation,
            draw_export_image_dialog,
        },
        export_psd_dialog::{
            ExportPsdDialogAction, ExportPsdDraft, ExportPsdPlan, PsdOverwriteConfirmation,
            draw_export_psd_dialog,
        },
        holopack_import_dialog::{HoloPackImportDialogAction, draw_holopack_import_dialog},
        icons::UiIconRegistry,
        image_mask_conversion_dialog::{
            ImageMaskConversionAction, ImageMaskConversionDraft, draw_image_mask_conversion_dialog,
        },
        input::{
            native_window, shortcut_profile::ShortcutProfile, shortcuts::ShortcutInputRuntime,
            view_pointer::ViewPointerInputRouter,
        },
        mesh_reload_dialog::{MeshReloadDialogAction, MeshReloadDraft, draw_mesh_reload_dialog},
        new_project_dialog::{NewProjectDialogAction, NewProjectDraft, draw_new_project_dialog},
        panels::{
            menu_bar,
            runtime_metrics::{self, RuntimeMetricsUiData},
            status_bar,
        },
        pro_unlock_dialog::{ProUnlockAction, draw_pro_unlock_dialog},
        render_resources::UiRenderResources,
        settings_window,
        system_information_window::{SystemInformation, draw_system_information_window},
        view_output::{ScreenEyedropperTarget, UiRequest, ViewOutput, merge_view_requests},
        workspace::{
            StartupWorkspace, WorkspaceDrawInput, WorkspaceFileV1, WorkspaceState,
            WorkspaceWindowV1,
        },
    },
};

const NOTO_SANS_JP_FONT_NAME: &str = "Noto Sans JP";
const NOTO_SANS_JP_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansJP-Regular.ttf");

fn holopainter_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        NOTO_SANS_JP_FONT_NAME.to_owned(),
        egui::FontData::from_static(NOTO_SANS_JP_FONT_BYTES).into(),
    );
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("default proportional font family must exist")
        .insert(0, NOTO_SANS_JP_FONT_NAME.to_owned());
    fonts
        .families
        .get_mut(&egui::FontFamily::Monospace)
        .expect("default monospace font family must exist")
        .push(NOTO_SANS_JP_FONT_NAME.to_owned());
    fonts
}

const fn egui_theme(theme: AppTheme) -> egui::Theme {
    match theme {
        AppTheme::Light => egui::Theme::Light,
        AppTheme::Dark => egui::Theme::Dark,
    }
}

pub struct HoloPainterApp {
    runtime: ApplicationRuntime,
    localization: Localization,
    egui_ctx: egui::Context,
    project_name_hint: Option<String>,
    project_path: Option<PathBuf>,
    project_dirty: bool,
    project_history_path: Option<PathBuf>,
    project_history: ProjectHistoryFileV1,
    last_window_title: Option<String>,
    exit_approved: bool,
    settings_autosave: RonAutosave<SettingsFileV1>,
    workspace_autosave: RonAutosave<WorkspaceFileV1>,
    editor_session_autosave: RonAutosave<EditorSessionFileV1>,
    brush_preset_store: BrushPresetStore,
    next_brush_save_check: Instant,
    brush_save_error: Option<String>,
    image_assets: ImageAssetCatalog,
    tool_layout_autosave: RonAutosave<crate::core::tool_layout::ToolLayoutFileV1>,
    shortcut_autosave: RonAutosave<ShortcutProfile>,
    save_error: Option<String>,
    open_error: Option<String>,
    holopack_import_plan: Option<HoloPackImportPlan>,
    holopack_import_error: Option<String>,
    pending_project_transition: Option<PendingProjectTransition>,
    new_project_draft: Option<NewProjectDraft>,
    mesh_reload_draft: Option<MeshReloadDraft>,
    export_image_draft: Option<ExportImageDraft>,
    export_psd_draft: Option<ExportPsdDraft>,
    pro_license: ProLicenseRuntime,
    pro_unlock_dialog_open: bool,
    renderer: Option<RenderHost>,
    pointer_input: ViewPointerInputRouter,
    shortcut_input: ShortcutInputRuntime,
    runtime_metrics: RuntimeMetricsUiData,
    runtime_metrics_window_open: bool,
    asset_library_window_open: bool,
    asset_library_ui_state: AssetLibraryUiState,
    image_mask_conversion_draft: Option<ImageMaskConversionDraft>,
    settings_window_open: bool,
    settings_window_state: settings_window::SettingsWindowState,
    system_information: SystemInformation,
    system_information_window_open: bool,
    about_window_open: bool,
    adjustment_filter_dialog: AdjustmentFilterDialogState,
    blur_dialog_open: bool,
    blur_dialog: BlurDialogState,
    workspace: WorkspaceState,
    workspace_window: WorkspaceWindowV1,
    icons: UiIconRegistry,
    copied_image: Option<CopiedImageMetadata>,
    decal_thumbnail: Option<DecalThumbnailTexture>,
    next_decal_image_id: u64,
    image_decode_sender: Sender<ImageDecodeCompletion>,
    image_decode_receiver: Receiver<ImageDecodeCompletion>,
    pending_image_decodes: PendingImageDecodes,
    next_image_decode_request_id: u64,
    project_open_sender: Sender<ProjectOpenCompletion>,
    project_open_receiver: Receiver<ProjectOpenCompletion>,
    pending_project_open: Option<u64>,
    next_project_open_request_id: u64,
    color_sampler: ColorSamplerRuntime,
    screen_eyedropper: ScreenEyedropperRuntime,
    screen_eyedropper_target: ScreenEyedropperTarget,
}

#[derive(Clone)]
struct DecalThumbnailTexture {
    image_id: DecalImageId,
    texture: egui::TextureHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlurMethod {
    Surface,
    Spatial,
}

#[derive(Debug, Clone, Copy)]
struct BlurDialogState {
    method: BlurMethod,
    surface_radius_px: f32,
    spatial_radius_px: f32,
    cross_meshes: bool,
    orientation: SpatialBlurOrientation,
    maximum_normal_angle_degrees: f32,
}

impl Default for BlurDialogState {
    fn default() -> Self {
        Self {
            method: BlurMethod::Surface,
            surface_radius_px: 8.0,
            spatial_radius_px: 8.0,
            cross_meshes: true,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        }
    }
}

impl BlurDialogState {
    fn apply_command(self) -> Command {
        match self.method {
            BlurMethod::Surface => Command::ApplySurfaceBlur {
                radius_px: self.surface_radius_px,
            },
            BlurMethod::Spatial => Command::ApplySpatialBlur {
                radius_px: self.spatial_radius_px,
                cross_meshes: self.cross_meshes,
                orientation: self.orientation,
                maximum_normal_angle_degrees: self.maximum_normal_angle_degrees,
            },
        }
    }
}

#[derive(Debug, Clone)]
struct CopiedImageMetadata {
    clipboard_sequence: u32,
    layer_name: String,
    canvas_size: [u32; 2],
    origin: [u32; 2],
    image_size: [u32; 2],
    kind: CopiedImageKind,
}

#[derive(Debug, Clone)]
enum CopiedImageKind {
    Raster,
    LayerMask(LayerMaskClipboardPayload),
}

#[derive(Debug, Clone)]
enum ImageAssetImportIntent {
    Generic,
    Decal,
    TextureResource { required_tags: Vec<String> },
}

#[derive(Debug, Clone)]
enum ImageDecodeTarget {
    ImageAsset {
        source_path: PathBuf,
        intent: ImageAssetImportIntent,
    },
    ImportedLayer {
        layer_name: String,
    },
}

#[derive(Debug)]
struct ImageDecodeCompletion {
    request_id: u64,
    target: ImageDecodeTarget,
    result: Result<DecodedRgba8Image, String>,
}

#[derive(Debug)]
struct ProjectOpenCompletion {
    request_id: u64,
    path: PathBuf,
    result: Result<LoadedProject, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingProjectTransition {
    New,
    Open(PathBuf),
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsavedConfirmationDecision {
    None,
    Save,
    Discard,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectTransitionResolution {
    Wait,
    Perform,
    Cancel,
}

#[derive(Debug, Default)]
struct PendingImageDecodes {
    image_asset: Option<u64>,
    imported_layer: Option<u64>,
}

impl PendingImageDecodes {
    fn slot_mut(&mut self, target: &ImageDecodeTarget) -> &mut Option<u64> {
        match target {
            ImageDecodeTarget::ImageAsset { .. } => &mut self.image_asset,
            ImageDecodeTarget::ImportedLayer { .. } => &mut self.imported_layer,
        }
    }

    fn is_pending(&self, target: &ImageDecodeTarget) -> bool {
        match target {
            ImageDecodeTarget::ImageAsset { .. } => self.image_asset.is_some(),
            ImageDecodeTarget::ImportedLayer { .. } => self.imported_layer.is_some(),
        }
    }

    fn begin(&mut self, target: &ImageDecodeTarget, request_id: u64) -> bool {
        let slot = self.slot_mut(target);
        if slot.is_some() {
            return false;
        }
        *slot = Some(request_id);
        true
    }

    fn complete(&mut self, target: &ImageDecodeTarget, request_id: u64) -> bool {
        let slot = self.slot_mut(target);
        if *slot != Some(request_id) {
            return false;
        }
        *slot = None;
        true
    }
}

impl HoloPainterApp {
    fn editor_action_context(&self, keyboard_input_owned_by_ui: bool) -> EditorActionContext {
        EditorActionContext::new(
            &self.runtime.state,
            self.runtime.editor_action_block_reason(),
            keyboard_input_owned_by_ui,
        )
    }

    pub fn new(
        cc: &eframe::CreationContext<'_>,
        startup_request: StartupRequest,
        startup_workspace: StartupWorkspace,
    ) -> Self {
        cc.egui_ctx.set_fonts(holopainter_font_definitions());
        let settings_path = current_user_settings_path();
        let settings = load_effective_settings(settings_path.as_deref());
        let localization = Localization::new(settings.settings.appearance.language);
        cc.egui_ctx
            .set_theme(egui_theme(settings.settings.appearance.theme));
        let project_history_path = current_user_project_history_path();
        let project_history = load_project_history(project_history_path.as_deref());
        let brush_preset_dir = current_user_brush_preset_dir();
        let image_asset_dir = current_user_image_asset_dir();
        let image_assets = ImageAssetCatalog::load_effective(image_asset_dir.as_deref())
            .expect("loading image asset catalog");
        let tool_layout_path = current_user_tool_layout_path();
        let tool_layout = load_effective_tool_layout(tool_layout_path.as_deref());
        let default_tools = load_tools_from_resources_with_engines(
            brush_preset_dir.as_deref(),
            &image_assets,
            tool_layout,
            settings.settings.brush_engines.clone(),
        )
        .expect("loading brush tool catalog");
        let renderer = cc.wgpu_render_state.as_ref().and_then(|rs| {
            RenderHost::new(
                rs,
                [1024, 1024],
                &default_tools.brush_engines,
                &default_tools.brush_textures,
            )
            .ok()
        });

        let status = if renderer.is_some() {
            "GPU renderer initialized."
        } else {
            "GPU backend unavailable. Start with eframe WGPU renderer."
        };
        let mut state = AppState::with_status_and_configuration(
            status,
            settings.into_user_settings(),
            default_tools,
        );
        let editor_session_path = current_user_editor_session_path();
        if let Some(editor_session) = load_editor_session_file(editor_session_path.as_deref()) {
            editor_session.restore_into(&mut state);
        }
        let editor_session_baseline = EditorSessionFileV1::capture(&state)
            .expect("initial editor session must be capturable");
        let runtime = ApplicationRuntime::new(state);
        let shortcut_input = ShortcutInputRuntime::load().expect("loading shortcut profile");
        crate::native_input::configure_shortcuts(shortcut_input.profile());
        let (workspace_path, startup_workspace_file) = startup_workspace.into_parts();
        let workspace_window = startup_workspace_file.window.clone();
        let workspace = startup_workspace_file
            .build_state()
            .expect("startup workspace must have a valid layout");
        let autosave_now = Instant::now();
        let settings_baseline =
            SettingsFileV1::from_user_settings(runtime.state.user_settings().clone());
        let workspace_baseline = WorkspaceFileV1::capture(&workspace, &workspace_window)
            .expect("initial workspace must be capturable");
        let settings_autosave =
            RonAutosave::new("settings", settings_path, settings_baseline, autosave_now);
        let workspace_autosave = RonAutosave::new(
            "workspace",
            workspace_path,
            workspace_baseline,
            autosave_now,
        );
        let editor_session_autosave = RonAutosave::new(
            "editor session",
            editor_session_path,
            editor_session_baseline,
            autosave_now,
        );
        let brush_preset_store = BrushPresetStore::new(
            brush_preset_dir,
            runtime.state.user_brush_preset_definitions(),
        );
        let tool_layout_autosave = RonAutosave::new(
            "tool layout",
            tool_layout_path,
            runtime.state.tool_layout_snapshot(),
            autosave_now,
        );
        let shortcut_autosave = RonAutosave::new(
            "shortcut profile",
            current_user_shortcut_profile_path(),
            shortcut_input.profile().clone(),
            autosave_now,
        );
        let (image_decode_sender, image_decode_receiver) = mpsc::channel();
        let (project_open_sender, project_open_receiver) = mpsc::channel();

        let mut app = Self {
            runtime,
            localization,
            egui_ctx: cc.egui_ctx.clone(),
            project_name_hint: None,
            project_path: None,
            project_dirty: false,
            project_history_path,
            project_history,
            last_window_title: None,
            exit_approved: false,
            settings_autosave,
            workspace_autosave,
            editor_session_autosave,
            brush_preset_store,
            next_brush_save_check: autosave_now + RON_AUTOSAVE_INTERVAL,
            brush_save_error: None,
            image_assets,
            tool_layout_autosave,
            shortcut_autosave,
            save_error: None,
            open_error: None,
            holopack_import_plan: None,
            holopack_import_error: None,
            pending_project_transition: None,
            new_project_draft: None,
            mesh_reload_draft: None,
            export_image_draft: None,
            export_psd_draft: None,
            pro_license: ProLicenseRuntime::default(),
            pro_unlock_dialog_open: false,
            renderer,
            pointer_input: ViewPointerInputRouter::default(),
            shortcut_input,
            runtime_metrics: RuntimeMetricsUiData::default(),
            runtime_metrics_window_open: false,
            asset_library_window_open: false,
            asset_library_ui_state: AssetLibraryUiState::default(),
            image_mask_conversion_draft: None,
            settings_window_open: false,
            settings_window_state: settings_window::SettingsWindowState::default(),
            system_information: SystemInformation::from_creation_context(cc),
            system_information_window_open: false,
            about_window_open: false,
            adjustment_filter_dialog: AdjustmentFilterDialogState::default(),
            blur_dialog_open: false,
            blur_dialog: BlurDialogState::default(),
            workspace,
            workspace_window,
            icons: UiIconRegistry::new(&cc.egui_ctx).expect("loading builtin UI icons"),
            copied_image: None,
            decal_thumbnail: None,
            next_decal_image_id: 1,
            image_decode_sender,
            image_decode_receiver,
            pending_image_decodes: PendingImageDecodes::default(),
            next_image_decode_request_id: 1,
            project_open_sender,
            project_open_receiver,
            pending_project_open: None,
            next_project_open_request_id: 1,
            color_sampler: ColorSamplerRuntime::default(),
            screen_eyedropper: ScreenEyedropperRuntime::default(),
            screen_eyedropper_target: ScreenEyedropperTarget::CurrentColor,
        };
        app.apply_startup_request(startup_request);
        app
    }

    fn apply_startup_request(&mut self, request: StartupRequest) {
        match request {
            StartupRequest::None => {
                let result = self
                    .renderer
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("GPU renderer is unavailable"))
                    .and_then(|renderer| {
                        crate::application::create_startup_project(
                            renderer.max_texture_dimension_2d(),
                        )
                    })
                    .and_then(|project| {
                        self.runtime
                            .dispatch(Command::ProjectLoaded(project))
                            .map_err(anyhow::Error::msg)
                    });
                match result {
                    Ok(_) => {
                        self.project_name_hint = Some("Cube".into());
                        self.project_dirty = false;
                        self.workspace.restore_collapsed_layer_groups([]);
                    }
                    Err(error) => self.open_error = Some(format!("{error:#}")),
                }
            }
            StartupRequest::OpenProject(path) => self.start_open_project(path),
            StartupRequest::NewProjectFromModel(path) => {
                let mut draft = NewProjectDraft::default();
                draft.load_source(path);
                self.new_project_draft = Some(draft);
            }
        }
    }

    fn dispatch(&mut self, command: Command) {
        let _ = self.dispatch_checked(command);
    }

    fn dispatch_checked(&mut self, command: Command) -> Result<(), String> {
        let changed_theme = match &command {
            Command::SetTheme(theme) => Some(*theme),
            _ => None,
        };
        let changed_language = match &command {
            Command::SetLanguage(language) => Some(*language),
            _ => None,
        };
        let project_changed = self.runtime.dispatch(command)?;
        if let Some(theme) = changed_theme {
            self.egui_ctx.set_theme(egui_theme(theme));
        }
        if let Some(language) = changed_language {
            self.localization.set_preference(language);
            self.egui_ctx.request_repaint();
        }
        if project_changed {
            self.project_dirty = true;
        }
        Ok(())
    }

    fn project_display_filename(&self) -> Option<String> {
        self.runtime.state.document().map(|_| {
            default_project_filename(
                self.project_path.as_deref(),
                self.project_name_hint.as_deref(),
            )
        })
    }

    fn desired_window_title(&self) -> String {
        window_title(
            self.project_display_filename().as_deref(),
            self.project_dirty,
        )
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = self.desired_window_title();
        if self.last_window_title.as_deref() == Some(title.as_str()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
        self.last_window_title = Some(title);
    }

    fn handle_native_close_request(&mut self, ctx: &egui::Context) -> bool {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return false;
        }
        let Some(transition) = close_request_transition(self.project_dirty, self.exit_approved)
        else {
            return false;
        };

        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.screen_eyedropper.cancel();
        self.pending_project_transition = Some(transition);
        true
    }

    fn persist_tool_layout(&mut self) -> Result<(), String> {
        let presets = self.runtime.state.user_brush_preset_definitions();
        let layout = self.runtime.state.tool_layout_snapshot();
        self.brush_preset_store.sync_with_layout(presets, || {
            self.tool_layout_autosave
                .save_if_changed(layout, crate::core::tool_layout::ToolLayoutFileV1::validate)
        })
    }

    fn persist_editor_session(&mut self) -> Result<(), String> {
        let snapshot = EditorSessionFileV1::capture(&self.runtime.state)?;
        self.editor_session_autosave
            .save_now(snapshot, EditorSessionFileV1::validate)
    }

    fn persist_workspace(&mut self) -> Result<(), String> {
        let snapshot = WorkspaceFileV1::capture(&self.workspace, &self.workspace_window)?;
        self.workspace_autosave
            .save_now(snapshot, WorkspaceFileV1::validate)
    }

    fn apply_view_output(
        &mut self,
        output: ViewOutput,
        frame_view_requests: &mut Vec<ViewRequest>,
        frame_color_sample_request: &mut Option<ColorSampleRequest>,
        frame_actions: &mut Vec<EditorAction>,
    ) -> bool {
        let ViewOutput {
            commands,
            actions,
            view_requests,
            ui_requests,
            mut needs_repaint,
        } = output;
        for command in commands {
            if matches!(
                &command,
                Command::ViewPointer(crate::application::ViewPointerEvent::Viewport3d {
                    meta: crate::application::ViewPointerMeta {
                        phase: crate::application::ViewPointerPhase::Down,
                        ..
                    },
                    ..
                }) | Command::ViewPointer(crate::application::ViewPointerEvent::Uv {
                    meta: crate::application::ViewPointerMeta {
                        phase: crate::application::ViewPointerPhase::Down,
                        ..
                    },
                    ..
                }) | Command::ToolInput(ToolInputEvent::PointerDown(_))
            ) {
                self.shortcut_input.mark_pointer_down();
            }
            self.dispatch(command);
        }
        for request in ui_requests {
            match request {
                UiRequest::SelectDecalAsset { asset_id } => self.select_decal_asset(&asset_id),
                UiRequest::ImportDecalImage => {
                    self.import_image_asset(ImageAssetImportIntent::Decal);
                }
                UiRequest::ImportTextureResource { required_tags } => {
                    self.import_image_asset(ImageAssetImportIntent::TextureResource {
                        required_tags,
                    });
                }
                UiRequest::OpenImageAssetLibrary => {
                    self.asset_library_window_open = true;
                }
                UiRequest::SampleColor(request) => {
                    let time_seconds = self.egui_ctx.input(|input| input.time);
                    if let Some(sample) = self.color_sampler.prepare_request(
                        request,
                        self.runtime.state.document_generation(),
                        time_seconds,
                    ) {
                        if let Some(previous) = frame_color_sample_request.replace(sample) {
                            self.color_sampler.cancel_request(previous.id);
                        }
                    }
                }
                UiRequest::StartScreenEyedropper(target) => {
                    self.screen_eyedropper_target = target;
                    let pointer_was_active = self.pointer_input.cancel_active_input();
                    if pointer_was_active || self.runtime.state.is_tool_interacting() {
                        self.dispatch(Command::ToolInput(ToolInputEvent::Cancel {
                            reason: ToolCancelReason::PointerCaptureLost,
                        }));
                    }
                    for command in self.shortcut_input.cancel_active_inputs() {
                        self.dispatch(command);
                    }
                    self.screen_eyedropper.request_start();
                }
                UiRequest::OpenAdjustmentEditor { layer_id } => {
                    self.workspace
                        .open_adjustment_editor(layer_id, self.runtime.state.document_generation());
                }
            }
            needs_repaint = true;
        }
        frame_actions.extend(actions);
        merge_view_requests(frame_view_requests, view_requests);
        needs_repaint
    }

    fn render_resources(&self) -> UiRenderResources {
        UiRenderResources {
            renderer_available: self.renderer.is_some(),
            viewport_texture_id: self
                .renderer
                .as_ref()
                .map(|renderer| renderer.viewport_texture_id()),
            uv_view_texture_id: self
                .renderer
                .as_ref()
                .map(|renderer| renderer.uv_view_texture_id()),
            tool_preview_texture_id: self
                .renderer
                .as_ref()
                .map(|renderer| renderer.tool_preview_texture_id()),
            decal_thumbnail_texture_id: self
                .decal_thumbnail
                .as_ref()
                .map(|thumbnail| thumbnail.texture.id()),
            color_sample_preview: self.color_sampler.preview(),
        }
    }

    fn focused_texture_size(&self) -> Option<[usize; 2]> {
        self.renderer
            .as_ref()
            .map(|renderer| renderer.texture_size(self.focused_material_index()))
    }

    fn refresh_texture_metrics(&mut self) {
        let texture_metrics = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.texture_metrics())
            .unwrap_or_default();
        self.runtime_metrics
            .observe_texture_metrics(texture_metrics);
    }

    fn execute_frame(
        &mut self,
        requests: Vec<ViewRequest>,
        color_sample_request: Option<ColorSampleRequest>,
    ) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            self.color_sampler.cancel_pending();
            self.runtime.discard_renderer_frame_plan();
            let _ = self.runtime.resolve_pending_edits_without_renderer();
            return false;
        };

        let mut submission = self.runtime.drain_renderer_submission_work(requests);
        submission.renderer_plan.color_sample_request = color_sample_request;
        let frame_result = renderer.execute_frame(submission.renderer_plan);
        let render_metrics = frame_result.report.metrics.clone();
        self.runtime_metrics.observe_render_metrics(render_metrics);
        self.refresh_texture_metrics();
        if let Err(error) = self.runtime.accept_renderer_submission(
            submission.submission,
            frame_result.report,
            frame_result.started_commit,
        ) {
            self.color_sampler.cancel_pending();
            let _ = self.runtime.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-render-submission-failed").arg("error", error),
            ));
            return false;
        }
        self.runtime.has_pending_render_finalizations()
    }

    fn poll_completed_color_samples(&mut self) -> bool {
        let completed = self
            .renderer
            .as_mut()
            .map(RenderHost::poll_completed_color_samples)
            .unwrap_or_default();
        let generation = self.runtime.state.document_generation();
        let source = self.runtime.state.color_picker_options().source;
        let active_layer_id = self.runtime.state.active_layer_id();
        let mut changed = false;
        for completed in completed {
            let outcome =
                self.color_sampler
                    .complete(completed, generation, source, active_layer_id);
            changed |= outcome.preview_changed;
            if let Some(color) = outcome.current_color {
                self.dispatch(Command::SetCurrentColor(color));
                changed = true;
            }
            if let Some(error) = outcome.error {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-color-sampling-failed").arg("error", error),
                ));
                changed = true;
            }
        }
        changed || self.color_sampler.has_pending()
    }

    fn poll_completed_render_commits(&mut self) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };

        let mut changed = false;
        for completed in renderer.poll_completed_commits() {
            let commit_id = completed.id;
            match self.runtime.finalize_completed_render_commit(completed) {
                Ok((artifacts, project_changed)) => {
                    self.project_dirty |= project_changed;
                    if let Err(error) = renderer.accept_commit(commit_id, &artifacts) {
                        let _ = self.runtime.dispatch(Command::SetStatusMessage(
                            StatusMessage::localized("status-render-shadow-update-failed")
                                .arg("error", error),
                        ));
                        return changed;
                    }
                    changed = true;
                }
                Err(error) => {
                    let _ = self.runtime.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-render-finalization-failed")
                            .arg("error", error),
                    ));
                    return changed;
                }
            }
        }
        let metrics = renderer.take_metrics();
        self.runtime_metrics.observe_render_metrics(metrics);
        self.refresh_texture_metrics();
        changed
    }

    fn focused_material_index(&self) -> usize {
        self.runtime.state.focused_material_index()
    }

    fn start_image_decode(&mut self, path: PathBuf, target: ImageDecodeTarget) -> bool {
        let Some(max_texture_dimension_2d) = self
            .renderer
            .as_ref()
            .map(RenderHost::max_texture_dimension_2d)
        else {
            let key = match &target {
                ImageDecodeTarget::ImageAsset { .. } => "status-image-asset-import-gpu-unavailable",
                ImageDecodeTarget::ImportedLayer { .. } => "status-image-import-gpu-unavailable",
            };
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(key)));
            return false;
        };

        if self.pending_image_decodes.is_pending(&target) {
            let key = match &target {
                ImageDecodeTarget::ImageAsset { .. } => "status-image-asset-decode-pending",
                ImageDecodeTarget::ImportedLayer { .. } => "status-image-import-decode-pending",
            };
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(key)));
            return false;
        }

        let request_id = self.next_image_decode_request_id;
        self.next_image_decode_request_id =
            self.next_image_decode_request_id.wrapping_add(1).max(1);
        let began = self.pending_image_decodes.begin(&target, request_id);
        debug_assert!(began);

        let display_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Image")
            .to_owned();
        let key = match &target {
            ImageDecodeTarget::ImageAsset { intent, .. } => match intent {
                ImageAssetImportIntent::Generic => "status-importing-image-asset",
                ImageAssetImportIntent::Decal => "status-importing-decal-image",
                ImageAssetImportIntent::TextureResource { .. } => {
                    "status-importing-texture-resource-image"
                }
            },
            ImageDecodeTarget::ImportedLayer { .. } => "status-decoding-import-image",
        };
        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized(key).arg("name", display_name),
        ));

        let sender = self.image_decode_sender.clone();
        let repaint_ctx = self.egui_ctx.clone();
        let completion_target = target.clone();
        let spawn_result = thread::Builder::new()
            .name("holo-painter-image-decode".to_owned())
            .spawn(move || {
                let result = decode_raster_image(&path, max_texture_dimension_2d)
                    .map_err(|error| format!("{error:#}"));
                if sender
                    .send(ImageDecodeCompletion {
                        request_id,
                        target: completion_target,
                        result,
                    })
                    .is_ok()
                {
                    repaint_ctx.request_repaint();
                }
            });

        if let Err(error) = spawn_result {
            self.pending_image_decodes.complete(&target, request_id);
            let key = match &target {
                ImageDecodeTarget::ImageAsset { .. } => "status-image-asset-decoder-start-failed",
                ImageDecodeTarget::ImportedLayer { .. } => "status-image-decoder-start-failed",
            };
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized(key).arg("error", error),
            ));
            return false;
        }

        true
    }

    fn poll_completed_image_decodes(&mut self) -> bool {
        let mut changed = false;
        while let Ok(completion) = self.image_decode_receiver.try_recv() {
            if !self
                .pending_image_decodes
                .complete(&completion.target, completion.request_id)
            {
                continue;
            }
            changed = true;

            match (completion.target, completion.result) {
                (
                    ImageDecodeTarget::ImageAsset {
                        source_path,
                        intent,
                    },
                    Ok(image),
                ) => {
                    let display_name = image_asset_display_name(&self.localization, &source_path);
                    let representation_tags = match &intent {
                        ImageAssetImportIntent::Generic => Vec::new(),
                        ImageAssetImportIntent::Decal => {
                            vec![IMAGE_ASSET_DECAL_TAG.to_owned()]
                        }
                        ImageAssetImportIntent::TextureResource { .. } => Vec::new(),
                    };
                    match self.image_assets.import_image(
                        &display_name,
                        image.size,
                        image.rgba8,
                        representation_tags,
                    ) {
                        Ok(asset_id) => {
                            match intent {
                                ImageAssetImportIntent::Generic => {}
                                ImageAssetImportIntent::Decal => {
                                    self.select_decal_asset(&asset_id);
                                }
                                ImageAssetImportIntent::TextureResource { required_tags } => {
                                    self.begin_image_mask_conversion(
                                        asset_id.clone(),
                                        required_tags,
                                    );
                                }
                            }
                            self.dispatch(Command::SetStatusMessage(
                                StatusMessage::localized("status-image-asset-imported")
                                    .arg("name", display_name),
                            ));
                        }
                        Err(error) => {
                            self.dispatch(Command::SetStatusMessage(
                                StatusMessage::localized("status-image-asset-import-failed")
                                    .arg("error", format!("{error:#}")),
                            ));
                        }
                    }
                }
                (ImageDecodeTarget::ImageAsset { .. }, Err(error)) => {
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-image-asset-import-failed")
                            .arg("error", error),
                    ));
                }
                (ImageDecodeTarget::ImportedLayer { layer_name }, Ok(image)) => {
                    let command = load_decoded_image_as_command(image, layer_name, None);
                    if let Err(error) = self.dispatch_checked(command) {
                        self.dispatch(Command::SetStatusMessage(
                            StatusMessage::localized("status-image-import-failed")
                                .arg("error", error),
                        ));
                    }
                }
                (ImageDecodeTarget::ImportedLayer { .. }, Err(error)) => {
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-image-import-failed").arg("error", error),
                    ));
                }
            }
        }
        changed
    }

    fn import_image_asset(&mut self, intent: ImageAssetImportIntent) {
        let pending_target = ImageDecodeTarget::ImageAsset {
            source_path: PathBuf::new(),
            intent: intent.clone(),
        };
        if self.pending_image_decodes.is_pending(&pending_target) {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-image-asset-decode-pending",
            )));
            return;
        }
        let dialog = rfd::FileDialog::new().add_filter(
            self.localization.text("file-filter-image"),
            SUPPORTED_RASTER_IMAGE_EXTENSIONS,
        );
        let Some(path) = dialog.pick_file() else {
            return;
        };
        let target = ImageDecodeTarget::ImageAsset {
            source_path: path.clone(),
            intent,
        };
        self.start_image_decode(path, target);
    }

    fn request_holopack_import(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-holopack-requires-gpu",
            )));
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                self.localization.text("file-filter-holopainter-package"),
                &["holopack"],
            )
            .pick_file()
        else {
            return;
        };
        let result = std::fs::read(&path)
            .map_err(|error| anyhow::anyhow!("reading {}: {error}", path.display()))
            .and_then(|bytes| {
                HoloPackImportPlan::build(
                    &bytes,
                    self.runtime.state.brush_engines(),
                    self.runtime.state.brush_presets(),
                    self.runtime.state.brush_textures(),
                    renderer.max_texture_dimension_2d(),
                )
            });
        match result {
            Ok(plan) => {
                self.holopack_import_error = None;
                self.holopack_import_plan = Some(plan);
            }
            Err(error) => {
                self.holopack_import_plan = None;
                self.holopack_import_error = Some(format!("{error:#}"));
            }
        }
    }

    fn commit_holopack_import(&mut self) {
        let Some(plan) = self.holopack_import_plan.take() else {
            return;
        };
        let state_snapshot = self.runtime.state.user_brush_resource_snapshot();
        let old_settings =
            SettingsFileV1::from_user_settings(self.runtime.state.user_settings().clone());
        let old_presets = self.runtime.state.user_brush_preset_definitions();
        let old_layout = self.runtime.state.tool_layout_snapshot();
        let previous_engines = plan
            .brush_engines
            .iter()
            .map(|engine| {
                (
                    engine.id.clone(),
                    self.runtime.state.brush_engines().get(&engine.id).cloned(),
                )
            })
            .collect::<Vec<_>>();

        let mut registered_engine_ids = Vec::new();
        if let Some(renderer) = self.renderer.as_mut() {
            for engine in &plan.brush_engines {
                if let Err(error) = renderer.register_brush_engine(engine) {
                    rollback_renderer_engines(renderer, &previous_engines, &registered_engine_ids);
                    let mut args = fluent::FluentArgs::new();
                    args.set("id", engine.id.as_str());
                    args.set("error", format!("{error:#}"));
                    self.holopack_import_error = Some(
                        self.localization
                            .format("dialog-holopack-engine-compile-error", Some(&args)),
                    );
                    return;
                }
                registered_engine_ids.push(engine.id.clone());
            }
        }

        if let Err(error) = self
            .runtime
            .state
            .import_holopack_brush_resources(plan.brush_engines.clone(), plan.brush_presets.clone())
        {
            if let Some(renderer) = self.renderer.as_mut() {
                rollback_renderer_engines(renderer, &previous_engines, &registered_engine_ids);
            }
            self.holopack_import_error = Some(format!("{error:#}"));
            return;
        }

        let mut imported_asset_ids = Vec::new();
        let import_result = (|| -> Result<(), String> {
            for texture in &plan.textures {
                let display_name = texture
                    .image
                    .file_name
                    .strip_suffix(".png")
                    .unwrap_or(texture.image.file_name.as_str());
                let asset_id = self
                    .image_assets
                    .import_png_bytes(
                        display_name,
                        texture.image.size,
                        texture.image.rgba8.clone(),
                        Vec::new(),
                        &texture.encoded_png,
                    )
                    .map_err(|error| format!("importing Texture {:?}: {error:#}", texture.path))?;
                imported_asset_ids.push(asset_id);
            }

            self.settings_autosave.save_now(
                SettingsFileV1::from_user_settings(self.runtime.state.user_settings().clone()),
                SettingsFileV1::validate,
            )?;
            self.persist_tool_layout()?;
            Ok(())
        })();

        if let Err(error) = import_result {
            for asset_id in imported_asset_ids.iter().rev() {
                let _ = self.image_assets.remove_user(asset_id);
            }
            self.runtime
                .state
                .restore_user_brush_resource_snapshot(state_snapshot);
            if let Some(renderer) = self.renderer.as_mut() {
                rollback_renderer_engines(renderer, &previous_engines, &registered_engine_ids);
            }
            let rollback_result = self
                .settings_autosave
                .save_now(old_settings, SettingsFileV1::validate)
                .and_then(|_| {
                    self.brush_preset_store.sync_with_layout(old_presets, || {
                        self.tool_layout_autosave.save_if_changed(
                            old_layout,
                            crate::core::tool_layout::ToolLayoutFileV1::validate,
                        )
                    })
                });
            self.holopack_import_error = Some(match rollback_result {
                Ok(()) => error,
                Err(rollback_error) => {
                    let mut args = fluent::FluentArgs::new();
                    args.set("error", error);
                    args.set("rollback_error", rollback_error);
                    self.localization
                        .format("dialog-holopack-rollback-error", Some(&args))
                }
            });
            return;
        }

        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-holopack-imported").arg("name", plan.manifest.name),
        ));
    }

    fn draw_holopack_dialogs(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        if let Some(plan) = self.holopack_import_plan.as_ref()
            && let Some(action) = draw_holopack_import_dialog(ctx, &self.localization, plan)
        {
            changed = true;
            match action {
                HoloPackImportDialogAction::Cancel => self.holopack_import_plan = None,
                HoloPackImportDialogAction::Import => self.commit_holopack_import(),
            }
        }
        if let Some(error) = self.holopack_import_error.clone() {
            egui::Modal::new(egui::Id::new("holopack_import_error")).show(ctx, |ui| {
                ui.heading(self.localization.text("dialog-holopack-import-error-title"));
                ui.label(error);
                if ui.button(self.localization.text("action-ok")).clicked() {
                    self.holopack_import_error = None;
                    changed = true;
                }
            });
        }
        changed
    }

    fn delete_user_brush_engine(&mut self, id: &str) {
        let users = self.runtime.state.brush_engine_users(id);
        if !users.is_empty() {
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-brush-engine-delete-in-use")
                    .arg("users", users.join(", ")),
            ));
            return;
        }
        let Some(definition) = self.runtime.state.brush_engines().get(id).cloned() else {
            return;
        };
        let snapshot = self.runtime.state.user_brush_resource_snapshot();
        let old_settings =
            SettingsFileV1::from_user_settings(self.runtime.state.user_settings().clone());
        if let Err(error) = self.runtime.state.delete_user_brush_engine(id) {
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-brush-engine-delete-failed")
                    .arg("error", format!("{error:#}")),
            ));
            return;
        }
        let save_result = self.settings_autosave.save_now(
            SettingsFileV1::from_user_settings(self.runtime.state.user_settings().clone()),
            SettingsFileV1::validate,
        );
        if let Err(error) = save_result {
            self.runtime
                .state
                .restore_user_brush_resource_snapshot(snapshot);
            let _ = self
                .settings_autosave
                .save_now(old_settings, SettingsFileV1::validate);
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-brush-engine-delete-failed").arg("error", error),
            ));
            return;
        }
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.unregister_brush_engine(id);
        }
        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-brush-engine-deleted")
                .arg("name", definition.display_name),
        ));
    }

    fn select_decal_asset(&mut self, asset_id: &str) {
        let Some(asset) = self.image_assets.get(asset_id) else {
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-decal-image-missing").arg("id", asset_id),
            ));
            return;
        };
        let Some(representation) = asset.rgba8_representation().filter(|representation| {
            representation
                .tags
                .iter()
                .any(|tag| tag == IMAGE_ASSET_DECAL_TAG)
        }) else {
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-decal-image-not-enabled")
                    .arg("name", &asset.display_name),
            ));
            return;
        };
        let image_id = DecalImageId(self.next_decal_image_id);
        self.next_decal_image_id = self.next_decal_image_id.wrapping_add(1).max(1);
        let display_name = asset.display_name.clone();
        let decal = DecalImageAsset {
            id: image_id,
            file_name: display_name.clone(),
            size: representation.size,
            rgba8: representation.bytes.clone(),
        };
        self.dispatch(Command::SetDecalImage(std::sync::Arc::new(decal)));
        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-decal-image-selected").arg("name", display_name),
        ));
    }

    fn selected_decal_asset_id(&self) -> Option<&str> {
        let decal = self.runtime.state.decal_image()?;
        self.image_assets.assets().find_map(|asset| {
            let representation = asset.rgba8_representation()?;
            (representation
                .tags
                .iter()
                .any(|tag| tag == IMAGE_ASSET_DECAL_TAG)
                && std::sync::Arc::ptr_eq(&representation.bytes, &decal.rgba8))
            .then_some(asset.id.as_str())
        })
    }

    fn begin_image_mask_conversion(&mut self, asset_id: String, required_tags: Vec<String>) {
        let Some(asset) = self.image_assets.get(&asset_id) else {
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-texture-conversion-missing-asset")
                    .arg("id", asset_id),
            ));
            return;
        };
        if asset.rgba8_representation().is_none() {
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-texture-conversion-no-rgba")
                    .arg("name", &asset.display_name),
            ));
            return;
        }
        self.image_mask_conversion_draft =
            Some(ImageMaskConversionDraft::new(asset_id, required_tags));
    }

    fn create_image_mask_representation(
        &mut self,
        asset_id: String,
        required_tags: Vec<String>,
        recipe: ImageMaskRecipe,
    ) {
        let representation_id = match self.image_assets.add_r8_representation_from_rgba(
            &asset_id,
            required_tags,
            recipe,
        ) {
            Ok(id) => id,
            Err(error) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-texture-conversion-failed")
                        .arg("error", format!("{error:#}")),
                ));
                return;
            }
        };
        let definition = {
            let source_asset_origin = self
                .image_assets
                .origin(&asset_id)
                .expect("new image representation must retain its asset origin");
            let asset = self
                .image_assets
                .get(&asset_id)
                .expect("new image representation must belong to an existing asset");
            let representation = asset
                .representation(&representation_id)
                .expect("new image representation must be visible in the asset catalog");
            match TextureResourceDefinition::from_image_representation(
                asset,
                source_asset_origin,
                representation,
            ) {
                Ok(definition) => definition,
                Err(error) => {
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-texture-registration-failed")
                            .arg("error", format!("{error:#}")),
                    ));
                    return;
                }
            }
        };

        let renderer_registered = if let Some(renderer) = self.renderer.as_mut() {
            if let Err(error) = renderer.register_brush_texture(&definition) {
                let rollback_error = self
                    .image_assets
                    .remove_user_r8_representation(&asset_id, &representation_id)
                    .err();
                let mut status = StatusMessage::localized(if rollback_error.is_some() {
                    "status-texture-gpu-registration-rollback-failed"
                } else {
                    "status-texture-gpu-registration-failed"
                })
                .arg("error", format!("{error:#}"));
                if let Some(rollback_error) = rollback_error {
                    status = status.arg("rollback_error", format!("{rollback_error:#}"));
                }
                self.dispatch(Command::SetStatusMessage(status));
                return;
            }
            true
        } else {
            false
        };
        if let Err(error) = self.runtime.state.insert_brush_texture(definition.clone()) {
            if renderer_registered && let Some(renderer) = self.renderer.as_mut() {
                renderer.unregister_brush_texture(&definition.id);
            }
            let rollback_error = self
                .image_assets
                .remove_user_r8_representation(&asset_id, &representation_id)
                .err();
            let mut status = StatusMessage::localized(if rollback_error.is_some() {
                "status-texture-registration-rollback-failed"
            } else {
                "status-texture-registration-failed"
            })
            .arg("error", format!("{error:#}"));
            if let Some(rollback_error) = rollback_error {
                status = status.arg("rollback_error", format!("{rollback_error:#}"));
            }
            self.dispatch(Command::SetStatusMessage(status));
            return;
        }
        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-texture-resource-created")
                .arg("name", definition.display_name),
        ));
    }

    fn handle_asset_library_action(&mut self, action: AssetLibraryAction) {
        match action {
            AssetLibraryAction::ImportImage => {
                self.import_image_asset(ImageAssetImportIntent::Generic);
            }
            AssetLibraryAction::Rename {
                asset_id,
                display_name,
            } => match self.image_assets.rename_user(&asset_id, &display_name) {
                Ok(()) => {
                    if let Some(asset) = self.image_assets.get(&asset_id) {
                        let texture_ids = asset
                            .representations
                            .iter()
                            .filter(|representation| representation.r8().is_some())
                            .map(|representation| representation.id.clone())
                            .collect::<Vec<_>>();
                        for texture_id in texture_ids {
                            self.runtime.state.set_brush_texture_display_name(
                                &texture_id,
                                asset.display_name.as_str(),
                            );
                        }
                    }
                    self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                        "status-asset-renamed",
                    )));
                }
                Err(error) => self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-asset-rename-failed")
                        .arg("error", format!("{error:#}")),
                )),
            },
            AssetLibraryAction::SetTags { asset_id, tags } => {
                match self.image_assets.set_user_tags(&asset_id, tags) {
                    Ok(()) => self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                        "status-asset-tags-updated",
                    ))),
                    Err(error) => self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-asset-tag-update-failed")
                            .arg("error", format!("{error:#}")),
                    )),
                }
            }
            AssetLibraryAction::SetDecalEnabled { asset_id, enabled } => {
                match self
                    .image_assets
                    .set_user_rgba_tag(&asset_id, IMAGE_ASSET_DECAL_TAG, enabled)
                {
                    Ok(()) => {
                        if !enabled && self.selected_decal_asset_id() == Some(asset_id.as_str()) {
                            self.dispatch(Command::ClearDecalImage);
                        }
                        self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                            if enabled {
                                "status-asset-decal-enabled"
                            } else {
                                "status-asset-decal-disabled"
                            },
                        )));
                    }
                    Err(error) => self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-asset-capability-update-failed")
                            .arg("error", format!("{error:#}")),
                    )),
                }
            }
            AssetLibraryAction::SetBrushTipEnabled { asset_id, enabled } => {
                if enabled {
                    self.begin_image_mask_conversion(
                        asset_id,
                        vec![IMAGE_ASSET_BRUSH_TIP_TAG.to_owned()],
                    );
                    return;
                }
                let Some(representation_id) = self
                    .image_assets
                    .get(&asset_id)
                    .and_then(|asset| asset.representation_with_tag(IMAGE_ASSET_BRUSH_TIP_TAG))
                    .map(|representation| representation.id.clone())
                else {
                    return;
                };
                let users = self.runtime.state.brush_texture_users(&representation_id);
                if !users.is_empty() {
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-brush-tip-disable-in-use")
                            .arg("users", users.join(", ")),
                    ));
                    return;
                }
                match self
                    .image_assets
                    .remove_user_r8_representation(&asset_id, &representation_id)
                {
                    Ok(_) => {
                        self.runtime.state.remove_brush_texture(&representation_id);
                        if let Some(renderer) = self.renderer.as_mut() {
                            renderer.unregister_brush_texture(&representation_id);
                        }
                        self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                            "status-asset-brush-tip-disabled",
                        )));
                    }
                    Err(error) => self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-brush-tip-update-failed")
                            .arg("error", format!("{error:#}")),
                    )),
                }
            }
            AssetLibraryAction::Delete { asset_id } => {
                let texture_ids = self
                    .image_assets
                    .get(&asset_id)
                    .map(|asset| {
                        asset
                            .representations
                            .iter()
                            .filter(|representation| representation.r8().is_some())
                            .map(|representation| representation.id.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut users = texture_ids
                    .iter()
                    .flat_map(|texture_id| self.runtime.state.brush_texture_users(texture_id))
                    .collect::<Vec<_>>();
                users.sort();
                users.dedup();
                if !users.is_empty() {
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-asset-delete-in-use")
                            .arg("users", users.join(", ")),
                    ));
                    return;
                }

                let selected = self.selected_decal_asset_id() == Some(asset_id.as_str());
                match self.image_assets.remove_user(&asset_id) {
                    Ok(()) => {
                        for texture_id in texture_ids {
                            self.runtime.state.remove_brush_texture(&texture_id);
                            if let Some(renderer) = self.renderer.as_mut() {
                                renderer.unregister_brush_texture(&texture_id);
                            }
                        }
                        if selected {
                            self.dispatch(Command::ClearDecalImage);
                        }
                        self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                            "status-asset-deleted",
                        )));
                    }
                    Err(error) => self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-asset-delete-failed")
                            .arg("error", format!("{error:#}")),
                    )),
                }
            }
        }
    }

    fn sync_decal_thumbnail(&mut self, ctx: &egui::Context) {
        let Some(image) = self.runtime.state.decal_image() else {
            self.decal_thumbnail = None;
            return;
        };
        if self
            .decal_thumbnail
            .as_ref()
            .is_some_and(|thumbnail| thumbnail.image_id == image.id)
        {
            return;
        }

        let color_image = egui::ColorImage::from_rgba_premultiplied(
            [image.size[0] as usize, image.size[1] as usize],
            image.rgba8.as_ref(),
        );
        let texture = ctx.load_texture(
            format!("decal.thumbnail.{}", image.id.0),
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.decal_thumbnail = Some(DecalThumbnailTexture {
            image_id: image.id,
            texture,
        });
    }

    fn import_image_path_as_layer(&mut self, path: &Path) {
        let target = ImageDecodeTarget::ImportedLayer {
            layer_name: imported_layer_name(path),
        };
        self.start_image_decode(path.to_owned(), target);
    }

    fn paste_clipboard(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-image-paste-gpu-unavailable",
            )));
            return;
        };
        let max_texture_dimension_2d = renderer.max_texture_dimension_2d();

        let clipboard_sequence = crate::native_input::clipboard_sequence_number();
        if let Some(CopiedImageMetadata {
            kind: CopiedImageKind::LayerMask(payload),
            ..
        }) = self
            .copied_image
            .as_ref()
            .filter(|copied| clipboard_sequence == Some(copied.clipboard_sequence))
        {
            let command = Command::PasteLayerMask {
                layer_id: self.runtime.state.active_layer_id(),
                material_index: self.runtime.state.focused_material_index(),
                payload: payload.clone(),
            };
            if let Err(err) = self.dispatch_checked(command) {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-layer-mask-paste-failed").arg("error", err),
                ));
            }
            return;
        }

        let image = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image());
        let image = match image {
            Ok(image) => image,
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-image-paste-failed").arg("error", err),
                ));
                return;
            }
        };
        let width = match u32::try_from(image.width) {
            Ok(width) => width,
            Err(_) => {
                self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                    "status-image-paste-width-too-large",
                )));
                return;
            }
        };
        let height = match u32::try_from(image.height) {
            Ok(height) => height,
            Err(_) => {
                self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                    "status-image-paste-height-too-large",
                )));
                return;
            }
        };
        let internal_copy = self.copied_image.as_ref().filter(|copied| {
            clipboard_sequence == Some(copied.clipboard_sequence)
                && copied.image_size == [width, height]
        });

        let layer_name = internal_copy
            .map(|copied| format!("{} (copy)", copied.layer_name))
            .unwrap_or_else(|| "Clipboard Image".to_owned());
        let target_size = self.runtime.state.document().and_then(|document| {
            document
                .materials
                .get(self.runtime.state.focused_material_index())
                .map(|material| material.texture_size)
        });
        let initial_origin = internal_copy
            .filter(|copied| target_size == Some(copied.canvas_size))
            .map(|copied| copied.origin);
        let command = load_rgba8_as_paste_command_at(
            internal_copy
                .map(|copied| copied.layer_name.as_str())
                .unwrap_or("Clipboard Image"),
            layer_name,
            [width, height],
            image.bytes.into_owned(),
            max_texture_dimension_2d,
            initial_origin,
        );

        match command {
            Ok(command) => {
                if let Err(err) = self.dispatch_checked(command) {
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-image-paste-failed").arg("error", err),
                    ));
                }
            }
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-image-paste-failed")
                        .arg("error", format!("{err:#}")),
                ));
            }
        }
    }

    fn write_layer_content_to_clipboard(
        &mut self,
        copied: CopiedLayerContent,
    ) -> anyhow::Result<String> {
        let (layer_name, canvas_size, origin, size, rgba8, kind) = match copied {
            CopiedLayerContent::Raster(copied) => (
                copied.layer_name,
                copied.canvas_size,
                copied.origin,
                copied.size,
                copied.rgba8,
                CopiedImageKind::Raster,
            ),
            CopiedLayerContent::LayerMask(copied) => {
                let rgba8 = copied.payload.clipboard_rgba8()?;
                let canvas_size = copied.payload.canvas_size;
                let origin = copied.payload.origin;
                let size = copied.payload.size;
                (
                    copied.layer_name,
                    canvas_size,
                    origin,
                    size,
                    rgba8,
                    CopiedImageKind::LayerMask(copied.payload),
                )
            }
        };
        let image = arboard::ImageData {
            width: size[0] as usize,
            height: size[1] as usize,
            bytes: Cow::Owned(rgba8),
        };
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_image(image)?;
        self.copied_image =
            crate::native_input::clipboard_sequence_number().map(|clipboard_sequence| {
                CopiedImageMetadata {
                    clipboard_sequence,
                    layer_name: layer_name.clone(),
                    canvas_size,
                    origin,
                    image_size: size,
                    kind,
                }
            });
        Ok(layer_name)
    }

    fn copy_layer_target(&mut self, layer_id: crate::core::surface::LayerId) {
        let copied = match copy_layer_content(&self.runtime.state, layer_id) {
            Ok(copied) => copied,
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-image-copy-failed")
                        .arg("error", format!("{err:#}")),
                ));
                return;
            }
        };
        match self.write_layer_content_to_clipboard(copied) {
            Ok(layer_name) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-layer-copied").arg("name", layer_name),
                ));
            }
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-image-copy-failed")
                        .arg("error", format!("{err:#}")),
                ));
            }
        }
    }

    fn cut_layer_target(&mut self, layer_id: crate::core::surface::LayerId) {
        let prepared = match prepare_cut_layer_content(&self.runtime.state, layer_id) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-image-cut-failed")
                        .arg("error", format!("{err:#}")),
                ));
                return;
            }
        };
        let action = prepared.action;
        let layer_name = match self.write_layer_content_to_clipboard(prepared.copied) {
            Ok(layer_name) => layer_name,
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-image-cut-failed")
                        .arg("error", format!("{err:#}")),
                ));
                return;
            }
        };

        let command = match action {
            PreparedLayerCutAction::ClearSurface(target) => Command::CutSurfacePixels { target },
            PreparedLayerCutAction::DeleteLayerMask(layer_id) => {
                Command::DeleteLayerMask { layer_id }
            }
        };
        match self.dispatch_checked(command) {
            Ok(()) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-layer-cut").arg("name", layer_name),
                ));
            }
            Err(err) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-layer-copied-cut-failed")
                        .arg("name", layer_name)
                        .arg("error", err),
                ));
            }
        }
    }

    fn handle_dropped_files(
        &mut self,
        dropped_files: Vec<egui::DroppedFileHandle>,
        actions: EditorActionContext,
    ) -> bool {
        if dropped_files.is_empty() {
            return false;
        }
        if dropped_files.len() != 1 {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-image-drop-single-image",
            )));
            return true;
        }
        if !actions.has_document {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-image-import-load-mesh",
            )));
            return true;
        }
        if actions.tool_interacting {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-image-import-finish-operation",
            )));
            return true;
        }
        match actions.edit_block_reason {
            Some(EditorActionBlockReason::RenderCommitPending) => {
                self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                    "status-image-import-wait-edit",
                )));
                return true;
            }
            Some(EditorActionBlockReason::FatalRendererError) => {
                // Preserve the fatal renderer status; a dropped file must not replace it
                // with a temporary-state message.
                return true;
            }
            None => {}
        }

        let path = dropped_files[0].path();
        if !is_supported_raster_image_path(path) {
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-image-import-unsupported-format",
            )));
            return true;
        }
        self.import_image_path_as_layer(path);
        true
    }

    fn draw_blur_dialog(&mut self, ctx: &egui::Context, actions: EditorActionContext) -> bool {
        if !self.blur_dialog_open {
            return false;
        }
        let mut dialog = self.blur_dialog;
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let apply_enabled = actions.has_document
            && !actions.tool_interacting
            && !actions.editing_blocked()
            && self.runtime.state.blur_filter_available();

        egui::Window::new(self.localization.text("dialog-blur-title"))
            .id(egui::Id::new("blur_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(self.localization.text("dialog-blur-method"));
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut dialog.method,
                        BlurMethod::Surface,
                        self.localization.text("dialog-blur-surface"),
                    );
                    ui.selectable_value(
                        &mut dialog.method,
                        BlurMethod::Spatial,
                        self.localization.text("dialog-blur-spatial"),
                    );
                });
                ui.add_space(4.0);

                match dialog.method {
                    BlurMethod::Surface => {
                        ui.label(self.localization.text("dialog-blur-surface-help"));
                        ui.label(self.localization.text("dialog-blur-surface-detail"));
                        ui.add_space(4.0);
                        ui.add(
                            egui::Slider::new(
                                &mut dialog.surface_radius_px,
                                SURFACE_BLUR_MIN_RADIUS_PX..=SURFACE_BLUR_MAX_RADIUS_PX,
                            )
                            .text(self.localization.text("dialog-blur-reference-radius"))
                            .suffix(self.localization.text("dialog-blur-average-texel-suffix"))
                            .logarithmic(true),
                        );
                    }
                    BlurMethod::Spatial => {
                        ui.label(self.localization.text("dialog-blur-spatial-help"));
                        if self.runtime.state.active_layer_target()
                            == crate::core::document::ActiveLayerTarget::LayerMask
                        {
                            ui.label(self.localization.text("dialog-blur-spatial-mask-detail"));
                        } else {
                            ui.label(self.localization.text("dialog-blur-spatial-color-detail"));
                        }
                        ui.add_space(4.0);
                        ui.add(
                            egui::Slider::new(
                                &mut dialog.spatial_radius_px,
                                SPATIAL_BLUR_MIN_RADIUS_PX..=SPATIAL_BLUR_MAX_RADIUS_PX,
                            )
                            .text(self.localization.text("dialog-blur-reference-radius"))
                            .suffix(self.localization.text("dialog-blur-average-texel-suffix"))
                            .logarithmic(true),
                        );
                        ui.checkbox(
                            &mut dialog.cross_meshes,
                            self.localization.text("dialog-blur-cross-meshes"),
                        );
                        ui.add_space(4.0);
                        ui.label(self.localization.text("dialog-blur-orientation"));
                        ui.radio_value(
                            &mut dialog.orientation,
                            SpatialBlurOrientation::Ignore,
                            self.localization.text("dialog-blur-ignore-orientation"),
                        );
                        ui.radio_value(
                            &mut dialog.orientation,
                            SpatialBlurOrientation::SimilarNormals,
                            self.localization.text("dialog-blur-similar-normals"),
                        );
                        ui.add_enabled(
                            dialog.orientation == SpatialBlurOrientation::SimilarNormals,
                            egui::Slider::new(
                                &mut dialog.maximum_normal_angle_degrees,
                                SPATIAL_BLUR_MIN_ANGLE_DEGREES..=SPATIAL_BLUR_MAX_ANGLE_DEGREES,
                            )
                            .text(self.localization.text("dialog-blur-maximum-angle"))
                            .suffix("°"),
                        );
                    }
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(self.localization.text("action-cancel")).clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled(
                            apply_enabled,
                            egui::Button::new(self.localization.text("action-apply")),
                        )
                        .clicked()
                    {
                        apply = true;
                    }
                });
            });

        self.blur_dialog = dialog;
        if apply {
            self.blur_dialog_open = false;
            self.dispatch(dialog.apply_command());
            return true;
        }
        if cancel || !open {
            self.blur_dialog_open = false;
            return true;
        }
        false
    }

    fn request_new_project(&mut self) {
        if self.pending_project_open.is_some()
            || self.pending_project_transition.is_some()
            || self.new_project_draft.is_some()
            || self.mesh_reload_draft.is_some()
        {
            return;
        }
        self.request_project_transition(PendingProjectTransition::New);
    }

    fn request_project_transition(&mut self, transition: PendingProjectTransition) {
        if self.project_dirty {
            self.pending_project_transition = Some(transition);
        } else {
            self.perform_project_transition(transition);
        }
    }

    fn perform_project_transition(&mut self, transition: PendingProjectTransition) {
        match transition {
            PendingProjectTransition::New => {
                self.new_project_draft = Some(NewProjectDraft::default());
            }
            PendingProjectTransition::Open(path) => self.start_open_project(path),
            PendingProjectTransition::Exit => {
                self.exit_approved = true;
                self.pending_project_transition = None;
                self.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn request_open_project(&mut self) {
        if self.pending_project_open.is_some()
            || self.pending_project_transition.is_some()
            || self.new_project_draft.is_some()
            || self.mesh_reload_draft.is_some()
        {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                self.localization.text("file-filter-holopainter-project"),
                &["holopaint"],
            )
            .pick_file()
        else {
            return;
        };
        self.request_project_transition(PendingProjectTransition::Open(path));
    }

    fn request_reload_mesh(&mut self) {
        if self.pending_project_open.is_some()
            || self.pending_project_transition.is_some()
            || self.new_project_draft.is_some()
            || self.mesh_reload_draft.is_some()
        {
            return;
        }
        if self.renderer.is_none() || self.runtime.state.document().is_none() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                self.localization.text("file-filter-mesh"),
                SUPPORTED_MODEL_EXTENSIONS,
            )
            .pick_file()
        else {
            return;
        };

        let mut draft = MeshReloadDraft::default();
        if let Some(document) = self.runtime.state.document() {
            draft.load_source(document, path);
            self.mesh_reload_draft = Some(draft);
        }
    }

    fn request_export_image(&mut self) {
        let actions = self.editor_action_context(false);
        if self.export_image_draft.is_some()
            || self.export_psd_draft.is_some()
            || self.renderer.is_none()
            || actions.tool_interacting
            || actions.editing_blocked()
        {
            return;
        }
        let Some(document) = self.runtime.state.document() else {
            return;
        };
        if !document.has_materials() {
            return;
        }
        let output_dir = self
            .project_path
            .as_deref()
            .and_then(|path| self.project_history.entry(path))
            .and_then(|entry| entry.export_image_directory.clone())
            .or_else(|| {
                self.project_path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            });
        self.export_image_draft = Some(ExportImageDraft::new(
            &document.materials,
            self.runtime.state.document_generation(),
            output_dir,
        ));
    }

    fn draw_export_image(&mut self, ctx: &egui::Context) -> bool {
        let action = {
            let Some(draft) = self.export_image_draft.as_mut() else {
                return false;
            };
            draw_export_image_dialog(ctx, &self.localization, draft)
        };
        match action {
            ExportImageDialogAction::None => false,
            ExportImageDialogAction::Cancel => {
                self.export_image_draft = None;
                true
            }
            ExportImageDialogAction::Browse => {
                let initial = self
                    .export_image_draft
                    .as_ref()
                    .and_then(|draft| draft.output_dir.clone())
                    .or_else(|| {
                        self.project_path
                            .as_deref()
                            .and_then(Path::parent)
                            .map(Path::to_path_buf)
                    });
                let picker = initial
                    .as_deref()
                    .map_or_else(rfd::FileDialog::new, |directory| {
                        rfd::FileDialog::new().set_directory(directory)
                    });
                if let Some(directory) = picker.pick_folder()
                    && let Some(draft) = self.export_image_draft.as_mut()
                {
                    draft.output_dir = Some(directory);
                    draft.error = None;
                }
                true
            }
            ExportImageDialogAction::PrepareExport => {
                let prepared = self
                    .export_image_draft
                    .as_mut()
                    .expect("export draft exists while drawing")
                    .prepare_plan();
                match prepared {
                    Ok(plan) => {
                        let existing_file_names = plan
                            .files
                            .iter()
                            .filter(|file| file.output_path.exists())
                            .map(|file| file.file_name.clone())
                            .collect::<Vec<_>>();
                        if existing_file_names.is_empty() {
                            self.execute_export_image_plan(plan);
                        } else if let Some(draft) = self.export_image_draft.as_mut() {
                            draft.error = None;
                            draft.overwrite_confirmation = Some(OverwriteConfirmation {
                                plan,
                                existing_file_names,
                            });
                        }
                    }
                    Err(error) => {
                        if let Some(draft) = self.export_image_draft.as_mut() {
                            draft.error = Some(error.to_string());
                        }
                    }
                }
                true
            }
            ExportImageDialogAction::CancelOverwrite => {
                if let Some(draft) = self.export_image_draft.as_mut() {
                    draft.overwrite_confirmation = None;
                }
                true
            }
            ExportImageDialogAction::OverwriteAll => {
                let plan = self
                    .export_image_draft
                    .as_mut()
                    .and_then(|draft| draft.overwrite_confirmation.take())
                    .map(|confirmation| confirmation.plan);
                if let Some(plan) = plan {
                    self.execute_export_image_plan(plan);
                }
                true
            }
        }
    }

    fn execute_export_image_plan(&mut self, plan: ExportImagePlan) {
        let expected_sizes = match self.validate_export_image_plan(&plan) {
            Ok(sizes) => sizes,
            Err(error) => {
                self.report_export_image_error(
                    StatusMessage::localized("status-image-export-failed")
                        .arg("error", format!("{error:#}")),
                );
                return;
            }
        };

        let mut completed = 0;
        let result = (|| -> anyhow::Result<()> {
            let renderer = self
                .renderer
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("GPU renderer is unavailable."))?;
            for (file, expected_size) in plan.files.iter().zip(expected_sizes) {
                let snapshot = renderer
                    .read_composite_rgba8(file.material_index)
                    .map_err(|error| anyhow::anyhow!("{}: {error:#}", file.file_name))?;
                anyhow::ensure!(
                    snapshot.texture_size == expected_size
                        && snapshot.origin == [0, 0]
                        && snapshot.size == expected_size,
                    "{}: composite readback size {:?} does not match material size {:?}",
                    file.file_name,
                    snapshot.size,
                    expected_size
                );
                let mut rgba8 = snapshot.rgba8;
                unpremultiply_rgba8(&mut rgba8);
                image::save_buffer_with_format(
                    &file.output_path,
                    &rgba8,
                    expected_size[0],
                    expected_size[1],
                    image::ColorType::Rgba8,
                    image::ImageFormat::Png,
                )
                .map_err(|error| anyhow::anyhow!("{}: {error}", file.file_name))?;
                completed += 1;
            }
            Ok(())
        })();

        if let Err(error) = result {
            self.report_export_image_error(
                StatusMessage::localized("status-image-export-partial-failed")
                    .arg("count", completed)
                    .arg("error", format!("{error:#}")),
            );
            return;
        }

        self.record_image_export_directory(plan.output_dir.clone());

        let changed_names = {
            let document = self
                .runtime
                .state
                .document()
                .expect("export plan was validated against a document");
            plan.files
                .iter()
                .filter_map(|file| {
                    document
                        .materials
                        .iter()
                        .find(|material| material.id == file.material_id)
                        .filter(|material| {
                            material.export_image_file_name.as_deref()
                                != Some(file.file_name.as_str())
                        })
                        .map(|_| (file.material_id, file.file_name.clone()))
                })
                .collect::<Vec<_>>()
        };
        if !changed_names.is_empty()
            && let Err(error) = self.dispatch_checked(Command::SetMaterialExportImageFileNames {
                names: changed_names,
            })
        {
            self.report_export_image_error(
                StatusMessage::localized("status-image-export-save-names-failed")
                    .arg("count", completed)
                    .arg("error", error),
            );
            return;
        }

        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-images-exported")
                .arg("count", plan.files.len())
                .arg("path", plan.output_dir.display()),
        ));
        self.export_image_draft = None;
    }

    fn validate_export_image_plan(&self, plan: &ExportImagePlan) -> anyhow::Result<Vec<[u32; 2]>> {
        let document = self
            .runtime
            .state
            .document()
            .ok_or_else(|| anyhow::anyhow!("A document is no longer available."))?;
        anyhow::ensure!(self.renderer.is_some(), "GPU renderer is unavailable.");
        anyhow::ensure!(
            self.runtime.state.document_generation() == plan.document_generation,
            "The document changed while the Export Image dialog was open."
        );
        anyhow::ensure!(
            plan.output_dir.is_dir(),
            "The output folder is no longer available."
        );
        anyhow::ensure!(
            plan.files.len() == document.materials.len(),
            "The material list changed while the Export Image dialog was open."
        );
        plan.files
            .iter()
            .map(|file| {
                let material = document.materials.get(file.material_index).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Material index {} is no longer available.",
                        file.material_index
                    )
                })?;
                anyhow::ensure!(
                    material.id == file.material_id,
                    "Material {} no longer matches its export entry.",
                    material.name
                );
                Ok(material.texture_size)
            })
            .collect()
    }

    fn report_export_image_error(&mut self, message: StatusMessage) {
        let formatted = message.format(&self.localization);
        if let Some(draft) = self.export_image_draft.as_mut() {
            draft.error = Some(formatted);
            draft.overwrite_confirmation = None;
        }
        self.dispatch(Command::SetStatusMessage(message));
    }

    fn request_export_psd(&mut self) {
        if !self.pro_license.state().owns_pro() {
            self.export_psd_draft = None;
            self.pro_unlock_dialog_open = true;
            return;
        }
        let actions = self.editor_action_context(false);
        if self.export_psd_draft.is_some()
            || self.export_image_draft.is_some()
            || self.renderer.is_none()
            || actions.tool_interacting
            || actions.editing_blocked()
        {
            return;
        }
        let Some(document) = self.runtime.state.document() else {
            return;
        };
        if !document.has_materials() {
            return;
        }
        let warnings = warnings_for_document(document)
            .into_iter()
            .map(|warning| match warning {
                PsdExportWarning::EmbeddedImageRasterized { layer_name } => {
                    let mut args = fluent::FluentArgs::new();
                    args.set("name", layer_name);
                    self.localization
                        .format("export-psd-warning-embedded-image", Some(&args))
                }
                PsdExportWarning::GradientMapDitherMayDiffer { layer_name } => {
                    let mut args = fluent::FluentArgs::new();
                    args.set("name", layer_name);
                    self.localization
                        .format("export-psd-warning-gradient-dither", Some(&args))
                }
            })
            .collect();
        let output_dir = self
            .project_path
            .as_deref()
            .and_then(|path| self.project_history.entry(path))
            .and_then(|entry| entry.export_psd_directory.clone())
            .or_else(|| {
                self.project_path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            });
        self.export_psd_draft = Some(ExportPsdDraft::new(
            &document.materials,
            self.runtime.state.document_generation(),
            output_dir,
            warnings,
        ));
    }

    fn draw_export_psd(&mut self, ctx: &egui::Context) -> bool {
        let action = {
            let Some(draft) = self.export_psd_draft.as_mut() else {
                return false;
            };
            draw_export_psd_dialog(ctx, &self.localization, draft)
        };
        match action {
            ExportPsdDialogAction::None => false,
            ExportPsdDialogAction::Cancel => {
                self.export_psd_draft = None;
                true
            }
            ExportPsdDialogAction::Browse => {
                let initial = self
                    .export_psd_draft
                    .as_ref()
                    .and_then(|draft| draft.output_dir.clone())
                    .or_else(|| {
                        self.project_path
                            .as_deref()
                            .and_then(Path::parent)
                            .map(Path::to_path_buf)
                    });
                let picker = initial
                    .as_deref()
                    .map_or_else(rfd::FileDialog::new, |directory| {
                        rfd::FileDialog::new().set_directory(directory)
                    });
                if let Some(directory) = picker.pick_folder()
                    && let Some(draft) = self.export_psd_draft.as_mut()
                {
                    draft.output_dir = Some(directory);
                    draft.error = None;
                }
                true
            }
            ExportPsdDialogAction::PrepareExport => {
                let prepared = self
                    .export_psd_draft
                    .as_mut()
                    .expect("PSD export draft exists while drawing")
                    .prepare_plan();
                match prepared {
                    Ok(plan) => {
                        let existing_file_names = plan
                            .files
                            .iter()
                            .filter(|file| file.output_path.exists())
                            .map(|file| file.file_name.clone())
                            .collect::<Vec<_>>();
                        if existing_file_names.is_empty() {
                            self.execute_export_psd_plan(plan);
                        } else if let Some(draft) = self.export_psd_draft.as_mut() {
                            draft.error = None;
                            draft.overwrite_confirmation = Some(PsdOverwriteConfirmation {
                                plan,
                                existing_file_names,
                            });
                        }
                    }
                    Err(error) => {
                        if let Some(draft) = self.export_psd_draft.as_mut() {
                            draft.error = Some(error.to_string());
                        }
                    }
                }
                true
            }
            ExportPsdDialogAction::CancelOverwrite => {
                if let Some(draft) = self.export_psd_draft.as_mut() {
                    draft.overwrite_confirmation = None;
                }
                true
            }
            ExportPsdDialogAction::OverwriteAll => {
                let plan = self
                    .export_psd_draft
                    .as_mut()
                    .and_then(|draft| draft.overwrite_confirmation.take())
                    .map(|confirmation| confirmation.plan);
                if let Some(plan) = plan {
                    self.execute_export_psd_plan(plan);
                }
                true
            }
        }
    }

    fn execute_export_psd_plan(&mut self, plan: ExportPsdPlan) {
        if !self.pro_license.state().owns_pro() {
            self.export_psd_draft = None;
            self.pro_unlock_dialog_open = true;
            return;
        }
        let expected_sizes = match self.validate_export_psd_plan(&plan) {
            Ok(sizes) => sizes,
            Err(error) => {
                self.report_export_psd_error(
                    StatusMessage::localized("status-psd-export-failed")
                        .arg("error", format!("{error:#}")),
                );
                return;
            }
        };
        let snapshot = {
            let document = self
                .runtime
                .state
                .document()
                .expect("PSD export plan was validated against a document");
            PsdExportSnapshot::capture(document, plan.document_generation)
        };

        let result = (|| -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
            let renderer = self
                .renderer
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("GPU renderer is unavailable."))?;
            let mut files = Vec::with_capacity(plan.files.len());
            for (file, expected_size) in plan.files.iter().zip(expected_sizes) {
                let composite = renderer
                    .read_composite_rgba8(file.material_index)
                    .map_err(|error| anyhow::anyhow!("{}: {error:#}", file.file_name))?;
                anyhow::ensure!(
                    composite.texture_size == expected_size
                        && composite.origin == [0, 0]
                        && composite.size == expected_size,
                    "{}: composite readback size {:?} does not match material size {:?}",
                    file.file_name,
                    composite.size,
                    expected_size
                );
                let document = snapshot
                    .build_material_document(file.material_index, composite.rgba8)
                    .map_err(|error| anyhow::anyhow!("{}: {error:#}", file.file_name))?;
                let bytes = serialize_psd(&document)
                    .map_err(|error| anyhow::anyhow!("{}: {error:#}", file.file_name))?;
                files.push((file.output_path.clone(), bytes));
            }
            Ok(files)
        })();
        let files = match result {
            Ok(files) => files,
            Err(error) => {
                self.report_export_psd_error(
                    StatusMessage::localized("status-psd-export-failed")
                        .arg("error", format!("{error:#}")),
                );
                return;
            }
        };

        if self.runtime.state.document_generation() != snapshot.document_generation {
            self.report_export_psd_error(StatusMessage::localized(
                "status-psd-export-document-changed",
            ));
            return;
        }
        if let Err(error) = write_psd_files_transactionally(&files) {
            self.report_export_psd_error(
                StatusMessage::localized("status-psd-export-failed")
                    .arg("error", format!("{error:#}")),
            );
            return;
        }

        self.record_psd_export_directory(plan.output_dir.clone());

        let changed_names = {
            let document = self
                .runtime
                .state
                .document()
                .expect("PSD export plan was validated against a document");
            plan.files
                .iter()
                .filter_map(|file| {
                    document
                        .materials
                        .iter()
                        .find(|material| material.id == file.material_id)
                        .filter(|material| {
                            material.export_psd_file_name.as_deref()
                                != Some(file.file_name.as_str())
                        })
                        .map(|_| (file.material_id, file.file_name.clone()))
                })
                .collect::<Vec<_>>()
        };
        if !changed_names.is_empty()
            && let Err(error) = self.dispatch_checked(Command::SetMaterialExportPsdFileNames {
                names: changed_names,
            })
        {
            self.report_export_psd_error(
                StatusMessage::localized("status-psd-export-save-names-failed").arg("error", error),
            );
            return;
        }

        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-psd-files-exported")
                .arg("count", plan.files.len())
                .arg("path", plan.output_dir.display()),
        ));
        self.export_psd_draft = None;
    }

    fn validate_export_psd_plan(&self, plan: &ExportPsdPlan) -> anyhow::Result<Vec<[u32; 2]>> {
        let document = self
            .runtime
            .state
            .document()
            .ok_or_else(|| anyhow::anyhow!("A document is no longer available."))?;
        anyhow::ensure!(self.renderer.is_some(), "GPU renderer is unavailable.");
        anyhow::ensure!(
            self.runtime.state.document_generation() == plan.document_generation,
            "The document changed while the Export PSD dialog was open."
        );
        anyhow::ensure!(
            plan.output_dir.is_dir(),
            "The output folder is no longer available."
        );
        anyhow::ensure!(
            plan.files.len() == document.materials.len(),
            "The material list changed while the Export PSD dialog was open."
        );
        plan.files
            .iter()
            .map(|file| {
                let material = document.materials.get(file.material_index).ok_or_else(|| {
                    anyhow::anyhow!(
                        "Material index {} is no longer available.",
                        file.material_index
                    )
                })?;
                anyhow::ensure!(
                    material.id == file.material_id,
                    "Material {} no longer matches its export entry.",
                    material.name
                );
                Ok(material.texture_size)
            })
            .collect()
    }

    fn report_export_psd_error(&mut self, message: StatusMessage) {
        let formatted = message.format(&self.localization);
        if let Some(draft) = self.export_psd_draft.as_mut() {
            draft.error = Some(formatted);
            draft.overwrite_confirmation = None;
        }
        self.dispatch(Command::SetStatusMessage(message));
    }

    fn start_open_project(&mut self, path: PathBuf) {
        let request_id = self.next_project_open_request_id;
        self.next_project_open_request_id =
            self.next_project_open_request_id.wrapping_add(1).max(1);
        self.pending_project_open = Some(request_id);
        self.open_error = None;
        self.dispatch(Command::SetStatusMessage(
            StatusMessage::localized("status-project-opening").arg("path", path.display()),
        ));
        let sender = self.project_open_sender.clone();
        let repaint_ctx = self.egui_ctx.clone();
        let worker_path = path.clone();
        let spawn_result = thread::Builder::new()
            .name("holo-painter-project-open".to_owned())
            .spawn(move || {
                let result = open_project(&worker_path).map_err(|error| error.to_string());
                if sender
                    .send(ProjectOpenCompletion {
                        request_id,
                        path: worker_path,
                        result,
                    })
                    .is_ok()
                {
                    repaint_ctx.request_repaint();
                }
            });
        if let Err(error) = spawn_result {
            self.pending_project_open = None;
            let message = format!("starting project reader: {error}");
            self.open_error = Some(message.clone());
            self.dispatch(Command::SetStatusMessage(
                StatusMessage::localized("status-project-open-failed").arg("error", message),
            ));
        }
    }

    fn poll_completed_project_open(&mut self) -> bool {
        let mut changed = false;
        loop {
            let completion = match self.project_open_receiver.try_recv() {
                Ok(completion) => completion,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            if self.pending_project_open != Some(completion.request_id) {
                continue;
            }
            self.pending_project_open = None;
            changed = true;
            match completion.result {
                Ok(project) => {
                    let collapsed_layer_group_ids = project
                        .editor_state
                        .layer_tree
                        .collapsed_layer_group_ids
                        .clone();
                    if let Err(error) = self.dispatch_checked(Command::ProjectLoaded(project)) {
                        self.open_error = Some(error.clone());
                        self.dispatch(Command::SetStatusMessage(
                            StatusMessage::localized("status-project-open-failed")
                                .arg("error", error),
                        ));
                        continue;
                    }
                    self.project_path = Some(completion.path.clone());
                    self.project_name_hint = None;
                    self.project_dirty = false;
                    self.workspace
                        .restore_collapsed_layer_groups(collapsed_layer_group_ids);
                    self.touch_project_history(completion.path.clone());
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-project-opened")
                            .arg("path", completion.path.display()),
                    ));
                }
                Err(error) => {
                    self.open_error = Some(error.clone());
                    self.dispatch(Command::SetStatusMessage(
                        StatusMessage::localized("status-project-open-failed").arg("error", error),
                    ));
                }
            }
        }
        changed
    }

    fn save_current_project(&mut self) -> bool {
        if let Some(path) = self.project_path.clone() {
            self.save_current_project_to(path)
        } else {
            self.save_current_project_as()
        }
    }

    fn save_current_project_as(&mut self) -> bool {
        let default_name = default_project_filename(
            self.project_path.as_deref(),
            self.project_name_hint.as_deref(),
        );
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                self.localization.text("file-filter-holopainter-project"),
                &["holopaint"],
            )
            .set_file_name(default_name)
            .save_file()
        else {
            return false;
        };
        self.save_current_project_to(ensure_holopaint_extension(path))
    }

    fn save_current_project_to(&mut self, path: PathBuf) -> bool {
        let dirty_before_save = self.project_dirty;
        let editor_state = self
            .runtime
            .state
            .project_editor_state(self.workspace.collapsed_layer_group_ids());
        let result = self
            .runtime
            .state
            .document()
            .ok_or_else(|| "No project is loaded.".to_owned())
            .and_then(|document| {
                let editor_state = editor_state
                    .as_ref()
                    .ok_or_else(|| "No project editor state is available.".to_owned())?;
                save_project(document, editor_state, &path).map_err(|error| error.to_string())
            });
        match result {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.touch_project_history(path.clone());
                self.save_error = None;
                self.project_dirty = project_dirty_after_save(dirty_before_save, true);
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-project-saved").arg("path", path.display()),
                ));
                true
            }
            Err(error) => {
                self.project_dirty = project_dirty_after_save(dirty_before_save, false);
                self.save_error = Some(error.clone());
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-project-save-failed").arg("error", error),
                ));
                false
            }
        }
    }

    fn touch_project_history(&mut self, path: PathBuf) {
        let result = self.project_history.touch(path).and_then(|()| {
            save_project_history(self.project_history_path.as_deref(), &self.project_history)
        });
        if let Err(error) = result {
            eprintln!("Unable to update project history: {error}");
        }
    }

    fn record_image_export_directory(&mut self, directory: PathBuf) {
        let Some(path) = self.project_path.clone() else {
            return;
        };
        let result = self
            .project_history
            .set_export_image_directory(path, directory)
            .and_then(|()| {
                save_project_history(self.project_history_path.as_deref(), &self.project_history)
            });
        if let Err(error) = result {
            eprintln!("Unable to save image export directory: {error}");
        }
    }

    fn record_psd_export_directory(&mut self, directory: PathBuf) {
        let Some(path) = self.project_path.clone() else {
            return;
        };
        let result = self
            .project_history
            .set_export_psd_directory(path, directory)
            .and_then(|()| {
                save_project_history(self.project_history_path.as_deref(), &self.project_history)
            });
        if let Err(error) = result {
            eprintln!("Unable to save PSD export directory: {error}");
        }
    }

    fn draw_save_error(&mut self, ctx: &egui::Context) -> bool {
        let Some(error) = self.save_error.clone() else {
            return false;
        };
        let mut open = true;
        let mut dismissed = false;
        egui::Window::new(self.localization.text("dialog-project-save-error-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(error);
                ui.add_space(8.0);
                if ui.button(self.localization.text("action-ok")).clicked() {
                    dismissed = true;
                }
            });
        if dismissed || !open {
            self.save_error = None;
            true
        } else {
            false
        }
    }

    fn draw_open_error(&mut self, ctx: &egui::Context) -> bool {
        let Some(error) = self.open_error.clone() else {
            return false;
        };
        let mut open = true;
        let mut dismissed = false;
        egui::Window::new(self.localization.text("dialog-project-open-error-title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(error);
                ui.add_space(8.0);
                if ui.button(self.localization.text("action-ok")).clicked() {
                    dismissed = true;
                }
            });
        if dismissed || !open {
            self.open_error = None;
            true
        } else {
            false
        }
    }

    fn draw_unsaved_project_confirmation(&mut self, ctx: &egui::Context) -> bool {
        let Some(transition) = self.pending_project_transition.clone() else {
            return false;
        };
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        let save_enabled = self.editor_action_context(false).can_save_project();
        let response =
            egui::Modal::new(egui::Id::new("unsaved_project_confirmation")).show(ctx, |ui| {
                ui.heading(self.localization.text("dialog-unsaved-title"));
                ui.separator();
                ui.label(self.localization.text("dialog-unsaved-description"));
                ui.label(match &transition {
                    PendingProjectTransition::New => {
                        self.localization.text("dialog-unsaved-before-new")
                    }
                    PendingProjectTransition::Open(path) => {
                        let mut args = fluent::FluentArgs::new();
                        args.set("path", path.display().to_string());
                        self.localization
                            .format("dialog-unsaved-before-open", Some(&args))
                    }
                    PendingProjectTransition::Exit => {
                        self.localization.text("dialog-unsaved-before-exit")
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            save_enabled,
                            egui::Button::new(self.localization.text("action-save")),
                        )
                        .clicked()
                    {
                        save = true;
                    }
                    if ui
                        .button(self.localization.text("action-discard"))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button(self.localization.text("action-cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        let decision = if save {
            UnsavedConfirmationDecision::Save
        } else if discard {
            UnsavedConfirmationDecision::Discard
        } else if cancel || response.should_close() {
            UnsavedConfirmationDecision::Cancel
        } else {
            UnsavedConfirmationDecision::None
        };
        let save_succeeded =
            decision == UnsavedConfirmationDecision::Save && self.save_current_project();
        match resolve_project_transition(decision, save_succeeded) {
            ProjectTransitionResolution::Perform => {
                self.pending_project_transition = None;
                self.perform_project_transition(transition);
                true
            }
            ProjectTransitionResolution::Cancel => {
                self.pending_project_transition = None;
                true
            }
            ProjectTransitionResolution::Wait => decision != UnsavedConfirmationDecision::None,
        }
    }

    fn draw_mesh_reload(&mut self, ctx: &egui::Context) -> bool {
        let Some(draft) = self.mesh_reload_draft.as_mut() else {
            return false;
        };
        let Some(document) = self.runtime.state.document() else {
            self.mesh_reload_draft = None;
            return true;
        };
        let maximum = self
            .renderer
            .as_ref()
            .map(RenderHost::max_texture_dimension_2d);
        let action = draw_mesh_reload_dialog(ctx, &self.localization, draft, document, maximum);
        match action {
            MeshReloadDialogAction::None => false,
            MeshReloadDialogAction::Cancel => {
                self.mesh_reload_draft = None;
                true
            }
            MeshReloadDialogAction::SelectMesh => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        self.localization.text("file-filter-mesh"),
                        SUPPORTED_MODEL_EXTENSIONS,
                    )
                    .pick_file()
                    && let Some(document) = self.runtime.state.document()
                    && let Some(draft) = self.mesh_reload_draft.as_mut()
                {
                    draft.load_source(document, path);
                }
                true
            }
            MeshReloadDialogAction::Reload => {
                let prepared = self.mesh_reload_draft.as_ref().and_then(|draft| {
                    self.runtime
                        .state
                        .document()
                        .map(|document| draft.create_command(document, maximum))
                });
                let Some(result) = prepared else {
                    return false;
                };
                match result
                    .and_then(|command| self.dispatch_checked(command).map_err(anyhow::Error::msg))
                {
                    Ok(()) => {
                        self.mesh_reload_draft = None;
                    }
                    Err(error) => {
                        if let Some(draft) = self.mesh_reload_draft.as_mut() {
                            draft.error = Some(format!("{error:#}"));
                        }
                    }
                }
                true
            }
        }
    }

    fn draw_new_project(&mut self, ctx: &egui::Context) -> bool {
        let Some(draft) = self.new_project_draft.as_mut() else {
            return false;
        };
        let maximum = self
            .renderer
            .as_ref()
            .map(RenderHost::max_texture_dimension_2d);
        let action = draw_new_project_dialog(ctx, &self.localization, draft, maximum);
        match action {
            NewProjectDialogAction::None => false,
            NewProjectDialogAction::Cancel => {
                self.new_project_draft = None;
                true
            }
            NewProjectDialogAction::SelectMesh => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        self.localization.text("file-filter-mesh"),
                        SUPPORTED_MODEL_EXTENSIONS,
                    )
                    .pick_file()
                    && let Some(draft) = self.new_project_draft.as_mut()
                {
                    draft.load_source(path);
                }
                true
            }
            NewProjectDialogAction::Create => {
                let prepared = self.new_project_draft.as_ref().map(|draft| {
                    (
                        draft.create_command(maximum),
                        draft.source_name_hint(),
                        draft.materials.len(),
                    )
                });
                let Some((result, name_hint, material_count)) = prepared else {
                    return false;
                };
                match result
                    .and_then(|command| self.dispatch_checked(command).map_err(anyhow::Error::msg))
                {
                    Ok(()) => {
                        self.project_path = None;
                        self.project_name_hint = name_hint;
                        self.project_dirty = true;
                        self.workspace.restore_collapsed_layer_groups([]);
                        self.dispatch(Command::SetStatusMessage(
                            StatusMessage::localized("status-project-created")
                                .arg("count", material_count),
                        ));
                        self.new_project_draft = None;
                    }
                    Err(error) => {
                        if let Some(draft) = self.new_project_draft.as_mut() {
                            draft.error = Some(format!("{error:#}"));
                        }
                    }
                }
                true
            }
        }
    }
}

fn image_asset_display_name(localization: &Localization, path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| localization.text("asset-imported-image"))
}

fn ensure_holopaint_extension(mut path: PathBuf) -> PathBuf {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("holopaint"))
    {
        path.set_extension("holopaint");
    }
    path
}

fn default_project_filename(
    project_path: Option<&Path>,
    project_name_hint: Option<&str>,
) -> String {
    project_path
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            project_name_hint
                .filter(|name| !name.is_empty())
                .map(|name| format!("{name}.holopaint"))
        })
        .unwrap_or_else(|| "Untitled.holopaint".to_owned())
}

fn draw_screen_eyedropper_input_blocker(ctx: &egui::Context) {
    let content_rect = ctx.input(|input| input.content_rect());
    egui::Area::new(egui::Id::new("screen_eyedropper_input_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(content_rect.min)
        .default_size(content_rect.size())
        .movable(false)
        .sense(egui::Sense::click_and_drag())
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let rect = egui::Rect::from_min_size(ui.min_rect().min, content_rect.size());
            ui.allocate_rect(rect, egui::Sense::click_and_drag());
        });
}

fn draw_image_drop_overlay(
    ctx: &egui::Context,
    hovered_files: &[egui::HoveredFile],
    actions: EditorActionContext,
) {
    let Some((title, detail)) = image_drop_overlay_text(hovered_files, actions) else {
        return;
    };
    egui::Area::new(egui::Id::new("image_drop_overlay"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(title);
                    ui.label(detail);
                });
            });
        });
}

fn image_drop_overlay_text(
    hovered_files: &[egui::HoveredFile],
    actions: EditorActionContext,
) -> Option<(&'static str, &'static str)> {
    if hovered_files.is_empty() {
        return None;
    }
    if hovered_files.len() != 1 {
        return Some((
            "Drop one image file",
            "Multiple image files are not supported yet.",
        ));
    }
    if !actions.has_document {
        return Some((
            "Load a mesh first",
            "A PNG or JPEG image can be added after a glTF or GLB is loaded.",
        ));
    }
    if actions.tool_interacting {
        return Some((
            "Finish the current operation",
            "Apply or cancel it before importing another image.",
        ));
    }
    match actions.edit_block_reason {
        Some(EditorActionBlockReason::RenderCommitPending) => {
            return Some((
                "Image import is temporarily unavailable",
                "Wait for the current edit to finish.",
            ));
        }
        Some(EditorActionBlockReason::FatalRendererError) => {
            return Some((
                "Renderer error blocks image import",
                "Restart HoloPainter before continuing to edit.",
            ));
        }
        None => {}
    }
    let Some(path) = hovered_files[0].path.as_deref() else {
        return Some((
            "File path unavailable",
            "Drop a PNG or JPEG file from Windows Explorer.",
        ));
    };
    if !is_supported_raster_image_path(path) {
        return Some((
            "Unsupported image format",
            "This import supports PNG and JPEG images.",
        ));
    }
    Some((
        "Import image as a new layer",
        "Drop to adjust its position and size with Transform.",
    ))
}

fn rollback_renderer_engines(
    renderer: &mut RenderHost,
    previous: &[(
        String,
        Option<crate::core::brush_engine::BrushEngineDefinition>,
    )],
    registered_ids: &[String],
) {
    for id in registered_ids.iter().rev() {
        if let Some((_, Some(definition))) =
            previous.iter().find(|(previous_id, _)| previous_id == id)
        {
            let _ = renderer.register_brush_engine(definition);
        } else {
            renderer.unregister_brush_engine(id);
        }
    }
}

impl eframe::App for HoloPainterApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.input(|input| self.workspace_window.observe_viewport(input.viewport()));
        let mut needs_repaint = self.handle_native_close_request(&ctx);
        let hwnd = native_window::frame_hwnd(frame);
        if matches!(self.pro_license.state(), ProLicenseState::Checking) {
            self.pro_license.initialize(hwnd, &ctx);
        }
        needs_repaint |= self.pro_license.poll(&ctx);
        let screen_eyedropper_blocks_input = self.screen_eyedropper.is_active();
        if screen_eyedropper_blocks_input {
            match self.screen_eyedropper.update(&ctx) {
                ScreenEyedropperUpdate::None => {}
                ScreenEyedropperUpdate::Apply(color) => {
                    let target = std::mem::replace(
                        &mut self.screen_eyedropper_target,
                        ScreenEyedropperTarget::CurrentColor,
                    );
                    self.dispatch(match target {
                        ScreenEyedropperTarget::CurrentColor => Command::SetCurrentColor(color),
                        ScreenEyedropperTarget::SolidFill {
                            layer_id,
                            edit_session,
                        } => Command::SetSolidFillColor {
                            layer_id,
                            color,
                            edit_session,
                        },
                    });
                    needs_repaint = true;
                }
                ScreenEyedropperUpdate::Cancelled => {
                    self.screen_eyedropper_target = ScreenEyedropperTarget::CurrentColor;
                    needs_repaint = true;
                }
            }
            draw_screen_eyedropper_input_blocker(&ctx);
        }
        self.color_sampler.begin_frame();
        let tablet_settings = &self.runtime.state.user_settings().input.tablet;
        let requested_tablet_backend = tablet_settings.backend;
        let tablet_pressure_curve = tablet_settings.pressure_curve;
        if let Some(normalized_backend) = self.pointer_input.begin_frame(
            &ctx,
            frame,
            requested_tablet_backend,
            tablet_pressure_curve,
        ) {
            self.dispatch(Command::SetTabletBackend(normalized_backend));
            needs_repaint = true;
        }
        let tablet_status = self.pointer_input.tablet_status();
        let tablet_pressure_raw = self.pointer_input.tablet_pressure_raw();
        if self.renderer.is_none()
            && let Some(rs) = frame.wgpu_render_state()
            && let Ok(renderer) = RenderHost::new(
                rs,
                [1024, 1024],
                self.runtime.state.brush_engines(),
                self.runtime.state.brush_textures(),
            )
        {
            self.renderer = Some(renderer);
            self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                "status-renderer-initialized",
            )));
            needs_repaint = true;
        }

        needs_repaint |= self.poll_completed_image_decodes();
        needs_repaint |= self.poll_completed_project_open();
        needs_repaint |= self.poll_completed_color_samples();
        needs_repaint |= self.poll_completed_render_commits();
        self.refresh_texture_metrics();

        let mut frame_view_requests = Vec::new();
        let mut frame_color_sample_request = None;
        let mut frame_actions = Vec::new();
        let copy_layer = self
            .workspace
            .single_selected_copy_target(&self.runtime.state);
        let cut_layer =
            copy_layer.filter(|&layer_id| can_cut_layer_content(&self.runtime.state, layer_id));

        let shortcut_context = self.editor_action_context(
            screen_eyedropper_blocks_input
                || ctx.egui_wants_keyboard_input()
                || self.settings_window_state.is_capturing(),
        );
        let native_paste_requested = crate::native_input::take_image_paste_request();
        let native_cut_requested = crate::native_input::take_image_cut_request();
        let native_copy_requested = crate::native_input::take_image_copy_request();
        let shortcut_start =
            self.shortcut_input
                .begin_frame(&ctx, &self.runtime.state, shortcut_context);
        needs_repaint |= shortcut_start.needs_repaint;
        let save_requested = shortcut_start.save_requested;
        let save_as_requested = shortcut_start.save_as_requested;
        let open_requested = shortcut_start.open_requested;
        let image_paste_requested = native_paste_requested || shortcut_start.paste_requested;
        let image_cut_requested = native_cut_requested || shortcut_start.cut_requested;
        let image_copy_requested = native_copy_requested || shortcut_start.copy_requested;
        let apply_requested = shortcut_start.apply_requested;
        let cancel_requested = shortcut_start.cancel_requested;
        for command in shortcut_start.commands {
            self.dispatch(command);
        }
        for action in shortcut_start.actions {
            let current = self.editor_action_context(ctx.egui_wants_keyboard_input());
            if current.shortcut_is_enabled(action) {
                self.dispatch(action.command());
                needs_repaint = true;
            }
        }
        if !screen_eyedropper_blocks_input
            && image_paste_requested
            && shortcut_context.can_paste_image_from_shortcut()
        {
            self.paste_clipboard();
            needs_repaint = true;
        }
        if !screen_eyedropper_blocks_input
            && image_cut_requested
            && shortcut_context.can_cut_image_from_shortcut(cut_layer.is_some())
            && let Some(layer_id) = cut_layer
        {
            self.cut_layer_target(layer_id);
            needs_repaint = true;
        }
        if !screen_eyedropper_blocks_input
            && image_copy_requested
            && shortcut_context.can_copy_image_from_shortcut(copy_layer.is_some())
            && let Some(layer_id) = copy_layer
        {
            self.copy_layer_target(layer_id);
            needs_repaint = true;
        }
        if !screen_eyedropper_blocks_input && apply_requested {
            if self.runtime.state.has_active_decal_session() {
                self.dispatch(Command::ApplyActiveDecal);
                needs_repaint = true;
            } else if self.runtime.state.has_active_raster_transform_session() {
                self.dispatch(Command::ApplyActiveTransform);
                needs_repaint = true;
            }
        }
        if !screen_eyedropper_blocks_input && cancel_requested {
            if self.runtime.state.has_active_raster_transform_session()
                || self.runtime.state.has_active_modal_tool()
            {
                self.dispatch(Command::CancelActiveTransform);
                needs_repaint = true;
            } else if self.runtime.state.is_tool_interacting() {
                self.dispatch(Command::ToolInput(ToolInputEvent::Cancel {
                    reason: ToolCancelReason::Escape,
                }));
                needs_repaint = true;
            }
        }
        if open_requested {
            self.request_open_project();
            needs_repaint = true;
        } else if save_as_requested {
            self.save_current_project_as();
            needs_repaint = true;
        } else if save_requested {
            self.save_current_project();
            needs_repaint = true;
        }

        self.sync_decal_thumbnail(&ctx);
        let render_resources = self.render_resources();
        let focused_texture_size = self.focused_texture_size();

        let action_context = self.editor_action_context(
            screen_eyedropper_blocks_input || ctx.egui_wants_keyboard_input(),
        );
        let (hovered_files, dropped_files) = ctx.input(|input| {
            (
                input.raw.hovered_files.clone(),
                input.raw.dropped_files.clone(),
            )
        });
        if !screen_eyedropper_blocks_input {
            draw_image_drop_overlay(&ctx, &hovered_files, action_context);
        }

        let menu_output = menu_bar::draw_menu_bar(
            ui,
            &self.localization,
            &self.runtime.state,
            &self.workspace,
            &self.project_history,
            self.renderer.is_some(),
            action_context,
            copy_layer.is_some(),
            cut_layer.is_some(),
        );
        if menu_output.new_project_requested {
            self.request_new_project();
            needs_repaint = true;
        }
        if menu_output.open_project_requested {
            self.request_open_project();
            needs_repaint = true;
        }
        if let Some(path) = menu_output.open_recent_project_requested {
            self.request_project_transition(PendingProjectTransition::Open(path));
            needs_repaint = true;
        }
        if menu_output.reload_mesh_requested {
            self.request_reload_mesh();
            needs_repaint = true;
        }
        if menu_output.save_as_requested {
            self.save_current_project_as();
            needs_repaint = true;
        } else if menu_output.save_requested {
            self.save_current_project();
            needs_repaint = true;
        }
        if menu_output.export_image_requested {
            self.request_export_image();
            needs_repaint = true;
        }
        if menu_output.export_psd_requested {
            self.request_export_psd();
            needs_repaint = true;
        }
        if menu_output.quit_requested {
            self.request_project_transition(PendingProjectTransition::Exit);
            needs_repaint = true;
        }
        if let Some(kind) = menu_output.adjustment_filter_requested {
            let layer_id = self.runtime.state.active_layer_id();
            if kind == AdjustmentKind::Invert {
                self.dispatch(Command::ApplyAdjustmentFilter {
                    layer_id,
                    adjustment: kind.default_adjustment(),
                });
            } else {
                self.dispatch(Command::BeginAdjustmentFilterSession { layer_id, kind });
                if self
                    .runtime
                    .state
                    .adjustment_filter_session_matches(layer_id, kind)
                {
                    self.adjustment_filter_dialog.open_for(
                        layer_id,
                        self.runtime.state.document_generation(),
                        match self.runtime.state.active_layer_target() {
                            crate::core::document::ActiveLayerTarget::LayerMask => {
                                crate::core::surface::PaintSurfaceRole::LayerMask
                            }
                            _ => crate::core::surface::PaintSurfaceRole::Raster,
                        },
                        kind,
                    );
                }
            }
            needs_repaint = true;
        }
        if menu_output.blur_requested {
            self.blur_dialog_open = true;
            needs_repaint = true;
        }
        if menu_output.cut_image_requested
            && let Some(layer_id) = cut_layer
        {
            self.cut_layer_target(layer_id);
            needs_repaint = true;
        }
        if menu_output.copy_image_requested
            && let Some(layer_id) = copy_layer
        {
            self.copy_layer_target(layer_id);
            needs_repaint = true;
        }
        if menu_output.paste_image_requested {
            self.paste_clipboard();
            needs_repaint = true;
        }
        if menu_output.settings_requested {
            self.settings_window_open = true;
            needs_repaint = true;
        }
        if menu_output.system_information_requested {
            self.system_information_window_open = true;
            needs_repaint = true;
        }
        if menu_output.about_requested {
            self.about_window_open = true;
            needs_repaint = true;
        }
        if menu_output.reset_layout_requested {
            self.workspace.reset_layout();
            needs_repaint = true;
        }
        for (pane, visible) in menu_output.pane_visibility {
            self.workspace.set_pane_visible(pane, visible);
            needs_repaint = true;
        }
        needs_repaint |= self.apply_view_output(
            menu_output.view,
            &mut frame_view_requests,
            &mut frame_color_sample_request,
            &mut frame_actions,
        );

        let asset_library_actions = draw_asset_library_window(
            &ctx,
            &self.localization,
            &mut self.asset_library_window_open,
            &self.image_assets,
            &mut self.asset_library_ui_state,
        );
        for action in asset_library_actions {
            self.handle_asset_library_action(action);
            needs_repaint = true;
        }

        if let Some(action) = draw_image_mask_conversion_dialog(
            &ctx,
            &self.localization,
            &self.image_assets,
            &mut self.image_mask_conversion_draft,
        ) {
            self.image_mask_conversion_draft = None;
            if let ImageMaskConversionAction::Create {
                asset_id,
                required_tags,
                recipe,
            } = action
            {
                self.create_image_mask_representation(asset_id, required_tags, recipe);
            }
            needs_repaint = true;
        }

        let current_shortcut_profile = self.shortcut_input.profile().clone();
        let default_shortcut_profile = self.shortcut_input.default_profile().clone();
        let builtin_pressure_curve = SettingsFileV1::builtin_tablet_pressure_curve()
            .expect("embedded default settings must be valid");
        let holopack_import_enabled = self.renderer.is_some()
            && !action_context.tool_interacting
            && !action_context.editing_blocked();
        let settings_output = settings_window::draw_settings_window(
            &ctx,
            &mut self.settings_window_open,
            &self.localization,
            &self.runtime.state,
            &tablet_status,
            tablet_pressure_raw,
            &mut self.settings_window_state,
            &current_shortcut_profile,
            &default_shortcut_profile,
            builtin_pressure_curve,
            holopack_import_enabled,
        );
        if settings_output.import_holopack_requested {
            self.request_holopack_import();
            needs_repaint = true;
        }
        needs_repaint |= self.apply_view_output(
            settings_output.view,
            &mut frame_view_requests,
            &mut frame_color_sample_request,
            &mut frame_actions,
        );
        if let Some(engine_id) = settings_output.delete_brush_engine_id {
            self.delete_user_brush_engine(&engine_id);
            needs_repaint = true;
        }
        if let Some(profile) = settings_output.shortcut_profile {
            for command in self.shortcut_input.cancel_active_inputs() {
                self.dispatch(command);
            }
            match self.shortcut_input.replace_profile(profile.clone()) {
                Ok(()) => {
                    crate::native_input::configure_shortcuts(&profile);
                    if let Err(error) = self.shortcut_autosave.save_now(profile, |profile| {
                        profile.validate().map_err(|error| error.to_string())
                    }) {
                        self.dispatch(Command::SetStatusMessage(
                            StatusMessage::localized("status-shortcut-save-failed")
                                .arg("error", error),
                        ));
                    }
                    needs_repaint = true;
                }
                Err(error) => self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-shortcut-invalid").arg("error", error),
                )),
            }
        }

        if !screen_eyedropper_blocks_input {
            runtime_metrics::draw_runtime_metrics_window(
                &ctx,
                &self.localization,
                &mut self.runtime_metrics_window_open,
                &self.runtime_metrics,
            );
            draw_system_information_window(
                &ctx,
                &mut self.system_information_window_open,
                &self.localization,
                &self.system_information,
            );
            draw_about_window(
                &ctx,
                &mut self.about_window_open,
                &self.localization,
                &self.icons,
            );
            if self.pro_unlock_dialog_open {
                let action = draw_pro_unlock_dialog(
                    &ctx,
                    &self.localization,
                    self.pro_license.state(),
                    self.pro_license.purchase_state(),
                    self.pro_license.formatted_price(),
                    self.pro_license.product_error(),
                    self.pro_license.purchase_outcome(),
                );
                match action {
                    ProUnlockAction::None => {}
                    ProUnlockAction::Close => {
                        self.pro_unlock_dialog_open = false;
                        self.pro_license.clear_purchase_outcome();
                    }
                    ProUnlockAction::Purchase => self.pro_license.request_purchase(&ctx),
                    ProUnlockAction::Refresh => self.pro_license.retry(hwnd, &ctx),
                    ProUnlockAction::ExportPsd => {
                        self.pro_unlock_dialog_open = false;
                        self.pro_license.clear_purchase_outcome();
                        self.request_export_psd();
                    }
                }
                needs_repaint |= action != ProUnlockAction::None;
            }
            let adjustment_filter_output = draw_adjustment_filter_dialog(
                &ctx,
                &self.localization,
                &self.runtime.state,
                action_context,
                &mut self.adjustment_filter_dialog,
            );
            needs_repaint |= self.apply_view_output(
                adjustment_filter_output,
                &mut frame_view_requests,
                &mut frame_color_sample_request,
                &mut frame_actions,
            );
            needs_repaint |= self.draw_blur_dialog(&ctx, action_context);
            needs_repaint |= self.draw_save_error(&ctx);
            needs_repaint |= self.draw_open_error(&ctx);
            needs_repaint |= self.draw_holopack_dialogs(&ctx);
            if self.save_error.is_none() && self.open_error.is_none() {
                let had_unsaved_confirmation = self.pending_project_transition.is_some();
                needs_repaint |= self.draw_unsaved_project_confirmation(&ctx);
                if !had_unsaved_confirmation {
                    if self.export_image_draft.is_some() {
                        needs_repaint |= self.draw_export_image(&ctx);
                    } else if self.export_psd_draft.is_some() {
                        needs_repaint |= self.draw_export_psd(&ctx);
                    } else if self.mesh_reload_draft.is_some() {
                        needs_repaint |= self.draw_mesh_reload(&ctx);
                    } else {
                        needs_repaint |= self.draw_new_project(&ctx);
                    }
                }
            }
        }

        status_bar::draw_status_bar(
            ui,
            &self.localization,
            &self.runtime.state,
            focused_texture_size,
            &self.runtime_metrics,
            self.brush_save_error.as_deref(),
        );

        let tool_group_output = self.workspace.draw_tool_group_side_panel(
            ui,
            &self.localization,
            &self.runtime.state,
            action_context,
            &self.icons,
            self.shortcut_input.profile(),
        );
        needs_repaint |= self.apply_view_output(
            tool_group_output,
            &mut frame_view_requests,
            &mut frame_color_sample_request,
            &mut frame_actions,
        );

        let selected_decal_asset_id = self.selected_decal_asset_id().map(str::to_owned);
        let mut workspace_output = ViewOutput::default();
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(0))
            .show(ui, |ui| {
                let workspace = &mut self.workspace;
                let pointer_input = &mut self.pointer_input;
                workspace_output = workspace.draw(
                    ui,
                    WorkspaceDrawInput {
                        localization: &self.localization,
                        state: &self.runtime.state,
                        action_context,
                        focused_texture_size,
                        max_texture_dimension_2d: self
                            .renderer
                            .as_ref()
                            .map(RenderHost::max_texture_dimension_2d),
                        render_resources: &render_resources,
                        image_assets: &self.image_assets,
                        selected_decal_asset_id: selected_decal_asset_id.as_deref(),
                        pointer_input,
                        icons: &self.icons,
                        interaction_enabled: !screen_eyedropper_blocks_input,
                        shortcut_profile: self.shortcut_input.profile(),
                        shortcut_profile_revision: self.shortcut_input.profile_revision(),
                    },
                );
            });
        needs_repaint |= self.apply_view_output(
            workspace_output,
            &mut frame_view_requests,
            &mut frame_color_sample_request,
            &mut frame_actions,
        );

        if !screen_eyedropper_blocks_input {
            let adjustment_editor_output = self.workspace.draw_adjustment_editor(
                &ctx,
                &self.localization,
                &self.runtime.state,
            );
            needs_repaint |= self.apply_view_output(
                adjustment_editor_output,
                &mut frame_view_requests,
                &mut frame_color_sample_request,
                &mut frame_actions,
            );
        }

        for command in self.shortcut_input.finish_frame() {
            self.dispatch(command);
            needs_repaint = true;
        }

        if !screen_eyedropper_blocks_input {
            for action in frame_actions {
                let current = self.editor_action_context(ctx.egui_wants_keyboard_input());
                if current.is_enabled(action) {
                    self.dispatch(action.command());
                    needs_repaint = true;
                }
            }
        }

        let (focused, primary_down) =
            ctx.input(|input| (input.focused, input.pointer.primary_down()));
        if !screen_eyedropper_blocks_input {
            let reason = if self.runtime.state.is_tool_pointer_gesture_active() && !focused {
                Some(ToolCancelReason::FocusLost)
            } else if self.runtime.state.is_tool_pointer_gesture_active()
                && !primary_down
                && !self.pointer_input.is_active()
            {
                Some(ToolCancelReason::PointerCaptureLost)
            } else {
                None
            };
            if let Some(reason) = reason {
                self.dispatch(Command::ToolInput(ToolInputEvent::Cancel { reason }));
                needs_repaint = true;
            }
        }

        let drop_context = self.editor_action_context(ctx.egui_wants_keyboard_input());
        if !screen_eyedropper_blocks_input {
            needs_repaint |= self.handle_dropped_files(dropped_files, drop_context);
        }
        needs_repaint |= self.color_sampler.finish_frame();

        let frame_time_s = ctx.input(|input| input.time);
        if !screen_eyedropper_blocks_input
            && self
                .runtime
                .state
                .active_stroke_next_emit_time_s()
                .is_some_and(|next_emit_time_s| frame_time_s >= next_emit_time_s)
        {
            self.dispatch(Command::AdvanceActiveStroke {
                time_s: frame_time_s,
            });
            needs_repaint = true;
        }

        needs_repaint |= self.execute_frame(frame_view_requests, frame_color_sample_request);

        match self.screen_eyedropper.start_if_requested(
            native_window::frame_hwnd(frame),
            self.localization.text("screen-eyedropper-no-sample"),
            self.localization.text("screen-eyedropper-instructions"),
        ) {
            Ok(started) => {
                needs_repaint |= started;
            }
            Err(error) => {
                self.dispatch(Command::SetStatusMessage(
                    StatusMessage::localized("status-screen-eyedropper-failed").arg("error", error),
                ));
                needs_repaint = true;
            }
        }

        let autosave_now = Instant::now();
        self.settings_autosave.poll(
            autosave_now,
            || {
                Ok(SettingsFileV1::from_user_settings(
                    self.runtime.state.user_settings().clone(),
                ))
            },
            SettingsFileV1::validate,
        );
        self.workspace_autosave.poll(
            autosave_now,
            || WorkspaceFileV1::capture(&self.workspace, &self.workspace_window),
            WorkspaceFileV1::validate,
        );
        self.editor_session_autosave.poll(
            autosave_now,
            || EditorSessionFileV1::capture(&self.runtime.state),
            EditorSessionFileV1::validate,
        );
        if autosave_now >= self.next_brush_save_check {
            self.next_brush_save_check = autosave_now + RON_AUTOSAVE_INTERVAL;
            match self.persist_tool_layout() {
                Ok(()) => {
                    if self.brush_save_error.take().is_some() {
                        self.dispatch(Command::SetStatusMessage(StatusMessage::localized(
                            "status-brush-autosave-recovered",
                        )));
                    }
                }
                Err(error) => {
                    if self.brush_save_error.as_ref() != Some(&error) {
                        self.dispatch(Command::SetStatusMessage(
                            StatusMessage::localized("status-brush-autosave-failed")
                                .arg("error", &error),
                        ));
                    }
                    self.brush_save_error = Some(error);
                }
            }
        }
        if let Some(delay) = [
            self.settings_autosave.time_until_check(autosave_now),
            self.workspace_autosave.time_until_check(autosave_now),
            self.editor_session_autosave.time_until_check(autosave_now),
            Some(
                self.next_brush_save_check
                    .saturating_duration_since(autosave_now),
            ),
        ]
        .into_iter()
        .flatten()
        .min()
        {
            ctx.request_repaint_after(delay);
        }

        if let Some(next_emit_time_s) = self.runtime.state.active_stroke_next_emit_time_s() {
            ctx.request_repaint_after(Duration::from_secs_f64(
                (next_emit_time_s - frame_time_s).max(0.0),
            ));
        }
        if needs_repaint {
            ctx.request_repaint();
        }
        self.sync_window_title(&ctx);
    }

    fn on_exit(&mut self) {
        if let Err(error) = self.persist_tool_layout() {
            eprintln!("Failed to save brush presets and tool layout on exit: {error}");
        }
        if let Err(error) = self.persist_workspace() {
            eprintln!("Failed to save workspace on exit: {error}");
        }
        if let Err(error) = self.persist_editor_session() {
            eprintln!("Failed to save editor session on exit: {error}");
        }
    }
}

fn window_title(project_filename: Option<&str>, project_dirty: bool) -> String {
    let Some(project_filename) = project_filename else {
        return "HoloPainter".to_owned();
    };
    let dirty_marker = if project_dirty { "*" } else { "" };
    format!("{project_filename}{dirty_marker} - HoloPainter")
}

fn close_request_transition(
    project_dirty: bool,
    exit_approved: bool,
) -> Option<PendingProjectTransition> {
    if project_dirty && !exit_approved {
        Some(PendingProjectTransition::Exit)
    } else {
        None
    }
}

fn resolve_project_transition(
    decision: UnsavedConfirmationDecision,
    save_succeeded: bool,
) -> ProjectTransitionResolution {
    match decision {
        UnsavedConfirmationDecision::Save if save_succeeded => ProjectTransitionResolution::Perform,
        UnsavedConfirmationDecision::Discard => ProjectTransitionResolution::Perform,
        UnsavedConfirmationDecision::Cancel => ProjectTransitionResolution::Cancel,
        UnsavedConfirmationDecision::None | UnsavedConfirmationDecision::Save => {
            ProjectTransitionResolution::Wait
        }
    }
}

fn project_dirty_after_save(project_dirty: bool, save_succeeded: bool) -> bool {
    project_dirty && !save_succeeded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noto_sans_jp_has_the_required_font_priorities() {
        let fonts = holopainter_font_definitions();
        assert!(fonts.font_data.contains_key(NOTO_SANS_JP_FONT_NAME));
        assert_eq!(
            fonts.families[&egui::FontFamily::Proportional].first(),
            Some(&NOTO_SANS_JP_FONT_NAME.to_owned())
        );
        assert_eq!(
            fonts.families[&egui::FontFamily::Monospace].last(),
            Some(&NOTO_SANS_JP_FONT_NAME.to_owned())
        );
    }

    #[test]
    fn no_document_title_is_app_name() {
        assert_eq!(window_title(None, false), "HoloPainter");
        assert_eq!(window_title(None, true), "HoloPainter");
    }

    #[test]
    fn saved_project_title_contains_filename() {
        assert_eq!(
            window_title(Some("Material.holopaint"), false),
            "Material.holopaint - HoloPainter"
        );
    }

    #[test]
    fn dirty_project_title_has_asterisk() {
        assert_eq!(
            window_title(Some("Material.holopaint"), true),
            "Material.holopaint* - HoloPainter"
        );
    }

    #[test]
    fn untitled_dirty_project_uses_default_filename() {
        let filename = default_project_filename(None, None);
        assert_eq!(
            window_title(Some(&filename), true),
            "Untitled.holopaint* - HoloPainter"
        );
    }

    #[test]
    fn name_hint_is_used_for_unsaved_project() {
        let filename = default_project_filename(None, Some("SourceName"));
        assert_eq!(
            window_title(Some(&filename), true),
            "SourceName.holopaint* - HoloPainter"
        );
    }

    #[test]
    fn close_request_only_creates_exit_transition_when_dirty_and_unapproved() {
        assert_eq!(close_request_transition(false, false), None);
        assert_eq!(
            close_request_transition(true, false),
            Some(PendingProjectTransition::Exit)
        );
        assert_eq!(close_request_transition(true, true), None);
    }

    #[test]
    fn exit_confirmation_resolves_save_discard_and_cancel() {
        assert_eq!(
            resolve_project_transition(UnsavedConfirmationDecision::Save, true),
            ProjectTransitionResolution::Perform
        );
        assert_eq!(
            resolve_project_transition(UnsavedConfirmationDecision::Save, false),
            ProjectTransitionResolution::Wait
        );
        assert_eq!(
            resolve_project_transition(UnsavedConfirmationDecision::Discard, false),
            ProjectTransitionResolution::Perform
        );
        assert_eq!(
            resolve_project_transition(UnsavedConfirmationDecision::Cancel, false),
            ProjectTransitionResolution::Cancel
        );
    }

    #[test]
    fn successful_save_cleans_project_and_failed_save_preserves_dirty_state() {
        assert!(!project_dirty_after_save(true, true));
        assert!(project_dirty_after_save(true, false));
    }

    #[test]
    fn project_filename_uses_project_then_hint_then_untitled() {
        assert_eq!(
            default_project_filename(
                Some(Path::new("C:/projects/current.holopaint")),
                Some("source"),
            ),
            "current.holopaint"
        );
        assert_eq!(
            default_project_filename(None, Some("source")),
            "source.holopaint"
        );
        assert_eq!(default_project_filename(None, None), "Untitled.holopaint");
    }

    #[test]
    fn project_extension_is_added_or_normalized() {
        assert_eq!(
            ensure_holopaint_extension(PathBuf::from("paint")),
            PathBuf::from("paint.holopaint")
        );
        assert_eq!(
            ensure_holopaint_extension(PathBuf::from("paint.HOLOPAINT")),
            PathBuf::from("paint.HOLOPAINT")
        );
        assert_eq!(
            ensure_holopaint_extension(PathBuf::from("paint.txt")),
            PathBuf::from("paint.holopaint")
        );
    }

    #[test]
    fn blur_dialog_defaults_to_surface_and_keeps_method_specific_settings() {
        let mut dialog = BlurDialogState::default();

        assert_eq!(dialog.method, BlurMethod::Surface);
        assert_eq!(dialog.surface_radius_px, 8.0);
        assert_eq!(dialog.spatial_radius_px, 8.0);
        assert!(dialog.cross_meshes);
        assert_eq!(dialog.orientation, SpatialBlurOrientation::Ignore);
        assert_eq!(dialog.maximum_normal_angle_degrees, 60.0);

        dialog.surface_radius_px = 3.0;
        dialog.method = BlurMethod::Spatial;
        dialog.spatial_radius_px = 12.0;
        dialog.cross_meshes = false;
        dialog.method = BlurMethod::Surface;

        assert_eq!(dialog.surface_radius_px, 3.0);
        assert_eq!(dialog.spatial_radius_px, 12.0);
        assert!(!dialog.cross_meshes);
    }

    #[test]
    fn blur_dialog_builds_the_selected_blur_command() {
        let surface = BlurDialogState {
            surface_radius_px: 4.0,
            ..BlurDialogState::default()
        };
        assert!(matches!(
            surface.apply_command(),
            Command::ApplySurfaceBlur { radius_px: 4.0 }
        ));

        let spatial = BlurDialogState {
            method: BlurMethod::Spatial,
            spatial_radius_px: 10.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::SimilarNormals,
            maximum_normal_angle_degrees: 45.0,
            ..BlurDialogState::default()
        };
        assert!(matches!(
            spatial.apply_command(),
            Command::ApplySpatialBlur {
                radius_px: 10.0,
                cross_meshes: false,
                orientation: SpatialBlurOrientation::SimilarNormals,
                maximum_normal_angle_degrees: 45.0,
            }
        ));
    }

    #[test]
    fn pending_image_decodes_reject_duplicates_and_ignore_stale_completions() {
        let decal = ImageDecodeTarget::ImageAsset {
            source_path: PathBuf::from("decal.png"),
            intent: ImageAssetImportIntent::Decal,
        };
        let imported = ImageDecodeTarget::ImportedLayer {
            layer_name: "Layer".to_owned(),
        };
        let mut pending = PendingImageDecodes::default();

        assert!(pending.begin(&decal, 1));
        assert!(!pending.begin(&decal, 2));
        assert!(pending.begin(&imported, 3));
        assert!(!pending.complete(&decal, 2));
        assert!(pending.complete(&decal, 1));
        assert!(pending.complete(&imported, 3));
        assert!(!pending.is_pending(&decal));
        assert!(!pending.is_pending(&imported));
    }

    fn ready_actions() -> EditorActionContext {
        EditorActionContext {
            has_document: true,
            can_undo: false,
            can_redo: false,
            can_deselect: false,
            can_delete_selected_pixels: false,
            tool_interacting: false,
            decal_session_active: false,
            edit_block_reason: None,
            keyboard_input_owned_by_ui: false,
        }
    }

    fn hovered(path: &str) -> egui::HoveredFile {
        egui::HoveredFile {
            path: Some(PathBuf::from(path)),
            mime: String::new(),
        }
    }

    #[test]
    fn image_drop_extensions_are_case_insensitive() {
        for path in [
            "image.png",
            "image.PNG",
            "image.jpg",
            "image.JPG",
            "image.jpeg",
            "image.JPEG",
        ] {
            assert!(is_supported_raster_image_path(Path::new(path)), "{path}");
        }
        assert!(!is_supported_raster_image_path(Path::new("image.gif")));
    }

    #[test]
    fn supported_image_hover_reports_import_action() {
        for path in ["image.png", "image.jpg", "image.JPEG"] {
            assert_eq!(
                image_drop_overlay_text(&[hovered(path)], ready_actions()),
                Some((
                    "Import image as a new layer",
                    "Drop to adjust its position and size with Transform.",
                ))
            );
        }
    }

    #[test]
    fn unsupported_image_hover_reports_the_explicit_allowlist() {
        assert_eq!(
            image_drop_overlay_text(&[hovered("image.gif")], ready_actions()),
            Some((
                "Unsupported image format",
                "This import supports PNG and JPEG images.",
            ))
        );
    }

    #[test]
    fn multiple_hovered_files_are_rejected_as_a_group() {
        assert_eq!(
            image_drop_overlay_text(
                &[hovered("first.png"), hovered("second.png")],
                ready_actions(),
            ),
            Some((
                "Drop one image file",
                "Multiple image files are not supported yet.",
            ))
        );
    }

    #[test]
    fn image_drop_overlay_distinguishes_pending_and_fatal_renderer_blocks() {
        let pending = EditorActionContext {
            edit_block_reason: Some(EditorActionBlockReason::RenderCommitPending),
            ..ready_actions()
        };
        assert_eq!(
            image_drop_overlay_text(&[hovered("image.png")], pending),
            Some((
                "Image import is temporarily unavailable",
                "Wait for the current edit to finish.",
            ))
        );

        let fatal = EditorActionContext {
            edit_block_reason: Some(EditorActionBlockReason::FatalRendererError),
            ..ready_actions()
        };
        assert_eq!(
            image_drop_overlay_text(&[hovered("image.png")], fatal),
            Some((
                "Renderer error blocks image import",
                "Restart HoloPainter before continuing to edit.",
            ))
        );
    }
}
