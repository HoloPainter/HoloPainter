use eframe::egui;

use crate::{
    application::{AppState, Command, EditorActionContext},
    core::{
        adjustment::{Adjustment, AdjustmentKind},
        surface::{LayerId, PaintSurfaceRole},
        surface_filter::SurfaceFilterDomain,
    },
    localization::Localization,
    ui::{
        adjustment_controls::{AdjustmentControlsState, draw_adjustment_controls},
        view_output::ViewOutput,
    },
};

const ADJUSTMENT_FILTER_WINDOW_ID: &str = "adjustment_filter_dialog";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdjustmentFilterTarget {
    layer_id: LayerId,
    document_generation: u64,
    role: PaintSurfaceRole,
}

pub(crate) struct AdjustmentFilterDialogState {
    target: Option<AdjustmentFilterTarget>,
    adjustment: Option<Adjustment>,
    controls: AdjustmentControlsState,
    preview_enabled: bool,
}

impl Default for AdjustmentFilterDialogState {
    fn default() -> Self {
        Self {
            target: None,
            adjustment: None,
            controls: AdjustmentControlsState::default(),
            preview_enabled: true,
        }
    }
}

impl AdjustmentFilterDialogState {
    pub(crate) fn open_for(
        &mut self,
        layer_id: LayerId,
        document_generation: u64,
        role: PaintSurfaceRole,
        kind: AdjustmentKind,
    ) {
        debug_assert!(kind.supports_destructive_filter_preview(role.into()));
        debug_assert_ne!(kind, AdjustmentKind::Invert);
        self.target = Some(AdjustmentFilterTarget {
            layer_id,
            document_generation,
            role,
        });
        self.adjustment = Some(kind.default_adjustment());
        self.controls.reset();
        self.preview_enabled = true;
    }

    fn close(&mut self) {
        self.target = None;
        self.adjustment = None;
        self.controls.reset();
        self.preview_enabled = true;
    }

    fn valid_target(&self, state: &AppState) -> Option<(LayerId, PaintSurfaceRole, String)> {
        let target = self.target?;
        if target.document_generation != state.document_generation()
            || state.active_layer_id() != target.layer_id
            || state.active_layer_target()
                != match target.role {
                    PaintSurfaceRole::Raster => crate::core::document::ActiveLayerTarget::Raster,
                    PaintSurfaceRole::LayerMask => {
                        crate::core::document::ActiveLayerTarget::LayerMask
                    }
                }
        {
            return None;
        }
        let document = state.document()?;
        let node = document.layer_tree.get(target.layer_id)?;
        let kind = self.adjustment.as_ref()?.kind();
        if !state.adjustment_filter_available(kind) {
            return None;
        }
        Some((target.layer_id, target.role, node.props.name.clone()))
    }
}

pub(crate) fn draw_adjustment_filter_dialog(
    ctx: &egui::Context,
    l10n: &Localization,
    state: &AppState,
    actions: EditorActionContext,
    dialog: &mut AdjustmentFilterDialogState,
) -> ViewOutput {
    let mut output = ViewOutput::default();
    let Some((layer_id, role, layer_name)) = dialog.valid_target(state) else {
        if let Some(target) = dialog.target
            && state.has_adjustment_filter_session()
        {
            output.push(Command::CancelAdjustmentFilterSession {
                layer_id: target.layer_id,
            });
        }
        dialog.close();
        return output;
    };
    let Some(mut adjustment) = dialog.adjustment.take() else {
        dialog.close();
        return output;
    };

    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    let mut adjustment_changed = false;
    let mut preview_changed = false;
    let kind = adjustment.kind();

    egui::Window::new(adjustment_kind_label(l10n, kind))
        .id(egui::Id::new(ADJUSTMENT_FILTER_WINDOW_ID))
        .collapsible(false)
        .resizable(kind == AdjustmentKind::Curves)
        .open(&mut open)
        .show(ctx, |ui| {
            let target_label = if role == PaintSurfaceRole::LayerMask {
                l10n.text("adjustment-target-mask")
            } else {
                l10n.text("adjustment-target-layer")
            };
            ui.label(format!("{target_label}: {layer_name}"));
            ui.separator();
            adjustment_changed = draw_adjustment_controls(
                ui,
                l10n,
                &mut adjustment,
                &mut dialog.controls,
                SurfaceFilterDomain::from(role),
            )
            .changed;
            ui.add_space(4.0);
            preview_changed = ui
                .checkbox(&mut dialog.preview_enabled, l10n.text("adjustment-preview"))
                .changed();
            let apply_enabled = actions.has_document
                && !actions.editing_blocked()
                && state.adjustment_filter_session_matches(layer_id, kind)
                && state.adjustment_filter_available(kind)
                && !adjustment.is_identity();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(l10n.text("dialog-cancel")).clicked() {
                    cancel = true;
                }
                if ui
                    .add_enabled(apply_enabled, egui::Button::new(l10n.text("action-apply")))
                    .clicked()
                {
                    apply = true;
                }
            });
        });

    dialog.adjustment = Some(adjustment.clone());
    if apply {
        dialog.close();
        output.push(Command::CommitAdjustmentFilterSession {
            layer_id,
            adjustment,
        });
        output.request_repaint();
    } else if cancel || !open {
        dialog.close();
        output.push(Command::CancelAdjustmentFilterSession { layer_id });
        output.request_repaint();
    } else if adjustment_changed || preview_changed {
        let preview = (dialog.preview_enabled && !adjustment.is_identity()).then_some(adjustment);
        output.push(Command::PreviewAdjustmentFilter {
            layer_id,
            adjustment: preview,
        });
        output.request_repaint();
    }
    output
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_filter_starts_from_kind_default() {
        let tree = crate::core::surface::LayerTree::new_default_raster();
        let layer_id = tree.default_raster_layer().expect("default raster");
        let mut dialog = AdjustmentFilterDialogState::default();

        dialog.open_for(
            layer_id,
            7,
            PaintSurfaceRole::Raster,
            AdjustmentKind::Levels,
        );

        assert_eq!(dialog.target.map(|target| target.layer_id), Some(layer_id));
        assert_eq!(
            dialog.adjustment,
            Some(AdjustmentKind::Levels.default_adjustment())
        );
        assert!(dialog.preview_enabled);
    }
}
