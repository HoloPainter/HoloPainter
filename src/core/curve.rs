use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub const CURVE_MIN_POINT_SPAN: f32 = 1.0 / 255.0;
pub const MAX_CURVE_POINTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub(crate) point_count: u8,
    pub(crate) points: [CurvePoint; MAX_CURVE_POINTS],
}

impl Default for Curve {
    fn default() -> Self {
        let mut points = [CurvePoint::default(); MAX_CURVE_POINTS];
        points[1] = CurvePoint { x: 1.0, y: 1.0 };
        Self {
            point_count: 2,
            points,
        }
    }
}

impl Curve {
    pub fn points(&self) -> &[CurvePoint] {
        &self.points[..self.point_count as usize]
    }

    pub fn point(&self, index: usize) -> Option<CurvePoint> {
        self.points().get(index).copied()
    }

    pub fn set_point(&mut self, index: usize, point: CurvePoint) -> bool {
        let count = self.point_count as usize;
        if index >= count || !finite(point) {
            return false;
        }
        let normalized = CurvePoint {
            x: if index == 0 {
                0.0
            } else if index + 1 == count {
                1.0
            } else {
                point.x.clamp(
                    self.points[index - 1].x + CURVE_MIN_POINT_SPAN,
                    self.points[index + 1].x - CURVE_MIN_POINT_SPAN,
                )
            },
            y: point.y.clamp(0.0, 1.0),
        };
        if self.points[index] == normalized {
            return false;
        }
        self.points[index] = normalized;
        true
    }

    pub fn insert_point(&mut self, point: CurvePoint) -> Option<usize> {
        let count = self.point_count as usize;
        if count >= MAX_CURVE_POINTS || !finite(point) {
            return None;
        }
        let point = CurvePoint {
            x: point.x.clamp(0.0, 1.0),
            y: point.y.clamp(0.0, 1.0),
        };
        let insert_index = self.points()[1..count].partition_point(|p| p.x < point.x) + 1;
        let minimum = self.points[insert_index - 1].x + CURVE_MIN_POINT_SPAN;
        let maximum = self.points[insert_index].x - CURVE_MIN_POINT_SPAN;
        if minimum > maximum {
            return None;
        }
        self.points
            .copy_within(insert_index..count, insert_index + 1);
        self.points[insert_index] = CurvePoint {
            x: point.x.clamp(minimum, maximum),
            y: point.y,
        };
        self.point_count += 1;
        Some(insert_index)
    }

    pub fn remove_point(&mut self, index: usize) -> bool {
        let count = self.point_count as usize;
        if count <= 2 || index == 0 || index + 1 >= count {
            return false;
        }
        self.points.copy_within(index + 1..count, index);
        self.points[count - 1] = CurvePoint::default();
        self.point_count -= 1;
        true
    }

    pub fn evaluate(self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let points = self.points();
        if x <= points[0].x {
            return points[0].y;
        }
        if x >= points[points.len() - 1].x {
            return points[points.len() - 1].y;
        }
        let interval = points
            .windows(2)
            .position(|pair| x <= pair[1].x)
            .unwrap_or(points.len() - 2);
        if points.len() == 2 {
            return linear_value(points[0], points[1], x);
        }
        let mut widths = [0.0; MAX_CURVE_POINTS - 1];
        let mut slopes = [0.0; MAX_CURVE_POINTS - 1];
        for index in 0..points.len() - 1 {
            widths[index] = points[index + 1].x - points[index].x;
            slopes[index] = (points[index + 1].y - points[index].y) / widths[index];
        }
        let mut tangents = [0.0; MAX_CURVE_POINTS];
        tangents[0] = endpoint_tangent(widths[0], widths[1], slopes[0], slopes[1]);
        for index in 1..points.len() - 1 {
            let previous = slopes[index - 1];
            let next = slopes[index];
            tangents[index] = if previous == 0.0
                || next == 0.0
                || previous.is_sign_positive() != next.is_sign_positive()
            {
                0.0
            } else {
                let previous_width = widths[index - 1];
                let next_width = widths[index];
                let first_weight = 2.0 * next_width + previous_width;
                let second_weight = next_width + 2.0 * previous_width;
                (first_weight + second_weight) / (first_weight / previous + second_weight / next)
            };
        }
        let last = points.len() - 1;
        tangents[last] = endpoint_tangent(
            widths[last - 1],
            widths[last - 2],
            slopes[last - 1],
            slopes[last - 2],
        );
        let left = points[interval];
        let right = points[interval + 1];
        let width = right.x - left.x;
        let t = ((x - left.x) / width).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;
        ((2.0 * t3 - 3.0 * t2 + 1.0) * left.y
            + (t3 - 2.0 * t2 + t) * width * tangents[interval]
            + (-2.0 * t3 + 3.0 * t2) * right.y
            + (t3 - t2) * width * tangents[interval + 1])
            .clamp(0.0, 1.0)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let points = self.points();
        if !(2..=MAX_CURVE_POINTS).contains(&points.len()) {
            return Err("point count must be 2..=16");
        }
        if !points.iter().copied().all(finite) {
            return Err("points must be finite");
        }
        if !points
            .iter()
            .all(|p| (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y))
        {
            return Err("points must be inside 0..=1");
        }
        if points.first().unwrap().x != 0.0 || points.last().unwrap().x != 1.0 {
            return Err("endpoint x values must be 0 and 1");
        }
        if points
            .windows(2)
            .any(|pair| pair[1].x - pair[0].x < CURVE_MIN_POINT_SPAN)
        {
            return Err("point x values must be strictly increasing with minimum spacing");
        }
        Ok(())
    }

    pub fn try_normalized(self) -> Option<Self> {
        let count = self.point_count as usize;
        if !(2..=MAX_CURVE_POINTS).contains(&count)
            || !self.points[..count].iter().copied().all(finite)
        {
            return None;
        }
        let mut points = [CurvePoint::default(); MAX_CURVE_POINTS];
        points[..count].copy_from_slice(&self.points[..count]);
        for point in &mut points[..count] {
            point.x = point.x.clamp(0.0, 1.0);
            point.y = point.y.clamp(0.0, 1.0);
        }
        points[..count].sort_by(|left, right| left.x.total_cmp(&right.x));
        points[0].x = 0.0;
        points[count - 1].x = 1.0;
        for index in 1..count - 1 {
            let maximum = 1.0 - (count - 1 - index) as f32 * CURVE_MIN_POINT_SPAN;
            points[index].x = points[index]
                .x
                .clamp(points[index - 1].x + CURVE_MIN_POINT_SPAN, maximum);
        }
        Some(Self {
            point_count: count as u8,
            points,
        })
    }
}

impl Serialize for Curve {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.points().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Curve {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = Vec::<CurvePoint>::deserialize(deserializer)?;
        if !(2..=MAX_CURVE_POINTS).contains(&values.len()) {
            return Err(de::Error::custom("curve point count must be 2..=16"));
        }
        let mut curve = Self {
            point_count: values.len() as u8,
            points: [CurvePoint::default(); MAX_CURVE_POINTS],
        };
        curve.points[..values.len()].copy_from_slice(&values);
        curve.validate().map_err(de::Error::custom)?;
        Ok(curve)
    }
}

fn finite(point: CurvePoint) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn linear_value(left: CurvePoint, right: CurvePoint, x: f32) -> f32 {
    let amount = ((x - left.x) / (right.x - left.x)).clamp(0.0, 1.0);
    (left.y + (right.y - left.y) * amount).clamp(0.0, 1.0)
}

fn endpoint_tangent(
    first_width: f32,
    second_width: f32,
    first_slope: f32,
    second_slope: f32,
) -> f32 {
    let mut tangent = ((2.0 * first_width + second_width) * first_slope
        - first_width * second_slope)
        / (first_width + second_width);
    if tangent.is_sign_positive() != first_slope.is_sign_positive() {
        tangent = 0.0;
    } else if first_slope.is_sign_positive() != second_slope.is_sign_positive()
        && tangent.abs() > 3.0 * first_slope.abs()
    {
        tangent = 3.0 * first_slope;
    }
    tangent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_linear() {
        let curve = Curve::default();
        assert_eq!(
            curve.points(),
            &[CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 1.0, y: 1.0 }]
        );
        assert_eq!(curve.evaluate(0.0), 0.0);
        assert_eq!(curve.evaluate(0.5), 0.5);
        assert_eq!(curve.evaluate(1.0), 1.0);
    }

    #[test]
    fn endpoint_x_is_locked_and_y_is_editable() {
        let mut curve = Curve::default();
        assert!(curve.set_point(0, CurvePoint { x: 0.5, y: 0.2 }));
        assert_eq!(curve.point(0), Some(CurvePoint { x: 0.0, y: 0.2 }));
        assert!(curve.set_point(1, CurvePoint { x: 0.5, y: 0.8 }));
        assert_eq!(curve.point(1), Some(CurvePoint { x: 1.0, y: 0.8 }));
        assert!(!curve.remove_point(0));
        assert!(!curve.remove_point(1));
    }

    #[test]
    fn serde_rejects_invalid_ordering() {
        let invalid = "[(x: 0.0, y: 0.0), (x: 0.0, y: 0.5), (x: 1.0, y: 1.0)]";
        assert!(ron::from_str::<Curve>(invalid).is_err());
    }

    #[test]
    fn operations_respect_capacity_and_evaluation_stays_bounded() {
        let mut curve = Curve::default();
        for index in 1..MAX_CURVE_POINTS - 1 {
            assert!(
                curve
                    .insert_point(CurvePoint {
                        x: index as f32 / (MAX_CURVE_POINTS - 1) as f32,
                        y: if index % 2 == 0 { 0.0 } else { 1.0 },
                    })
                    .is_some()
            );
        }
        assert_eq!(curve.points().len(), MAX_CURVE_POINTS);
        assert!(curve.insert_point(CurvePoint { x: 0.5, y: 0.5 }).is_none());
        for input in [
            f32::NEG_INFINITY,
            -1.0,
            0.25,
            0.75,
            2.0,
            f32::INFINITY,
            f32::NAN,
        ] {
            let output = curve.evaluate(input);
            assert!(output.is_finite());
            assert!((0.0..=1.0).contains(&output));
        }
        assert!(curve.remove_point(1));
        assert_eq!(curve.points().len(), MAX_CURVE_POINTS - 1);
    }
}
