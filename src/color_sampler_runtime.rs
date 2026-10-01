use std::collections::HashSet;

use crate::{
    core::{surface::LayerId, tool::ColorSampleSource},
    renderer::{
        ColorSampleIntent, ColorSamplePreview, ColorSampleRequest, ColorSampleTarget,
        ColorSampleUiRequest, CompletedColorSample,
    },
};

const SAMPLE_INTERVAL_SECONDS: f64 = 1.0 / 30.0;

#[derive(Debug, Clone, Copy)]
struct LastDispatch {
    intent: ColorSampleIntent,
    source: ColorSampleSource,
    view: crate::renderer::ColorSampleView,
    active_layer_id: Option<LayerId>,
    time_seconds: f64,
}

#[derive(Debug, Default)]
pub(crate) struct ColorSamplerRuntime {
    next_request_id: u64,
    latest_preview_request_id: Option<u64>,
    latest_apply_request_id: Option<u64>,
    latest_applied_request_id: Option<u64>,
    preview: Option<ColorSamplePreview>,
    last_dispatch: Option<LastDispatch>,
    pending: HashSet<u64>,
    has_valid_context: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ColorSampleCompletionOutcome {
    pub(crate) preview_changed: bool,
    pub(crate) current_color: Option<[f32; 3]>,
    pub(crate) error: Option<String>,
}

impl ColorSamplerRuntime {
    pub(crate) fn begin_frame(&mut self) {
        self.has_valid_context = false;
    }

    pub(crate) fn preview(&self) -> Option<ColorSamplePreview> {
        self.preview
    }

    pub(crate) fn prepare_request(
        &mut self,
        request: ColorSampleUiRequest,
        document_generation: u64,
        time_seconds: f64,
    ) -> Option<ColorSampleRequest> {
        if !target_matches_source(request.source, request.target) {
            return None;
        }
        self.has_valid_context = true;
        let should_dispatch = self.last_dispatch.is_none_or(|last| {
            last.source != request.source
                || last.view != request.anchor.view
                || last.active_layer_id != request.active_layer_id
                || (request.intent == ColorSampleIntent::Apply
                    && last.intent != ColorSampleIntent::Apply)
                || time_seconds - last.time_seconds >= SAMPLE_INTERVAL_SECONDS
        });
        if !should_dispatch {
            return None;
        }

        let id = self.next_request_id.max(1);
        self.next_request_id = id.wrapping_add(1).max(1);
        let request = ColorSampleRequest {
            id,
            intent: request.intent,
            source: request.source,
            anchor: request.anchor,
            target: request.target,
            document_generation,
            active_layer_id: request.active_layer_id,
        };
        match request.intent {
            ColorSampleIntent::Preview => self.latest_preview_request_id = Some(id),
            ColorSampleIntent::Apply => {
                self.latest_preview_request_id = None;
                self.latest_apply_request_id = Some(id);
            }
        }
        self.last_dispatch = Some(LastDispatch {
            intent: request.intent,
            source: request.source,
            view: request.anchor.view,
            active_layer_id: request.active_layer_id,
            time_seconds,
        });
        self.pending.insert(id);
        Some(request)
    }

    pub(crate) fn finish_frame(&mut self) -> bool {
        if self.has_valid_context {
            return false;
        }
        self.last_dispatch = None;
        self.latest_preview_request_id = None;
        self.preview.take().is_some()
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn cancel_request(&mut self, id: u64) {
        self.pending.remove(&id);
        if self.latest_preview_request_id == Some(id) {
            self.latest_preview_request_id = None;
        }
        if self.latest_apply_request_id == Some(id) {
            self.latest_apply_request_id = None;
        }
        self.last_dispatch = None;
    }

    pub(crate) fn cancel_pending(&mut self) {
        self.pending.clear();
        self.latest_preview_request_id = None;
        self.latest_apply_request_id = None;
        self.last_dispatch = None;
    }

    pub(crate) fn complete(
        &mut self,
        completed: CompletedColorSample,
        current_document_generation: u64,
        current_source: ColorSampleSource,
        current_active_layer_id: LayerId,
    ) -> ColorSampleCompletionOutcome {
        self.pending.remove(&completed.request.id);
        if completed.request.document_generation != current_document_generation
            || completed.request.source != current_source
            || (completed.request.source == ColorSampleSource::CurrentLayer
                && completed.request.active_layer_id != Some(current_active_layer_id))
        {
            return ColorSampleCompletionOutcome::default();
        }
        match completed.request.intent {
            ColorSampleIntent::Preview
                if self.latest_preview_request_id != Some(completed.request.id) =>
            {
                return ColorSampleCompletionOutcome::default();
            }
            ColorSampleIntent::Apply
                if self.latest_apply_request_id != Some(completed.request.id)
                    || self
                        .latest_applied_request_id
                        .is_some_and(|id| id >= completed.request.id) =>
            {
                return ColorSampleCompletionOutcome::default();
            }
            ColorSampleIntent::Preview | ColorSampleIntent::Apply => {}
        }

        let rgba = match completed.result {
            Ok(rgba) => rgba,
            Err(error) => {
                if self.last_dispatch.is_some_and(|last| {
                    last.source == completed.request.source
                        && last.view == completed.request.anchor.view
                        && last.active_layer_id == completed.request.active_layer_id
                }) {
                    self.last_dispatch = None;
                }
                return ColorSampleCompletionOutcome {
                    error: (completed.request.intent == ColorSampleIntent::Apply).then_some(error),
                    ..ColorSampleCompletionOutcome::default()
                };
            }
        };
        let Some(rgb) = sampled_rgb(completed.request.source, rgba) else {
            return ColorSampleCompletionOutcome {
                preview_changed: self.preview.take().is_some(),
                error: (completed.request.intent == ColorSampleIntent::Apply)
                    .then_some("transparent pixel".to_owned()),
                ..ColorSampleCompletionOutcome::default()
            };
        };
        let preview = ColorSamplePreview {
            rgb,
            source: completed.request.source,
            anchor: completed.request.anchor,
            document_generation: completed.request.document_generation,
            active_layer_id: completed.request.active_layer_id,
        };

        match completed.request.intent {
            ColorSampleIntent::Preview => {
                let changed = self.preview != Some(preview);
                self.preview = Some(preview);
                ColorSampleCompletionOutcome {
                    preview_changed: changed,
                    ..ColorSampleCompletionOutcome::default()
                }
            }
            ColorSampleIntent::Apply => {
                self.latest_applied_request_id = Some(completed.request.id);
                let may_update_preview = self
                    .latest_preview_request_id
                    .is_none_or(|id| id <= completed.request.id);
                let changed = may_update_preview && self.preview != Some(preview);
                if may_update_preview {
                    self.preview = Some(preview);
                }
                ColorSampleCompletionOutcome {
                    preview_changed: changed,
                    current_color: Some(rgb.map(|channel| channel as f32 / 255.0)),
                    error: None,
                }
            }
        }
    }
}

fn target_matches_source(source: ColorSampleSource, target: ColorSampleTarget) -> bool {
    matches!(
        (source, target),
        (
            ColorSampleSource::View,
            ColorSampleTarget::ViewOutput { .. }
        ) | (
            ColorSampleSource::CompositeTexture,
            ColorSampleTarget::CompositeTexture { .. }
        ) | (
            ColorSampleSource::CurrentLayer,
            ColorSampleTarget::SurfaceTexture { .. }
        )
    )
}

fn sampled_rgb(source: ColorSampleSource, rgba: [u8; 4]) -> Option<[u8; 3]> {
    match source {
        ColorSampleSource::View => Some([rgba[0], rgba[1], rgba[2]]),
        ColorSampleSource::CompositeTexture | ColorSampleSource::CurrentLayer => {
            let alpha = u16::from(rgba[3]);
            if alpha == 0 {
                return None;
            }
            Some(std::array::from_fn(|index| {
                ((u16::from(rgba[index]) * 255 + alpha / 2) / alpha).min(255) as u8
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{ColorSampleAnchor, ColorSampleView};
    use slotmap::Key;

    fn view_request(intent: ColorSampleIntent, position: [u32; 2]) -> ColorSampleUiRequest {
        ColorSampleUiRequest {
            intent,
            source: ColorSampleSource::View,
            anchor: ColorSampleAnchor {
                view: ColorSampleView::Uv,
                position,
            },
            target: ColorSampleTarget::ViewOutput {
                view: ColorSampleView::Uv,
                position,
            },
            active_layer_id: None,
        }
    }

    #[test]
    fn view_samples_use_the_rendered_rgb_directly() {
        assert_eq!(
            sampled_rgb(ColorSampleSource::View, [64, 32, 16, 128]),
            Some([64, 32, 16])
        );
    }

    #[test]
    fn texture_samples_are_unpremultiplied() {
        assert_eq!(
            sampled_rgb(ColorSampleSource::CompositeTexture, [64, 32, 16, 128]),
            Some([128, 64, 32])
        );
        assert_eq!(
            sampled_rgb(ColorSampleSource::CurrentLayer, [0, 0, 0, 0]),
            None
        );
    }

    #[test]
    fn stale_preview_completion_is_ignored() {
        let mut runtime = ColorSamplerRuntime::default();
        let first = runtime
            .prepare_request(view_request(ColorSampleIntent::Preview, [1, 1]), 4, 0.0)
            .unwrap();
        let second = runtime
            .prepare_request(view_request(ColorSampleIntent::Preview, [2, 1]), 4, 0.04)
            .unwrap();
        let layer = LayerId::null();

        runtime.complete(
            CompletedColorSample {
                request: second,
                result: Ok([2, 3, 4, 255]),
            },
            4,
            ColorSampleSource::View,
            layer,
        );
        runtime.complete(
            CompletedColorSample {
                request: first,
                result: Ok([9, 9, 9, 255]),
            },
            4,
            ColorSampleSource::View,
            layer,
        );

        assert_eq!(runtime.preview().unwrap().rgb, [2, 3, 4]);
    }

    #[test]
    fn apply_uses_the_same_rgb_as_preview() {
        let mut runtime = ColorSamplerRuntime::default();
        let request = runtime
            .prepare_request(view_request(ColorSampleIntent::Apply, [3, 5]), 9, 0.0)
            .unwrap();
        let outcome = runtime.complete(
            CompletedColorSample {
                request,
                result: Ok([10, 20, 30, 255]),
            },
            9,
            ColorSampleSource::View,
            LayerId::null(),
        );

        assert_eq!(runtime.preview().unwrap().rgb, [10, 20, 30]);
        assert_eq!(
            outcome.current_color,
            Some([10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0])
        );
    }

    #[test]
    fn source_change_rejects_old_completion() {
        let mut runtime = ColorSamplerRuntime::default();
        let request = runtime
            .prepare_request(view_request(ColorSampleIntent::Apply, [3, 5]), 9, 0.0)
            .unwrap();
        let outcome = runtime.complete(
            CompletedColorSample {
                request,
                result: Ok([10, 20, 30, 255]),
            },
            9,
            ColorSampleSource::CompositeTexture,
            LayerId::null(),
        );
        assert_eq!(outcome.current_color, None);
    }
}
