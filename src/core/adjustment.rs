use crate::core::surface_filter::SurfaceFilterDomain;

pub const ADJUSTMENT_LUT_SIZE: usize = 256;
pub const MAX_CURVE_POINTS: usize = 19;
pub const CURVE_MIN_POINT_SPAN: u8 = 1;
pub const GRADIENT_LOCATION_MAX: u16 = 4096;
pub const GRADIENT_MIDPOINT_MIN: u16 = 5;
pub const GRADIENT_MIDPOINT_MAX: u16 = 95;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdjustmentKind {
    BrightnessContrast,
    Levels,
    Curves,
    HueSaturation,
    Invert,
    GradientMap,
    UvMirror,
}

impl AdjustmentKind {
    pub const ALL: [Self; 7] = [
        Self::BrightnessContrast,
        Self::Levels,
        Self::Curves,
        Self::HueSaturation,
        Self::Invert,
        Self::GradientMap,
        Self::UvMirror,
    ];
    pub const DESTRUCTIVE_FILTERS: [Self; 5] = [
        Self::BrightnessContrast,
        Self::Levels,
        Self::Curves,
        Self::HueSaturation,
        Self::Invert,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::BrightnessContrast => "brightness_contrast",
            Self::Levels => "levels",
            Self::Curves => "curves",
            Self::HueSaturation => "hue_saturation",
            Self::Invert => "invert",
            Self::GradientMap => "gradient_map",
            Self::UvMirror => "uv_mirror",
        }
    }

    pub fn default_adjustment(self) -> Adjustment {
        match self {
            Self::BrightnessContrast => Adjustment::BrightnessContrast(Default::default()),
            Self::Levels => Adjustment::Levels(Default::default()),
            Self::Curves => Adjustment::Curves(Default::default()),
            Self::HueSaturation => Adjustment::HueSaturation(Default::default()),
            Self::Invert => Adjustment::Invert,
            Self::GradientMap => Adjustment::GradientMap(Default::default()),
            Self::UvMirror => Adjustment::UvMirror(Default::default()),
        }
    }

    pub fn supports_destructive_filter(self, domain: SurfaceFilterDomain) -> bool {
        match self {
            Self::BrightnessContrast | Self::Levels | Self::Curves | Self::Invert => true,
            Self::HueSaturation => domain == SurfaceFilterDomain::Color,
            Self::GradientMap | Self::UvMirror => false,
        }
    }

    pub fn supports_destructive_filter_preview(self, domain: SurfaceFilterDomain) -> bool {
        self != Self::Invert && self.supports_destructive_filter(domain)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Adjustment {
    BrightnessContrast(BrightnessContrastAdjustment),
    Levels(LevelsAdjustment),
    Curves(CurvesAdjustment),
    HueSaturation(HueSaturationAdjustment),
    Invert,
    GradientMap(GradientMapAdjustment),
    UvMirror(UvMirrorAdjustment),
}

impl Adjustment {
    pub fn kind(&self) -> AdjustmentKind {
        match self {
            Self::BrightnessContrast(_) => AdjustmentKind::BrightnessContrast,
            Self::Levels(_) => AdjustmentKind::Levels,
            Self::Curves(_) => AdjustmentKind::Curves,
            Self::HueSaturation(_) => AdjustmentKind::HueSaturation,
            Self::Invert => AdjustmentKind::Invert,
            Self::GradientMap(_) => AdjustmentKind::GradientMap,
            Self::UvMirror(_) => AdjustmentKind::UvMirror,
        }
    }

    pub fn try_normalized(self) -> Option<Self> {
        match self {
            Self::BrightnessContrast(value) => Some(Self::BrightnessContrast(value.normalized())),
            Self::Levels(value) => Some(Self::Levels(value.normalized())),
            Self::Curves(value) => value.try_normalized().map(Self::Curves),
            Self::HueSaturation(value) => Some(Self::HueSaturation(value.normalized())),
            Self::Invert => Some(Self::Invert),
            Self::GradientMap(value) => value.try_normalized().map(Self::GradientMap),
            Self::UvMirror(value) => value.try_normalized().map(Self::UvMirror),
        }
    }

    pub fn try_normalized_for_domain(self, domain: SurfaceFilterDomain) -> Option<Self> {
        if !self.kind().supports_destructive_filter(domain) {
            return None;
        }
        let normalized = self.try_normalized()?;
        match (domain, normalized) {
            (SurfaceFilterDomain::Scalar, Self::Levels(mut levels)) => {
                levels.red = LevelsChannel::default();
                levels.green = LevelsChannel::default();
                levels.blue = LevelsChannel::default();
                Some(Self::Levels(levels))
            }
            (SurfaceFilterDomain::Scalar, Self::Curves(mut curves)) => {
                curves.red = CurveChannel::default();
                curves.green = CurveChannel::default();
                curves.blue = CurveChannel::default();
                Some(Self::Curves(curves))
            }
            (_, adjustment) => Some(adjustment),
        }
    }

    pub fn is_identity(&self) -> bool {
        match self {
            Self::BrightnessContrast(value) => *value == BrightnessContrastAdjustment::default(),
            Self::Levels(value) => *value == LevelsAdjustment::default(),
            Self::Curves(value) => *value == CurvesAdjustment::default(),
            Self::HueSaturation(value) => *value == HueSaturationAdjustment::default(),
            Self::Invert | Self::GradientMap(_) | Self::UvMirror(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UvMirrorAxis {
    X,
    Y,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UvMirrorDirection {
    PositiveToNegative,
    NegativeToPositive,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvMirrorAdjustment {
    pub axis: UvMirrorAxis,
    pub position: f32,
    pub direction: UvMirrorDirection,
}
impl Default for UvMirrorAdjustment {
    fn default() -> Self {
        Self {
            axis: UvMirrorAxis::X,
            position: 0.5,
            direction: UvMirrorDirection::PositiveToNegative,
        }
    }
}
impl UvMirrorAdjustment {
    fn try_normalized(self) -> Option<Self> {
        self.position.is_finite().then_some(Self {
            position: self.position.clamp(0.0, 1.0),
            ..self
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BrightnessContrastAdjustment {
    pub brightness: i16,
    pub contrast: i16,
}
impl BrightnessContrastAdjustment {
    fn normalized(self) -> Self {
        Self {
            brightness: self.brightness.clamp(-150, 150),
            contrast: self.contrast.clamp(-50, 100),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelsAdjustment {
    pub master: LevelsChannel,
    pub red: LevelsChannel,
    pub green: LevelsChannel,
    pub blue: LevelsChannel,
}
impl Default for LevelsAdjustment {
    fn default() -> Self {
        Self {
            master: Default::default(),
            red: Default::default(),
            green: Default::default(),
            blue: Default::default(),
        }
    }
}
impl LevelsAdjustment {
    fn normalized(self) -> Self {
        Self {
            master: self.master.normalized(),
            red: self.red.normalized(),
            green: self.green.normalized(),
            blue: self.blue.normalized(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelsChannel {
    pub input_black: u8,
    pub input_white: u8,
    pub gamma: f32,
    pub output_black: u8,
    pub output_white: u8,
}
impl Default for LevelsChannel {
    fn default() -> Self {
        Self {
            input_black: 0,
            input_white: 255,
            gamma: 1.0,
            output_black: 0,
            output_white: 255,
        }
    }
}
impl LevelsChannel {
    fn normalized(self) -> Self {
        let input_black = self.input_black.min(253);
        Self {
            input_black,
            input_white: self.input_white.clamp(2, 255).max(input_black + 1),
            gamma: if self.gamma.is_finite() {
                self.gamma.clamp(0.1, 9.99)
            } else {
                1.0
            },
            output_black: self.output_black,
            output_white: self.output_white,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurvePoint {
    pub input: u8,
    pub output: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurveChannel {
    pub(crate) point_count: u8,
    pub(crate) points: [CurvePoint; MAX_CURVE_POINTS],
}
impl Default for CurveChannel {
    fn default() -> Self {
        let mut points = [CurvePoint::default(); MAX_CURVE_POINTS];
        points[1] = CurvePoint {
            input: 255,
            output: 255,
        };
        Self {
            point_count: 2,
            points,
        }
    }
}
impl CurveChannel {
    pub fn points(&self) -> &[CurvePoint] {
        &self.points[..self.point_count as usize]
    }
    pub fn point(&self, index: usize) -> Option<CurvePoint> {
        self.points().get(index).copied()
    }
    pub fn from_points(values: &[CurvePoint]) -> Option<Self> {
        if !(2..=MAX_CURVE_POINTS).contains(&values.len()) {
            return None;
        }
        let mut curve = Self {
            point_count: values.len() as u8,
            points: [CurvePoint::default(); MAX_CURVE_POINTS],
        };
        curve.points[..values.len()].copy_from_slice(values);
        curve.try_normalized()
    }
    pub fn set_point(&mut self, index: usize, point: CurvePoint) -> bool {
        let count = self.point_count as usize;
        if index >= count {
            return false;
        }
        let minimum = index
            .checked_sub(1)
            .map_or(0, |previous| self.points[previous].input.saturating_add(1));
        let maximum = if index + 1 < count {
            self.points[index + 1].input.saturating_sub(1)
        } else {
            255
        };
        let input = point.input.clamp(minimum, maximum);
        let normalized = CurvePoint { input, ..point };
        if self.points[index] == normalized {
            return false;
        }
        self.points[index] = normalized;
        true
    }
    pub fn insert_point(&mut self, point: CurvePoint) -> Option<usize> {
        let count = self.point_count as usize;
        if count >= MAX_CURVE_POINTS {
            return None;
        }
        let index = self
            .points()
            .partition_point(|candidate| candidate.input < point.input);
        if self
            .points()
            .get(index)
            .is_some_and(|candidate| candidate.input == point.input)
        {
            return None;
        }
        self.points.copy_within(index..count, index + 1);
        self.points[index] = point;
        self.point_count += 1;
        Some(index)
    }
    pub fn remove_point(&mut self, index: usize) -> bool {
        let count = self.point_count as usize;
        if count <= 2 || index >= count {
            return false;
        }
        self.points.copy_within(index + 1..count, index);
        self.points[count - 1] = CurvePoint::default();
        self.point_count -= 1;
        true
    }
    pub fn evaluate(&self, input: f32) -> f32 {
        let x = if input.is_finite() {
            input.clamp(0.0, 1.0) * 255.0
        } else {
            0.0
        };
        let points = self.points();
        let mut inputs = [0.0; MAX_CURVE_POINTS];
        let mut outputs = [0.0; MAX_CURVE_POINTS];
        for (index, point) in points.iter().enumerate() {
            inputs[index] = point.input as f32;
            outputs[index] = point.output as f32;
        }
        crate::core::adjustment_evaluator::natural_cubic_spline(
            &inputs[..points.len()],
            &outputs[..points.len()],
            x,
        )
        .clamp(0.0, 255.0)
            / 255.0
    }
    fn try_normalized(mut self) -> Option<Self> {
        let count = self.point_count as usize;
        if !(2..=MAX_CURVE_POINTS).contains(&count) {
            return None;
        }
        self.points[..count].sort_by_key(|point| point.input);
        if self.points[..count]
            .windows(2)
            .any(|pair| pair[0].input >= pair[1].input)
        {
            return None;
        }
        self.points[count..].fill(CurvePoint::default());
        Some(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurvesAdjustment {
    pub master: CurveChannel,
    pub red: CurveChannel,
    pub green: CurveChannel,
    pub blue: CurveChannel,
}
impl Default for CurvesAdjustment {
    fn default() -> Self {
        Self {
            master: Default::default(),
            red: Default::default(),
            green: Default::default(),
            blue: Default::default(),
        }
    }
}
impl CurvesAdjustment {
    pub fn evaluate_rgb(&self, color: [f32; 3]) -> [f32; 3] {
        let channels = [
            self.red.evaluate(color[0]),
            self.green.evaluate(color[1]),
            self.blue.evaluate(color[2]),
        ];
        channels.map(|value| self.master.evaluate(value))
    }
    fn try_normalized(self) -> Option<Self> {
        Some(Self {
            master: self.master.try_normalized()?,
            red: self.red.try_normalized()?,
            green: self.green.try_normalized()?,
            blue: self.blue.try_normalized()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GradientStop {
    pub location: u16,
    pub midpoint: u16,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GradientMapAdjustment {
    pub stops: Vec<GradientStop>,
    pub reverse: bool,
    /// Preserves the PSD dither flag. The renderer's deterministic noise pattern is
    /// implementation-defined because Adobe does not publish Photoshop's algorithm.
    pub dither: bool,
}
impl Default for GradientMapAdjustment {
    fn default() -> Self {
        Self {
            stops: vec![
                GradientStop {
                    location: 0,
                    midpoint: 50,
                    color: [0; 3],
                },
                GradientStop {
                    location: GRADIENT_LOCATION_MAX,
                    midpoint: 50,
                    color: [255; 3],
                },
            ],
            reverse: false,
            dither: false,
        }
    }
}
impl GradientMapAdjustment {
    pub fn stops(&self) -> &[GradientStop] {
        &self.stops
    }
    pub fn stop(&self, index: usize) -> Option<GradientStop> {
        self.stops.get(index).copied()
    }
    pub fn set_stop(&mut self, index: usize, stop: GradientStop) -> bool {
        if index >= self.stops.len() {
            return false;
        }
        let minimum = index
            .checked_sub(1)
            .map_or(0, |i| self.stops[i].location.saturating_add(1));
        let maximum = self
            .stops
            .get(index + 1)
            .map_or(GRADIENT_LOCATION_MAX, |stop| {
                stop.location.saturating_sub(1)
            });
        let normalized = GradientStop {
            location: stop.location.clamp(minimum, maximum),
            midpoint: stop
                .midpoint
                .clamp(GRADIENT_MIDPOINT_MIN, GRADIENT_MIDPOINT_MAX),
            color: stop.color,
        };
        if self.stops[index] == normalized {
            return false;
        }
        self.stops[index] = normalized;
        true
    }
    pub fn insert_stop(&mut self, stop: GradientStop) -> Option<usize> {
        let location = stop.location.min(GRADIENT_LOCATION_MAX);
        let index = self
            .stops
            .partition_point(|existing| existing.location < location);
        let minimum = index
            .checked_sub(1)
            .map_or(0, |i| self.stops[i].location.saturating_add(1));
        let maximum = self.stops.get(index).map_or(GRADIENT_LOCATION_MAX, |stop| {
            stop.location.saturating_sub(1)
        });
        if minimum > maximum {
            return None;
        }
        self.stops.insert(
            index,
            GradientStop {
                location: location.clamp(minimum, maximum),
                midpoint: stop
                    .midpoint
                    .clamp(GRADIENT_MIDPOINT_MIN, GRADIENT_MIDPOINT_MAX),
                color: stop.color,
            },
        );
        Some(index)
    }
    pub fn remove_stop(&mut self, index: usize) -> bool {
        if self.stops.len() <= 2 || index >= self.stops.len() {
            return false;
        }
        self.stops.remove(index);
        true
    }
    /// Evaluates the supported PSD subset: classic linear RGB color stops with
    /// per-segment midpoint remapping. Opacity stops and other interpolation
    /// modes are intentionally outside the current model.
    pub fn evaluate(&self, position: f32) -> [f32; 3] {
        let position = if self.reverse {
            1.0 - position
        } else {
            position
        };
        let location = position.clamp(0.0, 1.0) * GRADIENT_LOCATION_MAX as f32;
        if location <= self.stops[0].location as f32 {
            return color_to_float(self.stops[0].color);
        }
        if location >= self.stops[self.stops.len() - 1].location as f32 {
            return color_to_float(self.stops[self.stops.len() - 1].color);
        }
        let right_index = self
            .stops
            .partition_point(|stop| (stop.location as f32) < location)
            .min(self.stops.len() - 1);
        let left = self.stops[right_index - 1];
        let right = self.stops[right_index];
        let raw =
            (location - left.location as f32) / (right.location as f32 - left.location as f32);
        let midpoint = left.midpoint as f32 / 100.0;
        let amount = if raw <= midpoint {
            0.5 * raw / midpoint
        } else {
            0.5 + 0.5 * (raw - midpoint) / (1.0 - midpoint)
        };
        let a = color_to_float(left.color);
        let b = color_to_float(right.color);
        [
            a[0] + (b[0] - a[0]) * amount,
            a[1] + (b[1] - a[1]) * amount,
            a[2] + (b[2] - a[2]) * amount,
        ]
    }
    fn try_normalized(mut self) -> Option<Self> {
        if self.stops.len() < 2 || self.stops.len() > GRADIENT_LOCATION_MAX as usize + 1 {
            return None;
        }
        self.stops.sort_by_key(|stop| stop.location);
        let len = self.stops.len();
        for (index, stop) in self.stops.iter_mut().enumerate() {
            stop.location = stop.location.clamp(
                index as u16,
                GRADIENT_LOCATION_MAX - (len - 1 - index) as u16,
            );
            stop.midpoint = stop
                .midpoint
                .clamp(GRADIENT_MIDPOINT_MIN, GRADIENT_MIDPOINT_MAX);
        }
        Some(self)
    }
}
fn color_to_float(color: [u8; 3]) -> [f32; 3] {
    color.map(|channel| channel as f32 / 255.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HueSaturationAdjustment {
    pub hue: i16,
    pub saturation: i16,
    pub lightness: i16,
}
impl HueSaturationAdjustment {
    fn normalized(self) -> Self {
        Self {
            hue: self.hue.clamp(-180, 180),
            saturation: self.saturation.clamp(-100, 100),
            lightness: self.lightness.clamp(-100, 100),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adjustment_curve_supports_nineteen_psd_points() {
        let points: Vec<_> = (0..MAX_CURVE_POINTS)
            .map(|i| CurvePoint {
                input: (i * 255 / 18) as u8,
                output: (i * 255 / 18) as u8,
            })
            .collect();
        let curve = CurveChannel::from_points(&points).unwrap();
        assert_eq!(curve.points().len(), 19);
        assert!((curve.evaluate(0.5) - 0.5).abs() < 0.01);
    }

    #[test]
    fn adjustment_curve_accepts_arbitrary_endpoints_and_clamps_outside_domain() {
        let curve = CurveChannel::from_points(&[
            CurvePoint {
                input: 32,
                output: 10,
            },
            CurvePoint {
                input: 128,
                output: 170,
            },
            CurvePoint {
                input: 220,
                output: 240,
            },
        ])
        .unwrap();
        assert_eq!(curve.points()[0].input, 32);
        assert_eq!(curve.points()[2].input, 220);
        assert!((curve.evaluate(0.0) - 10.0 / 255.0).abs() < 1.0e-6);
        assert!((curve.evaluate(1.0) - 240.0 / 255.0).abs() < 1.0e-6);
    }
    #[test]
    fn gradient_midpoint_controls_half_color_location() {
        let mut gradient = GradientMapAdjustment::default();
        gradient.stops[0].midpoint = 25;
        assert!((gradient.evaluate(0.25)[0] - 0.5).abs() < 1.0e-6);
    }
}
