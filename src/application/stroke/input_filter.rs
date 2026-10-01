use glam::Vec2;

use crate::core::stroke_preset::{INPUT_FILTER_CAP, StrokeInputFilter};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) struct RawStrokeInput {
    pub screen_px: Vec2,
    pub pressure: f32,
    pub time_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) struct FilteredStrokeInput {
    pub screen_px: Vec2,
    pub pressure: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(in crate::application) struct StrokeInputFilterRuntime {
    x: RingF32,
    y: RingF32,
    pressure: RingF32,
}

impl StrokeInputFilterRuntime {
    pub(in crate::application) fn new() -> Self {
        Self::default()
    }

    pub(in crate::application) fn update(
        &mut self,
        filter: &StrokeInputFilter,
        raw: RawStrokeInput,
    ) -> FilteredStrokeInput {
        let filter = filter.sanitized();
        let _ = raw.time_s;

        let x = if filter.stabilization == 0 {
            raw.screen_px.x
        } else {
            self.x.filter(raw.screen_px.x, filter.stabilization, true)
        };
        let y = if filter.stabilization == 0 {
            raw.screen_px.y
        } else {
            self.y.filter(raw.screen_px.y, filter.stabilization, true)
        };

        let raw_pressure = raw.pressure.clamp(0.0, 1.0);
        let pressure = if filter.pressure_filter == 0 {
            raw_pressure
        } else {
            self.pressure
                .filter(raw_pressure, filter.pressure_filter, true)
        };

        FilteredStrokeInput {
            screen_px: Vec2::new(x, y),
            pressure,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RingF32 {
    values: [f32; INPUT_FILTER_CAP],
    len: usize,
    next: usize,
}

impl Default for RingF32 {
    fn default() -> Self {
        Self {
            values: [0.0; INPUT_FILTER_CAP],
            len: 0,
            next: 0,
        }
    }
}

impl RingF32 {
    fn filter(&mut self, value: f32, target_len: usize, store_filtered: bool) -> f32 {
        let target_len = target_len.clamp(1, INPUT_FILTER_CAP);
        let mut sum = value;
        let mut used = 1.0;
        for back in 0..(target_len - 1) {
            if back >= self.len {
                break;
            }
            let idx = (self.next + INPUT_FILTER_CAP - 1 - back) % INPUT_FILTER_CAP;
            sum += self.values[idx];
            used += 1.0;
        }

        let filtered = sum / used;
        self.values[self.next] = if store_filtered { filtered } else { value };
        self.next = (self.next + 1) % INPUT_FILTER_CAP;
        self.len = (self.len + 1).min(INPUT_FILTER_CAP);
        filtered
    }
}
