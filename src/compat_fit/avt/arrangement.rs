//! Initialize cloth from curved arrangement surfaces stored in a native AVT.

use super::{element_name, fields, find, list, Field, BYTES, LIST, MAX_ARRANGEMENTS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, f64::consts::TAU};

const ARC_SAMPLES: usize = 1024;
// The spiral tapers inward as the cloth wraps. This pitch was measured from
// the saved front, back, waist and sleeve cases and approximates the native wrap.
const SPIRAL_PITCH_MM: f64 = 10.75;

#[derive(Clone, Deserialize, Serialize)]
pub(in crate::compat_fit) struct CurvedPlacement {
    matrix: [f64; 16],
    radii: [f64; 2],
    height: f64,
    arc: f64,
    normalized_height: f64,
}

fn float(record: &[Field<'_>], key: &str) -> Option<f64> {
    let value = f32::from_le_bytes(find(record, key, 6)?.try_into().ok()?);
    value.is_finite().then_some(f64::from(value))
}

fn integer(record: &[Field<'_>], key: &str, kind: u8) -> Option<i32> {
    Some(i32::from_le_bytes(find(record, key, kind)?.try_into().ok()?))
}

fn matrix(record: &[Field<'_>]) -> Option<[f64; 16]> {
    let data: &[u8; 64] = find(record, "m4Matrix", 14)?.try_into().ok()?;
    let matrix = std::array::from_fn(|i| {
        f64::from(f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().expect("four-byte component")))
    });
    (matrix.iter().all(|v| v.is_finite())
        && [matrix[3], matrix[7], matrix[11], matrix[15]] == [0., 0., 0., 1.])
        .then_some(matrix)
}

pub(super) fn read(root: &[Field<'_>]) -> BTreeMap<String, CurvedPlacement> {
    let Some(pans) = find(root, "listPan", LIST).and_then(|value| list(value, MAX_ARRANGEMENTS).ok()) else {
        return BTreeMap::new();
    };
    let Some(arrangements) = find(root, "listArr", LIST).and_then(|value| list(value, MAX_ARRANGEMENTS).ok()) else {
        return BTreeMap::new();
    };
    let pans = pans.into_iter().filter_map(|value| {
        let record = fields(value).ok()?;
        let name = element_name(&record)?;
        (integer(&record, "enShapePolicyType", 4) == Some(0)).then_some((name, record))
    }).collect::<BTreeMap<_, _>>();

    arrangements.into_iter().filter_map(|value| {
        let record = fields(value).ok()?;
        let arc = float(&record, "fNormalizedArcLength")?;
        if integer(&record, "enType", 4) != Some(1)
            || integer(&record, "uiSpiralType", 5) != Some(1)
            || float(&record, "fAngle") != Some(0.)
            || find(&record, "bUpDownReverse", 1) != Some(&[0])
            || find(&record, "bLeftRightReverse", 1) != Some(&[0])
            || !matches!(arc, 0. | -0.5 | 0.5)
        {
            return None;
        }
        let name = std::str::from_utf8(find(&record, "qsNameUTF8", BYTES)?).ok()?;
        let pan = pans.get(find(&record, "qsPanNameUTF8", BYTES)?)?;
        let offset = float(&record, "fOffset")?;
        let placement = CurvedPlacement {
            matrix: matrix(pan)?,
            radii: [float(pan, "fRadiusX")? + offset, float(pan, "fRadiusZ")? + offset],
            height: float(pan, "fHeight")?,
            arc,
            normalized_height: float(&record, "fNormalizedHeight")?,
        };
        placement.valid().then_some((name.to_owned(), placement))
    }).collect()
}

impl CurvedPlacement {
    fn valid(&self) -> bool {
        self.matrix.iter().all(|v| v.abs() <= 100_000.)
            && self.radii.iter().all(|r| (1.0..=5000.0).contains(r))
            && (1.0..=5000.0).contains(&self.height)
            && self.normalized_height.abs() <= 2.
            && matches!(self.arc, 0. | -0.5 | 0.5)
    }

    fn wrap(&self, points: &[[f64; 2]], [left, bottom, right, top]: [f64; 4]) -> Vec<[f64; 3]> {
        let [rx, rz] = self.radii;
        let mut lengths = vec![0.0];
        let mut previous = [0., rz];
        for i in 1..=ARC_SAMPLES {
            let theta = i as f64 * TAU / ARC_SAMPLES as f64;
            let next = [rx * theta.sin(), rz * theta.cos()];
            lengths.push(lengths[i - 1] + (next[0] - previous[0]).hypot(next[1] - previous[1]));
            previous = next;
        }
        let circumference = lengths[ARC_SAMPLES];
        let center_x = (left + right) / 2.;
        let center_y = (bottom + top) / 2.;
        let center_s = self.arc * circumference;
        let base_y = self.normalized_height * self.height;
        points.iter().map(|&[u, v]| {
            let s = (u - center_x + center_s).rem_euclid(circumference);
            let segment = lengths.partition_point(|&length| length <= s).min(ARC_SAMPLES) - 1;
            let t = (s - lengths[segment]) / (lengths[segment + 1] - lengths[segment]);
            let theta = (segment as f64 + t) * TAU / ARC_SAMPLES as f64;
            let (sin, cos) = theta.sin_cos();
            let [x, z] = [rx * sin, rz * cos];
            let distance_from_left = u - left;
            let travel = if self.arc == 0. { distance_from_left } else { circumference * 0.5 - distance_from_left };
            let shrink = (travel / circumference * SPIRAL_PITCH_MM).max(0.);
            let taper = (1. - shrink / x.hypot(z)).max(0.);
            let y = v - center_y + base_y;
            let m = &self.matrix;
            std::array::from_fn(|i| m[12 + i] + m[i] * x * taper + m[4 + i] * y + m[8 + i] * z * taper)
        }).collect()
    }
}

pub(in crate::compat_fit) fn place(
    pattern: &Value, points: &[[f64; 2]], avatar: &Value,
) -> Option<Vec<[f64; 3]>> {
    let name = pattern["arrangement"].as_str()?;
    let settings = &pattern["saved_arrangement"];
    if settings.is_object() {
        let defaults = super::super::array(&avatar["arrangement_records"]).iter()
            .find(|row| row["ArrangementName"] == name)?;
        if settings.as_object()?.iter().any(|(key, value)| defaults[key] != *value) {
            return None;
        }
    }
    if pattern["layer"].as_i64().is_some_and(|layer| layer != 0) {
        return None;
    }
    let placement = CurvedPlacement::deserialize(avatar["native_arrangements"].get(name)?).ok()?;
    placement.valid().then_some(())?;
    let bounds = crate::compat_scene::pattern_rest_bounds(pattern).ok()?;
    if !bounds.iter().chain(points.iter().flatten()).all(|v| v.abs() < 1e7) {
        return None;
    }
    Some(placement.wrap(points, bounds))
}
