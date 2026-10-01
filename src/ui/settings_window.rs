use eframe::egui;

use crate::{
    application::{AppState, Command},
    core::{curve::Curve, tool_layout::ToolGroupDefinition},
    localization::Localization,
    settings::{AppTheme, LanguagePreference, TabletBackend},
    ui::{
        input::{
            shortcut_profile::{
                ModifierMatch, PointerContext, PointerGestureAction, PointerGestureTrigger,
                ShortcutAction, ShortcutBinding, ShortcutChord, ShortcutDiagnostic,
                ShortcutDiagnosticReason, ShortcutKey, ShortcutPointerButton, ShortcutProfile,
                ToolShortcutTarget, TransientOverrideTarget,
            },
            tablet::TabletBackendStatus,
        },
        view_output::ViewOutput,
        widgets::curve_editor::draw_curve_editor,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SettingsSection {
    #[default]
    Appearance,
    Input,
    Shortcuts,
    Navigation,
    BrushEngines,
    Extensions,
}

#[derive(Debug, Default)]
pub(crate) struct SettingsWindowState {
    section: SettingsSection,
    capture: Option<CaptureTarget>,
    captured_modifiers: Option<ModifierMatch>,
    pressure_curve_selected_point: Option<usize>,
}

impl SettingsWindowState {
    pub(crate) fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaptureTarget {
    Invoke {
        action: ShortcutAction,
        index: Option<usize>,
    },
    Tool {
        target: ToolShortcutTarget,
        index: Option<usize>,
    },
    Temporary {
        target: TransientOverrideTarget,
        index: Option<usize>,
    },
}

#[derive(Default)]
pub(crate) struct SettingsWindowOutput {
    pub(crate) view: ViewOutput,
    pub(crate) shortcut_profile: Option<ShortcutProfile>,
    pub(crate) delete_brush_engine_id: Option<String>,
    pub(crate) import_holopack_requested: bool,
}

pub(crate) fn draw_settings_window(
    ctx: &egui::Context,
    open: &mut bool,
    l10n: &Localization,
    state: &AppState,
    tablet_status: &TabletBackendStatus,
    tablet_pressure_raw: Option<f32>,
    ui_state: &mut SettingsWindowState,
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    builtin_pressure_curve: Curve,
    holopack_import_enabled: bool,
) -> SettingsWindowOutput {
    let mut output = SettingsWindowOutput::default();
    if !*open {
        ui_state.capture = None;
        ui_state.captured_modifiers = None;
        return output;
    }

    if let Some(change) = capture_change(ctx, ui_state) {
        process_change(profile, ui_state, &mut output.shortcut_profile, change);
    }

    egui::Window::new(l10n.text("settings-title"))
        .id(egui::Id::new("settings_window"))
        .open(open)
        .default_width(720.0)
        .default_height(520.0)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(120.0);
                    ui.selectable_value(
                        &mut ui_state.section,
                        SettingsSection::Appearance,
                        l10n.text("settings-section-appearance"),
                    );
                    ui.selectable_value(
                        &mut ui_state.section,
                        SettingsSection::Input,
                        l10n.text("settings-section-input"),
                    );
                    ui.selectable_value(
                        &mut ui_state.section,
                        SettingsSection::Shortcuts,
                        l10n.text("settings-section-shortcuts"),
                    );
                    ui.selectable_value(
                        &mut ui_state.section,
                        SettingsSection::Navigation,
                        l10n.text("settings-section-navigation"),
                    );
                    ui.selectable_value(
                        &mut ui_state.section,
                        SettingsSection::BrushEngines,
                        l10n.text("settings-section-brush-engines"),
                    );
                    ui.selectable_value(
                        &mut ui_state.section,
                        SettingsSection::Extensions,
                        l10n.text("settings-section-extensions"),
                    );
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("settings_content_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_min_width(540.0);
                            match ui_state.section {
                                SettingsSection::Appearance => {
                                    draw_appearance_settings(ui, l10n, state, &mut output.view);
                                }
                                SettingsSection::Input => {
                                    output.view.extend(draw_input_settings(
                                        ui,
                                        l10n,
                                        state,
                                        tablet_status,
                                        tablet_pressure_raw,
                                        ui_state,
                                        builtin_pressure_curve,
                                    ));
                                }
                                SettingsSection::Shortcuts => draw_shortcuts(
                                    ui,
                                    l10n,
                                    state.tool_layout_groups(),
                                    profile,
                                    default_profile,
                                    ui_state,
                                    &mut output.shortcut_profile,
                                ),
                                SettingsSection::Navigation => draw_navigation(
                                    ui,
                                    l10n,
                                    state.tool_layout_groups(),
                                    profile,
                                    default_profile,
                                    ui_state,
                                    &mut output.shortcut_profile,
                                ),
                                SettingsSection::BrushEngines => draw_brush_engines(
                                    ui,
                                    l10n,
                                    state,
                                    &mut output.delete_brush_engine_id,
                                ),
                                SettingsSection::Extensions => draw_extensions(
                                    ui,
                                    l10n,
                                    holopack_import_enabled,
                                    &mut output.import_holopack_requested,
                                ),
                            }
                        });
                    });
            });
        });

    output
}

fn draw_extensions(
    ui: &mut egui::Ui,
    l10n: &Localization,
    import_enabled: bool,
    import_requested: &mut bool,
) {
    ui.heading(l10n.text("settings-extensions-title"));
    ui.strong(l10n.text("settings-extensions-experimental"));
    ui.label(l10n.text("settings-extensions-description"));
    ui.add_space(12.0);
    if ui
        .add_enabled(
            import_enabled,
            egui::Button::new(l10n.text("settings-extensions-import-holopack")),
        )
        .clicked()
    {
        *import_requested = true;
    }
}

fn draw_brush_engines(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    delete_requested: &mut Option<String>,
) {
    use crate::core::brush_engine::BrushEngineOrigin;

    ui.heading(l10n.text("settings-brush-engines-title"));
    ui.add_space(12.0);
    let mut engines = state.brush_engines().engines().collect::<Vec<_>>();
    engines.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    for engine in engines {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.strong(crate::ui::localized::brush_engine_name(l10n, state, engine));
                ui.small(engine.id.as_str());
            });
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match state.brush_engines().origin(&engine.id) {
                    Some(BrushEngineOrigin::Builtin) => {
                        ui.label(l10n.text("settings-resource-origin-builtin"));
                    }
                    Some(BrushEngineOrigin::User) => {
                        if ui.button(l10n.text("action-delete")).clicked() {
                            *delete_requested = Some(engine.id.clone());
                        }
                        ui.label(l10n.text("settings-resource-origin-user"));
                    }
                    None => {}
                },
            );
        });
        ui.separator();
    }
}

fn draw_input_settings(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    tablet_status: &TabletBackendStatus,
    tablet_pressure_raw: Option<f32>,
    ui_state: &mut SettingsWindowState,
    builtin_pressure_curve: Curve,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    ui.heading(l10n.text("settings-input-title"));
    ui.add_space(12.0);
    ui.strong(l10n.text("settings-input-tablet"));
    ui.add_space(6.0);
    draw_tablet_settings(
        ui,
        l10n,
        state,
        tablet_status,
        tablet_pressure_raw,
        ui_state,
        builtin_pressure_curve,
        &mut output,
    );
    output
}

fn draw_appearance_settings(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    output: &mut ViewOutput,
) {
    ui.heading(l10n.text("settings-appearance-title"));
    ui.add_space(12.0);

    let current = state.user_settings().appearance.theme;
    let mut selected = current;
    egui::Grid::new("settings_appearance_grid")
        .num_columns(2)
        .spacing([20.0, 8.0])
        .show(ui, |ui| {
            ui.label(l10n.text("settings-appearance-theme"));
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(theme_label(l10n, selected))
                .show_ui(ui, |ui| {
                    for theme in AppTheme::ALL {
                        ui.selectable_value(&mut selected, theme, theme_label(l10n, theme));
                    }
                });
            ui.end_row();

            let current_language = state.user_settings().appearance.language;
            let mut selected_language = current_language;
            ui.label(l10n.text("settings-appearance-language"));
            egui::ComboBox::from_id_salt("settings_language")
                .selected_text(language_label(l10n, selected_language))
                .show_ui(ui, |ui| {
                    for language in LanguagePreference::ALL {
                        ui.selectable_value(
                            &mut selected_language,
                            language,
                            language_label(l10n, language),
                        );
                    }
                });
            ui.end_row();

            if selected_language != current_language {
                output.push(Command::SetLanguage(selected_language));
                output.request_repaint();
            }
        });

    if selected != current {
        output.push(Command::SetTheme(selected));
        output.request_repaint();
    }
}

fn theme_label(l10n: &Localization, theme: AppTheme) -> String {
    l10n.text(match theme {
        AppTheme::Light => "settings-theme-light",
        AppTheme::Dark => "settings-theme-dark",
    })
}

fn language_label(l10n: &Localization, language: LanguagePreference) -> String {
    match language {
        LanguagePreference::System => {
            let mut args = fluent::FluentArgs::new();
            args.set("language", l10n.system_locale().language_name());
            l10n.format("settings-language-system", Some(&args))
        }
        LanguagePreference::English => l10n.text("settings-language-english"),
        LanguagePreference::Japanese => l10n.text("settings-language-japanese"),
    }
}

fn capture_change(
    ctx: &egui::Context,
    ui_state: &mut SettingsWindowState,
) -> Option<BindingChange> {
    let target = ui_state.capture.clone()?;
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        cancel_capture(ui_state);
        return None;
    }
    let existing_index = match &target {
        CaptureTarget::Invoke { index, .. }
        | CaptureTarget::Tool { index, .. }
        | CaptureTarget::Temporary { index, .. } => *index,
    };
    if ctx.input(|input| {
        input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
    }) {
        cancel_capture(ui_state);
        return existing_index.map(BindingChange::Remove);
    }
    if let CaptureTarget::Temporary { target, .. } = &target {
        let modifiers = ModifierMatch::from_egui(ctx.input(|input| input.modifiers));
        if !modifiers.is_empty() {
            let captured = ui_state
                .captured_modifiers
                .get_or_insert_with(ModifierMatch::default);
            captured.ctrl |= modifiers.ctrl;
            captured.shift |= modifiers.shift;
            captured.alt |= modifiers.alt;
            return None;
        }
        let modifiers = ui_state.captured_modifiers.take()?;
        ui_state.capture = None;
        return Some(BindingChange::Set {
            index: existing_index,
            binding: ShortcutBinding::HoldOverride {
                modifiers,
                target: target.clone(),
            },
        });
    }
    let event = ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some((*key, *modifiers)),
            _ => None,
        })
    })?;
    let (key, modifiers) = event;
    let key = ShortcutKey::from_egui(key)?;
    let binding = match target {
        CaptureTarget::Invoke { action, .. } => ShortcutBinding::Invoke {
            chord: ShortcutChord {
                key,
                ctrl: modifiers.ctrl,
                shift: modifiers.shift,
                alt: modifiers.alt,
            },
            action,
        },
        CaptureTarget::Tool { target, .. } => ShortcutBinding::PressOrHoldTool { key, target },
        CaptureTarget::Temporary { .. } => unreachable!(),
    };
    cancel_capture(ui_state);
    Some(BindingChange::Set {
        index: existing_index,
        binding,
    })
}

#[derive(Debug, Clone)]
enum BindingChange {
    Set {
        index: Option<usize>,
        binding: ShortcutBinding,
    },
    Remove(usize),
    ReplaceProfile(ShortcutProfile),
}

fn process_change(
    profile: &ShortcutProfile,
    _ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
    change: BindingChange,
) {
    match change {
        BindingChange::Remove(index) => {
            let mut candidate = profile.clone();
            if index < candidate.bindings.len() {
                candidate.bindings.remove(index);
                if candidate.validate().is_ok() {
                    *output = Some(candidate);
                }
            }
        }
        BindingChange::ReplaceProfile(candidate) => {
            if candidate.validate().is_ok() {
                *output = Some(candidate);
            }
        }
        BindingChange::Set { index, binding } => {
            let mut candidate = profile.clone();
            set_binding(&mut candidate.bindings, index, binding);
            if candidate.validate().is_ok() {
                *output = Some(candidate);
            }
        }
    }
}

fn set_binding(
    bindings: &mut Vec<ShortcutBinding>,
    replace_index: Option<usize>,
    binding: ShortcutBinding,
) {
    if let Some(index) = replace_index.filter(|index| *index < bindings.len()) {
        bindings[index] = binding;
    } else {
        bindings.push(binding);
    }
}

fn draw_shortcuts(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    ui.horizontal(|ui| {
        ui.heading(l10n.text("settings-shortcuts-title"));
        if ui
            .button(l10n.text("settings-shortcuts-reset-category"))
            .clicked()
        {
            process_change(
                profile,
                ui_state,
                output,
                BindingChange::ReplaceProfile(reset_shortcut_category(profile, default_profile)),
            );
        }
        if ui.button(l10n.text("settings-reset-all")).clicked() {
            process_change(
                profile,
                ui_state,
                output,
                BindingChange::ReplaceProfile(default_profile.clone()),
            );
        }
    });
    draw_diagnostic_summary(ui, l10n, tool_groups, profile);
    ui.add_space(10.0);
    ui.strong(l10n.text("settings-shortcuts-general"));
    egui::Grid::new("settings_general_shortcuts_grid")
        .num_columns(2)
        .spacing([20.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for (action, _) in ACTION_ROWS {
                draw_invoke_row(
                    ui,
                    l10n,
                    profile,
                    default_profile,
                    tool_groups,
                    *action,
                    ui_state,
                    output,
                );
            }
        });

    ui.add_space(12.0);
    ui.strong(l10n.text("settings-shortcuts-tools"));
    egui::Grid::new("settings_tool_shortcuts_grid")
        .num_columns(2)
        .spacing([20.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for group in tool_groups {
                draw_tool_row(
                    ui,
                    l10n,
                    profile,
                    default_profile,
                    tool_groups,
                    group,
                    ui_state,
                    output,
                );
            }
        });

    ui.add_space(12.0);
    ui.strong(l10n.text("settings-shortcuts-temporary"));
    draw_hold_rows(
        ui,
        l10n,
        tool_groups,
        profile,
        default_profile,
        ui_state,
        output,
    );
    if ui_state.capture.is_some() {
        ui.add_space(8.0);
        ui.colored_label(
            ui.visuals().warn_fg_color,
            l10n.text("settings-shortcuts-capture-help"),
        );
    }
}

const ACTION_ROWS: &[(ShortcutAction, &str)] = &[
    (ShortcutAction::OpenProject, "shortcut-action-open-project"),
    (ShortcutAction::SaveProject, "shortcut-action-save-project"),
    (
        ShortcutAction::SaveProjectAs,
        "shortcut-action-save-project-as",
    ),
    (ShortcutAction::Undo, "shortcut-action-undo"),
    (ShortcutAction::Redo, "shortcut-action-redo"),
    (ShortcutAction::CutImage, "shortcut-action-cut"),
    (ShortcutAction::CopyImage, "shortcut-action-copy"),
    (ShortcutAction::PasteImage, "shortcut-action-paste"),
    (ShortcutAction::SelectAll, "shortcut-action-select-all"),
    (
        ShortcutAction::InvertSelection,
        "shortcut-action-invert-selection",
    ),
    (ShortcutAction::Deselect, "shortcut-action-deselect"),
    (
        ShortcutAction::DeleteSelectedPixels,
        "shortcut-action-delete-selected-pixels",
    ),
    (
        ShortcutAction::BeginTransformMode,
        "shortcut-action-transform",
    ),
    (
        ShortcutAction::ApplyActiveOperation,
        "shortcut-action-apply-active-operation",
    ),
    (
        ShortcutAction::CancelActiveOperation,
        "shortcut-action-cancel-active-operation",
    ),
];

fn draw_invoke_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    tool_groups: &[ToolGroupDefinition],
    action: ShortcutAction,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    let bindings: Vec<_> = profile
        .bindings
        .iter()
        .enumerate()
        .filter_map(|(index, binding)| match binding {
            ShortcutBinding::Invoke {
                chord,
                action: candidate,
            } if *candidate == action => Some((index, chord_display(l10n, *chord))),
            _ => None,
        })
        .collect();
    let has_bindings = !bindings.is_empty();
    ui.label(action_label(l10n, action));
    ui.horizontal_wrapped(|ui| {
        let mut capturing = false;
        for (index, text) in bindings {
            let target = CaptureTarget::Invoke {
                action,
                index: Some(index),
            };
            if ui_state.capture.as_ref() == Some(&target) {
                capturing = true;
                draw_capture_controls(ui, l10n, ui_state);
            } else if binding_button(ui, l10n, tool_groups, &text, profile, index) {
                start_capture(ui_state, target);
                capturing = true;
            }
        }
        let add_target = CaptureTarget::Invoke {
            action,
            index: None,
        };
        let adding = ui_state.capture.as_ref() == Some(&add_target);
        if !has_bindings && !adding {
            ui.weak(l10n.text("settings-shortcuts-not-assigned"));
        }
        if adding {
            capturing = true;
            draw_capture_controls(ui, l10n, ui_state);
        } else if !capturing && ui.small_button("+").clicked() {
            start_capture(ui_state, add_target);
            capturing = true;
        }
        if !capturing && ui.small_button(l10n.text("action-reset")).clicked() {
            reset_binding_identity(
                profile,
                default_profile,
                &ShortcutBinding::Invoke {
                    chord: ShortcutChord::unmodified(ShortcutKey::A),
                    action,
                },
                ui_state,
                output,
            );
        }
    });
    ui.end_row();
}

fn draw_tool_row(
    ui: &mut egui::Ui,
    l10n: &Localization,
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    tool_groups: &[ToolGroupDefinition],
    group: &ToolGroupDefinition,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    let target = ToolShortcutTarget::ToolGroup(group.id.clone());
    let bindings: Vec<_> = profile
        .bindings
        .iter()
        .enumerate()
        .filter_map(|(index, binding)| match binding {
            ShortcutBinding::PressOrHoldTool {
                key,
                target: candidate,
            } if candidate == &target => Some((index, shortcut_key_label(l10n, *key))),
            _ => None,
        })
        .collect();
    let has_bindings = !bindings.is_empty();
    ui.label(crate::ui::localized::tool_group_definition_name(
        l10n, group,
    ));
    ui.horizontal_wrapped(|ui| {
        let mut capturing = false;
        for (index, text) in bindings {
            let capture_target = CaptureTarget::Tool {
                target: target.clone(),
                index: Some(index),
            };
            if ui_state.capture.as_ref() == Some(&capture_target) {
                capturing = true;
                draw_capture_controls(ui, l10n, ui_state);
            } else if binding_button(ui, l10n, tool_groups, &text, profile, index) {
                start_capture(ui_state, capture_target);
                capturing = true;
            }
        }
        let add_target = CaptureTarget::Tool {
            target: target.clone(),
            index: None,
        };
        let adding = ui_state.capture.as_ref() == Some(&add_target);
        if !has_bindings && !adding {
            ui.weak(l10n.text("settings-shortcuts-not-assigned"));
        }
        if adding {
            capturing = true;
            draw_capture_controls(ui, l10n, ui_state);
        } else if !capturing && ui.small_button("+").clicked() {
            start_capture(ui_state, add_target);
            capturing = true;
        }
        if !capturing && ui.small_button(l10n.text("action-reset")).clicked() {
            reset_binding_identity(
                profile,
                default_profile,
                &ShortcutBinding::PressOrHoldTool {
                    key: ShortcutKey::A,
                    target,
                },
                ui_state,
                output,
            );
        }
    });
    ui.end_row();
}

fn draw_capture_controls(
    ui: &mut egui::Ui,
    l10n: &Localization,
    ui_state: &mut SettingsWindowState,
) {
    ui.add(egui::Button::new(l10n.text("settings-shortcuts-press")).selected(true));
    if ui.small_button(l10n.text("action-cancel")).clicked() {
        cancel_capture(ui_state);
    }
}

fn binding_button(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    text: &str,
    profile: &ShortcutProfile,
    binding_index: usize,
) -> bool {
    let diagnostic = binding_diagnostic(profile, binding_index);
    let label = if diagnostic.is_some() {
        format!("{text} ⚠")
    } else {
        text.to_owned()
    };
    let response = ui.button(label);
    let clicked = response.clicked();
    if let Some(diagnostic) = diagnostic {
        response.on_hover_text(diagnostic_message(l10n, tool_groups, profile, &diagnostic));
    }
    clicked
}

fn draw_binding_diagnostic(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
    binding_index: usize,
) {
    if let Some(diagnostic) = binding_diagnostic(profile, binding_index) {
        ui.colored_label(ui.visuals().warn_fg_color, "⚠")
            .on_hover_text(diagnostic_message(l10n, tool_groups, profile, &diagnostic));
    }
}

fn draw_diagnostic_summary(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
) {
    let diagnostics = profile.diagnostics();
    let count = diagnostics.len();
    if count > 0 {
        let mut args = fluent::FluentArgs::new();
        args.set("count", count as i64);
        ui.colored_label(
            ui.visuals().warn_fg_color,
            l10n.format("settings-shortcuts-inactive-count", Some(&args)),
        )
        .on_hover_text(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic_message(l10n, tool_groups, profile, diagnostic))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }
}

fn binding_diagnostic(
    profile: &ShortcutProfile,
    binding_index: usize,
) -> Option<ShortcutDiagnostic> {
    profile
        .diagnostics()
        .into_iter()
        .find(|diagnostic| diagnostic.binding_index == binding_index)
}

fn diagnostic_message(
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
    diagnostic: &ShortcutDiagnostic,
) -> String {
    match diagnostic.reason {
        ShortcutDiagnosticReason::Conflict { conflicts_with } => {
            let mut args = fluent::FluentArgs::new();
            args.set(
                "trigger",
                binding_trigger_description(l10n, &profile.bindings[diagnostic.binding_index]),
            );
            args.set(
                "assignment",
                binding_description(l10n, tool_groups, &profile.bindings[conflicts_with]),
            );
            l10n.format("settings-shortcuts-diagnostic-conflict", Some(&args))
        }
        ShortcutDiagnosticReason::Duplicate { duplicates } => {
            let mut args = fluent::FluentArgs::new();
            args.set(
                "assignment",
                binding_description(l10n, tool_groups, &profile.bindings[duplicates]),
            );
            l10n.format("settings-shortcuts-diagnostic-duplicate", Some(&args))
        }
        ShortcutDiagnosticReason::InvalidActionContext => {
            l10n.text("settings-shortcuts-diagnostic-invalid-context")
        }
        ShortcutDiagnosticReason::EmptyModifiers => {
            l10n.text("settings-shortcuts-diagnostic-empty-modifiers")
        }
        ShortcutDiagnosticReason::EmptyTarget => {
            l10n.text("settings-shortcuts-diagnostic-empty-target")
        }
    }
}

fn start_capture(ui_state: &mut SettingsWindowState, target: CaptureTarget) {
    ui_state.capture = Some(target);
    ui_state.captured_modifiers = None;
}

fn cancel_capture(ui_state: &mut SettingsWindowState) {
    ui_state.capture = None;
    ui_state.captured_modifiers = None;
}

fn draw_hold_rows(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    let mut targets: Vec<_> = default_profile
        .bindings
        .iter()
        .filter_map(|binding| match binding {
            ShortcutBinding::HoldOverride { target, .. } => Some(target.clone()),
            _ => None,
        })
        .collect();
    for target in profile.bindings.iter().filter_map(|binding| match binding {
        ShortcutBinding::HoldOverride { target, .. } => Some(target),
        _ => None,
    }) {
        if !targets.contains(target) {
            targets.push(target.clone());
        }
    }
    egui::Grid::new("settings_temporary_shortcuts_grid")
        .num_columns(2)
        .spacing([20.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for target in targets {
                let binding =
                    profile.bindings.iter().enumerate().find_map(
                        |(index, binding)| match binding {
                            ShortcutBinding::HoldOverride {
                                modifiers,
                                target: candidate,
                            } if candidate == &target => {
                                Some((index, modifier_display(l10n, modifiers)))
                            }
                            _ => None,
                        },
                    );
                ui.label(override_label(l10n, &target));
                ui.horizontal_wrapped(|ui| {
                    let capture_target = CaptureTarget::Temporary {
                        target: target.clone(),
                        index: binding.as_ref().map(|(index, _)| *index),
                    };
                    if ui_state.capture.as_ref() == Some(&capture_target) {
                        draw_capture_controls(ui, l10n, ui_state);
                    } else {
                        let text = binding.as_ref().map_or_else(
                            || l10n.text("settings-shortcuts-not-assigned"),
                            |(_, text)| text.clone(),
                        );
                        if let Some((index, _)) = binding.as_ref()
                            && binding_button(ui, l10n, tool_groups, &text, profile, *index)
                        {
                            start_capture(ui_state, capture_target);
                        } else if binding.is_none() && ui.button(text).clicked() {
                            start_capture(ui_state, capture_target);
                        }
                        if ui.small_button(l10n.text("action-reset")).clicked() {
                            reset_binding_identity(
                                profile,
                                default_profile,
                                &ShortcutBinding::HoldOverride {
                                    modifiers: ModifierMatch::default(),
                                    target: target.clone(),
                                },
                                ui_state,
                                output,
                            );
                        }
                    }
                });
                ui.end_row();
            }
        });
}

fn draw_navigation(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    ui.horizontal(|ui| {
        ui.heading(l10n.text("settings-navigation-title"));
        if ui
            .button(l10n.text("settings-navigation-reset-category"))
            .clicked()
        {
            process_change(
                profile,
                ui_state,
                output,
                BindingChange::ReplaceProfile(reset_navigation_category(profile, default_profile)),
            );
        }
        if ui.button(l10n.text("settings-reset-all")).clicked() {
            process_change(
                profile,
                ui_state,
                output,
                BindingChange::ReplaceProfile(default_profile.clone()),
            );
        }
    });
    draw_diagnostic_summary(ui, l10n, tool_groups, profile);
    ui.add_space(10.0);
    draw_navigation_context(
        ui,
        l10n,
        tool_groups,
        profile,
        default_profile,
        NavigationSection {
            context: PointerContext::Viewport3d,
            heading_key: "settings-navigation-view-3d",
            actions: &[
                PointerGestureAction::Orbit3d,
                PointerGestureAction::Pan3d,
                PointerGestureAction::Zoom3d,
                PointerGestureAction::BrushSize,
            ],
        },
        ui_state,
        output,
    );
    ui.add_space(14.0);
    draw_navigation_context(
        ui,
        l10n,
        tool_groups,
        profile,
        default_profile,
        NavigationSection {
            context: PointerContext::Uv,
            heading_key: "settings-navigation-view-uv",
            actions: &[
                PointerGestureAction::RotateUv,
                PointerGestureAction::PanUv,
                PointerGestureAction::ZoomUv,
                PointerGestureAction::BrushSize,
            ],
        },
        ui_state,
        output,
    );
}

struct NavigationSection<'a> {
    context: PointerContext,
    heading_key: &'static str,
    actions: &'a [PointerGestureAction],
}

fn draw_navigation_context(
    ui: &mut egui::Ui,
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    section: NavigationSection<'_>,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    ui.strong(l10n.text(section.heading_key));
    for action in section.actions {
        ui.horizontal(|ui| {
            ui.strong(pointer_action_label(l10n, *action));
            if ui
                .small_button("+")
                .on_hover_text(l10n.text("settings-shortcuts-add-binding"))
                .clicked()
            {
                process_change(
                    profile,
                    ui_state,
                    output,
                    BindingChange::Set {
                        index: None,
                        binding: ShortcutBinding::PointerGesture {
                            context: section.context,
                            trigger: PointerGestureTrigger {
                                button: ShortcutPointerButton::Primary,
                                modifiers: ModifierMatch::default(),
                            },
                            action: *action,
                        },
                    },
                );
            }
            if ui.small_button(l10n.text("action-reset")).clicked() {
                reset_binding_identity(
                    profile,
                    default_profile,
                    &ShortcutBinding::PointerGesture {
                        context: section.context,
                        trigger: PointerGestureTrigger {
                            button: ShortcutPointerButton::Primary,
                            modifiers: ModifierMatch::default(),
                        },
                        action: *action,
                    },
                    ui_state,
                    output,
                );
            }
        });
        let rows: Vec<_> = profile
            .bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| match binding {
                ShortcutBinding::PointerGesture {
                    context: candidate_context,
                    trigger,
                    action: candidate_action,
                } if *candidate_context == section.context && candidate_action == action => {
                    Some((index, trigger.clone()))
                }
                _ => None,
            })
            .collect();
        for (index, mut trigger) in rows {
            ui.indent(("navigation_binding", index), |ui| {
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt(("pointer_button", index))
                        .selected_text(pointer_button_label(l10n, trigger.button))
                        .show_ui(ui, |ui| {
                            for button in ShortcutPointerButton::ALL {
                                ui.selectable_value(
                                    &mut trigger.button,
                                    button,
                                    pointer_button_label(l10n, button),
                                );
                            }
                        });
                    modifier_checkbox(
                        ui,
                        l10n.text("shortcut-modifier-control"),
                        &mut trigger.modifiers.ctrl,
                    );
                    modifier_checkbox(
                        ui,
                        l10n.text("shortcut-modifier-shift"),
                        &mut trigger.modifiers.shift,
                    );
                    modifier_checkbox(
                        ui,
                        l10n.text("shortcut-modifier-alt"),
                        &mut trigger.modifiers.alt,
                    );
                    draw_binding_diagnostic(ui, l10n, tool_groups, profile, index);
                    if ui
                        .small_button("-")
                        .on_hover_text(l10n.text("settings-shortcuts-delete-binding"))
                        .clicked()
                    {
                        process_change(profile, ui_state, output, BindingChange::Remove(index));
                    }
                });
            });
            let updated = ShortcutBinding::PointerGesture {
                context: section.context,
                trigger,
                action: *action,
            };
            if updated != profile.bindings[index] {
                process_change(
                    profile,
                    ui_state,
                    output,
                    BindingChange::Set {
                        index: Some(index),
                        binding: updated,
                    },
                );
            }
        }
        ui.add_space(5.0);
    }
}

fn modifier_checkbox(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, value: &mut bool) {
    ui.checkbox(value, label);
}

fn reset_binding_identity(
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
    identity: &ShortcutBinding,
    ui_state: &mut SettingsWindowState,
    output: &mut Option<ShortcutProfile>,
) {
    let mut candidate = profile.clone();
    candidate
        .bindings
        .retain(|binding| !same_binding_identity(binding, identity));
    candidate.bindings.extend(
        default_profile
            .bindings
            .iter()
            .filter(|binding| same_binding_identity(binding, identity))
            .cloned(),
    );
    process_change(
        profile,
        ui_state,
        output,
        BindingChange::ReplaceProfile(candidate),
    );
}

fn same_binding_identity(left: &ShortcutBinding, right: &ShortcutBinding) -> bool {
    match (left, right) {
        (
            ShortcutBinding::Invoke { action: left, .. },
            ShortcutBinding::Invoke { action: right, .. },
        ) => left == right,
        (
            ShortcutBinding::PressOrHoldTool { target: left, .. },
            ShortcutBinding::PressOrHoldTool { target: right, .. },
        ) => left == right,
        (
            ShortcutBinding::HoldOverride { target: left, .. },
            ShortcutBinding::HoldOverride { target: right, .. },
        ) => left == right,
        (
            ShortcutBinding::PointerGesture {
                context: lc,
                action: la,
                ..
            },
            ShortcutBinding::PointerGesture {
                context: rc,
                action: ra,
                ..
            },
        ) => lc == rc && la == ra,
        _ => false,
    }
}

fn reset_shortcut_category(
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
) -> ShortcutProfile {
    let mut result = profile.clone();
    result
        .bindings
        .retain(|binding| matches!(binding, ShortcutBinding::PointerGesture { .. }));
    result.bindings.extend(
        default_profile
            .bindings
            .iter()
            .filter(|binding| !matches!(binding, ShortcutBinding::PointerGesture { .. }))
            .cloned(),
    );
    result
}

fn reset_navigation_category(
    profile: &ShortcutProfile,
    default_profile: &ShortcutProfile,
) -> ShortcutProfile {
    let mut result = profile.clone();
    result
        .bindings
        .retain(|binding| !matches!(binding, ShortcutBinding::PointerGesture { .. }));
    result.bindings.extend(
        default_profile
            .bindings
            .iter()
            .filter(|binding| matches!(binding, ShortcutBinding::PointerGesture { .. }))
            .cloned(),
    );
    result
}

fn binding_description(
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    binding: &ShortcutBinding,
) -> String {
    match binding {
        ShortcutBinding::Invoke { action, .. } => action_label(l10n, *action),
        ShortcutBinding::PressOrHoldTool { target, .. } => target_label(l10n, tool_groups, target),
        ShortcutBinding::HoldOverride { target, .. } => override_label(l10n, target),
        ShortcutBinding::PointerGesture {
            context, action, ..
        } => format!(
            "{} / {}",
            context_label(l10n, *context),
            pointer_action_label(l10n, *action)
        ),
    }
}

fn binding_trigger_description(l10n: &Localization, binding: &ShortcutBinding) -> String {
    match binding {
        ShortcutBinding::Invoke { chord, .. } => chord_display(l10n, *chord),
        ShortcutBinding::PressOrHoldTool { key, .. } => shortcut_key_label(l10n, *key),
        ShortcutBinding::HoldOverride { modifiers, .. } => modifier_display(l10n, modifiers),
        ShortcutBinding::PointerGesture { trigger, .. } => pointer_trigger_display(l10n, trigger),
    }
}

fn action_label(l10n: &Localization, action: ShortcutAction) -> String {
    ACTION_ROWS
        .iter()
        .find_map(|(candidate, key)| (*candidate == action).then(|| l10n.text(key)))
        .unwrap_or_else(|| l10n.text("shortcut-action-unknown"))
}

fn target_label(
    l10n: &Localization,
    tool_groups: &[ToolGroupDefinition],
    target: &ToolShortcutTarget,
) -> String {
    tool_groups
        .iter()
        .find_map(|group| {
            (target.id() == group.id)
                .then(|| crate::ui::localized::tool_group_definition_name(l10n, group))
        })
        .unwrap_or_else(|| target.id().to_owned())
}

fn override_label(l10n: &Localization, target: &TransientOverrideTarget) -> String {
    match target {
        TransientOverrideTarget::ActiveToolOverride(id) => {
            let mut chars = id.chars();
            let display_id = chars.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_uppercase(), chars.as_str())
            });
            let mut args = fluent::FluentArgs::new();
            args.set("name", display_id);
            l10n.format("settings-shortcuts-temporary-name", Some(&args))
        }
    }
}

fn context_label(l10n: &Localization, context: PointerContext) -> String {
    l10n.text(match context {
        PointerContext::Viewport3d => "settings-navigation-view-3d",
        PointerContext::Uv => "settings-navigation-view-uv",
    })
}

fn draw_tablet_settings(
    ui: &mut egui::Ui,
    l10n: &Localization,
    state: &AppState,
    status: &TabletBackendStatus,
    raw_pressure: Option<f32>,
    ui_state: &mut SettingsWindowState,
    builtin_pressure_curve: Curve,
    output: &mut ViewOutput,
) {
    let requested = state.user_settings().input.tablet.backend;
    egui::Grid::new("settings_tablet_grid")
        .num_columns(2)
        .spacing([20.0, 8.0])
        .show(ui, |ui| {
            ui.label(l10n.text("settings-tablet-input-api"));
            if !status.availability_resolved {
                ui.add_enabled(
                    false,
                    egui::Button::new(l10n.text("settings-tablet-detecting")),
                );
            } else if status.available_backends.is_empty() {
                ui.add_enabled(
                    false,
                    egui::Button::new(l10n.text("settings-tablet-no-api")),
                );
            } else {
                let mut selected = requested;
                egui::ComboBox::from_id_salt("settings_tablet_backend")
                    .selected_text(tablet_backend_label(selected))
                    .show_ui(ui, |ui| {
                        for backend in &status.available_backends {
                            ui.selectable_value(
                                &mut selected,
                                *backend,
                                tablet_backend_label(*backend),
                            );
                        }
                    });
                if selected != requested {
                    output.push(Command::SetTabletBackend(selected));
                    output.request_repaint();
                }
            }
            ui.end_row();

            ui.label(l10n.text("settings-tablet-status"));
            ui.label(if !status.availability_resolved {
                l10n.text("settings-tablet-status-detecting")
            } else if status.active_backend.is_some() {
                l10n.text("settings-tablet-status-ready")
            } else {
                l10n.text("settings-tablet-status-unavailable")
            });
            ui.end_row();
        });

    if status.availability_resolved {
        for backend in TabletBackend::PRIORITY {
            if let Some(error) = status.error(backend) {
                ui.small(format!("{}: {error}", tablet_backend_label(backend)));
            }
        }
    }

    ui.add_space(12.0);
    ui.strong(l10n.text("settings-tablet-pressure-curve"));
    ui.add_space(4.0);

    let mut curve = state.user_settings().input.tablet.pressure_curve;
    let graph = draw_curve_editor(
        ui,
        &mut curve,
        &mut ui_state.pressure_curve_selected_point,
        ui.visuals().selection.stroke.color,
        Some(220.0),
        raw_pressure,
    );
    let adjusted_pressure = raw_pressure.map(|pressure| curve.evaluate(pressure));
    if graph.changed {
        output.push(Command::SetTabletPressureCurve(curve));
        output.request_repaint();
    }

    egui::Grid::new("settings_tablet_pressure_grid")
        .num_columns(2)
        .spacing([20.0, 6.0])
        .show(ui, |ui| {
            draw_pressure_value(ui, l10n.text("settings-tablet-pressure-raw"), raw_pressure);
            draw_pressure_value(
                ui,
                l10n.text("settings-tablet-pressure-adjusted"),
                adjusted_pressure,
            );
        });

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                curve != builtin_pressure_curve,
                egui::Button::new(l10n.text("settings-tablet-reset-default")),
            )
            .on_hover_text(l10n.text("settings-tablet-reset-default-help"))
            .clicked()
        {
            ui_state.pressure_curve_selected_point = None;
            output.push(Command::SetTabletPressureCurve(builtin_pressure_curve));
            output.request_repaint();
        }
        if ui
            .add_enabled(
                curve != Curve::default(),
                egui::Button::new(l10n.text("settings-tablet-reset-linear")),
            )
            .on_hover_text(l10n.text("settings-tablet-reset-linear-help"))
            .clicked()
        {
            ui_state.pressure_curve_selected_point = None;
            output.push(Command::SetTabletPressureCurve(Curve::default()));
            output.request_repaint();
        }
    });
}

fn draw_pressure_value(ui: &mut egui::Ui, label: String, value: Option<f32>) {
    ui.label(label);
    let text = value.map_or_else(|| "--".to_owned(), |value| format!("{value:.2}"));
    ui.add(egui::ProgressBar::new(value.unwrap_or(0.0)).text(text));
    ui.end_row();
}

const fn tablet_backend_label(backend: TabletBackend) -> &'static str {
    match backend {
        TabletBackend::WinTab => "WinTab",
        TabletBackend::WindowsInk => "Windows Ink",
    }
}

fn pointer_action_label(l10n: &Localization, action: PointerGestureAction) -> String {
    l10n.text(match action {
        PointerGestureAction::Orbit3d => "pointer-action-orbit",
        PointerGestureAction::Pan3d => "pointer-action-pan",
        PointerGestureAction::Zoom3d => "pointer-action-zoom",
        PointerGestureAction::RotateUv => "pointer-action-rotate",
        PointerGestureAction::PanUv => "pointer-action-pan",
        PointerGestureAction::ZoomUv => "pointer-action-zoom",
        PointerGestureAction::BrushSize => "pointer-action-brush-size",
    })
}

fn pointer_button_label(l10n: &Localization, button: ShortcutPointerButton) -> String {
    l10n.text(match button {
        ShortcutPointerButton::Primary => "pointer-button-primary",
        ShortcutPointerButton::Middle => "pointer-button-middle",
        ShortcutPointerButton::Secondary => "pointer-button-secondary",
        ShortcutPointerButton::Extra1 => "pointer-button-extra-1",
        ShortcutPointerButton::Extra2 => "pointer-button-extra-2",
    })
}

fn chord_display(l10n: &Localization, chord: ShortcutChord) -> String {
    let mut parts = modifier_parts(l10n, chord.ctrl, chord.shift, chord.alt);
    parts.push(shortcut_key_label(l10n, chord.key));
    parts.join("+")
}

fn modifier_display(l10n: &Localization, modifiers: &ModifierMatch) -> String {
    modifier_parts(l10n, modifiers.ctrl, modifiers.shift, modifiers.alt).join("+")
}

fn pointer_trigger_display(l10n: &Localization, trigger: &PointerGestureTrigger) -> String {
    let mut parts = modifier_parts(
        l10n,
        trigger.modifiers.ctrl,
        trigger.modifiers.shift,
        trigger.modifiers.alt,
    );
    parts.push(pointer_button_label(l10n, trigger.button));
    parts.join(" + ")
}

fn modifier_parts(l10n: &Localization, ctrl: bool, shift: bool, alt: bool) -> Vec<String> {
    let mut parts = Vec::new();
    if ctrl {
        parts.push(l10n.text("shortcut-modifier-control"));
    }
    if shift {
        parts.push(l10n.text("shortcut-modifier-shift"));
    }
    if alt {
        parts.push(l10n.text("shortcut-modifier-alt"));
    }
    parts
}

fn shortcut_key_label(l10n: &Localization, key: ShortcutKey) -> String {
    let key_name = match key {
        ShortcutKey::Space => Some("shortcut-key-space"),
        ShortcutKey::Enter => Some("shortcut-key-enter"),
        ShortcutKey::Escape => Some("shortcut-key-escape"),
        ShortcutKey::Tab => Some("shortcut-key-tab"),
        ShortcutKey::Backspace => Some("shortcut-key-backspace"),
        ShortcutKey::Delete => Some("shortcut-key-delete"),
        ShortcutKey::ArrowUp => Some("shortcut-key-arrow-up"),
        ShortcutKey::ArrowDown => Some("shortcut-key-arrow-down"),
        ShortcutKey::ArrowLeft => Some("shortcut-key-arrow-left"),
        ShortcutKey::ArrowRight => Some("shortcut-key-arrow-right"),
        ShortcutKey::Home => Some("shortcut-key-home"),
        ShortcutKey::End => Some("shortcut-key-end"),
        ShortcutKey::PageUp => Some("shortcut-key-page-up"),
        ShortcutKey::PageDown => Some("shortcut-key-page-down"),
        _ => None,
    };
    key_name.map_or_else(|| key.label().to_owned(), |key| l10n.text(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::input::shortcut_profile::bindings_share_trigger;

    #[test]
    fn system_language_label_describes_the_system_locale_not_the_active_locale() {
        let english_ui =
            Localization::with_system_locale(LanguagePreference::English, Some("ja-JP"));
        assert_eq!(
            language_label(&english_ui, LanguagePreference::System),
            "System Default (日本語)"
        );

        let japanese_ui =
            Localization::with_system_locale(LanguagePreference::Japanese, Some("en-US"));
        assert_eq!(
            language_label(&japanese_ui, LanguagePreference::System),
            "システム設定 (English)"
        );
    }

    #[test]
    fn shortcut_tool_group_labels_use_stable_id_localization() {
        let l10n = Localization::with_system_locale(LanguagePreference::Japanese, None);
        let layout = crate::core::tool_layout::ToolLayoutFileV1::builtin().unwrap();
        let target = ToolShortcutTarget::ToolGroup("tool.brush".to_owned());

        assert_eq!(target_label(&l10n, &layout.tools, &target), "ブラシ");
    }

    #[test]
    fn conflict_detection_is_scoped_by_pointer_context() {
        let trigger = PointerGestureTrigger {
            button: ShortcutPointerButton::Middle,
            modifiers: ModifierMatch::default(),
        };
        let left = ShortcutBinding::PointerGesture {
            context: PointerContext::Viewport3d,
            trigger: trigger.clone(),
            action: PointerGestureAction::Pan3d,
        };
        let right = ShortcutBinding::PointerGesture {
            context: PointerContext::Uv,
            trigger,
            action: PointerGestureAction::PanUv,
        };
        assert!(!bindings_share_trigger(&left, &right));
    }

    #[test]
    fn keyboard_and_tool_shortcuts_conflict_when_unmodified() {
        let invoke = ShortcutBinding::Invoke {
            chord: ShortcutChord::unmodified(ShortcutKey::B),
            action: ShortcutAction::Undo,
        };
        let tool = ShortcutBinding::PressOrHoldTool {
            key: ShortcutKey::B,
            target: ToolShortcutTarget::ToolGroup("tool.brush".to_owned()),
        };
        assert!(bindings_share_trigger(&invoke, &tool));
    }

    #[test]
    fn selection_actions_are_available_in_shortcut_settings() {
        assert!(ACTION_ROWS.contains(&(ShortcutAction::SelectAll, "shortcut-action-select-all")));
        assert!(ACTION_ROWS.contains(&(
            ShortcutAction::InvertSelection,
            "shortcut-action-invert-selection"
        )));
    }

    #[test]
    fn action_reset_restores_all_default_bindings() {
        let default_profile = ShortcutProfile::load_default().unwrap();
        let mut profile = default_profile.clone();
        profile.bindings.retain(|binding| {
            !matches!(
                binding,
                ShortcutBinding::Invoke {
                    action: ShortcutAction::Redo,
                    ..
                }
            )
        });
        profile.bindings.push(ShortcutBinding::Invoke {
            chord: ShortcutChord::unmodified(ShortcutKey::F12),
            action: ShortcutAction::Redo,
        });
        let identity = profile.bindings.last().unwrap().clone();
        let mut ui_state = SettingsWindowState::default();
        let mut output = None;

        reset_binding_identity(
            &profile,
            &default_profile,
            &identity,
            &mut ui_state,
            &mut output,
        );

        let reset = output.unwrap();
        let reset_redo: Vec<_> = reset
            .bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding,
                    ShortcutBinding::Invoke {
                        action: ShortcutAction::Redo,
                        ..
                    }
                )
            })
            .collect();
        let default_redo: Vec<_> = default_profile
            .bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding,
                    ShortcutBinding::Invoke {
                        action: ShortcutAction::Redo,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(reset_redo, default_redo);
    }

    #[test]
    fn navigation_action_reset_restores_all_default_bindings_for_only_its_context() {
        let default_profile = ShortcutProfile::load_default().unwrap();
        let mut profile = default_profile.clone();
        profile.bindings.retain(|binding| {
            !matches!(
                binding,
                ShortcutBinding::PointerGesture {
                    context: PointerContext::Viewport3d,
                    action: PointerGestureAction::Orbit3d,
                    ..
                }
            )
        });
        profile.bindings.extend([
            ShortcutBinding::PointerGesture {
                context: PointerContext::Viewport3d,
                trigger: PointerGestureTrigger {
                    button: ShortcutPointerButton::Middle,
                    modifiers: ModifierMatch {
                        shift: true,
                        ..Default::default()
                    },
                },
                action: PointerGestureAction::Orbit3d,
            },
            ShortcutBinding::PointerGesture {
                context: PointerContext::Viewport3d,
                trigger: PointerGestureTrigger {
                    button: ShortcutPointerButton::Extra1,
                    modifiers: ModifierMatch::default(),
                },
                action: PointerGestureAction::Orbit3d,
            },
        ]);
        let uv_bindings_before: Vec<_> = profile
            .bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding,
                    ShortcutBinding::PointerGesture {
                        context: PointerContext::Uv,
                        ..
                    }
                )
            })
            .cloned()
            .collect();
        let identity = profile.bindings.last().unwrap().clone();
        let mut ui_state = SettingsWindowState::default();
        let mut output = None;

        reset_binding_identity(
            &profile,
            &default_profile,
            &identity,
            &mut ui_state,
            &mut output,
        );

        let reset = output.unwrap();
        let reset_orbit: Vec<_> = reset
            .bindings
            .iter()
            .filter(|binding| same_binding_identity(binding, &identity))
            .collect();
        let default_orbit: Vec<_> = default_profile
            .bindings
            .iter()
            .filter(|binding| same_binding_identity(binding, &identity))
            .collect();
        let uv_bindings_after: Vec<_> = reset
            .bindings
            .iter()
            .filter(|binding| {
                matches!(
                    binding,
                    ShortcutBinding::PointerGesture {
                        context: PointerContext::Uv,
                        ..
                    }
                )
            })
            .cloned()
            .collect();
        assert_eq!(reset_orbit, default_orbit);
        assert_eq!(uv_bindings_after, uv_bindings_before);
    }

    #[test]
    fn conflicting_navigation_binding_can_be_added_and_remains_inactive() {
        let profile = ShortcutProfile::load_default().unwrap();
        let mut ui_state = SettingsWindowState::default();
        let mut output = None;
        let conflicting = ShortcutBinding::PointerGesture {
            context: PointerContext::Viewport3d,
            trigger: PointerGestureTrigger {
                button: ShortcutPointerButton::Primary,
                modifiers: ModifierMatch {
                    alt: true,
                    ..Default::default()
                },
            },
            action: PointerGestureAction::Pan3d,
        };

        process_change(
            &profile,
            &mut ui_state,
            &mut output,
            BindingChange::Set {
                index: None,
                binding: conflicting.clone(),
            },
        );

        let changed = output.unwrap();
        assert_eq!(changed.bindings.last(), Some(&conflicting));
        assert!(matches!(
            changed.diagnostics().last(),
            Some(ShortcutDiagnostic {
                reason: ShortcutDiagnosticReason::Conflict { .. },
                ..
            })
        ));
    }

    #[test]
    fn starting_another_capture_replaces_the_current_target() {
        let mut ui_state = SettingsWindowState {
            capture: Some(CaptureTarget::Invoke {
                action: ShortcutAction::SaveProject,
                index: Some(0),
            }),
            captured_modifiers: Some(ModifierMatch {
                ctrl: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let next = CaptureTarget::Invoke {
            action: ShortcutAction::Undo,
            index: Some(1),
        };

        start_capture(&mut ui_state, next.clone());

        assert_eq!(ui_state.capture, Some(next));
        assert!(ui_state.captured_modifiers.is_none());
    }
}
