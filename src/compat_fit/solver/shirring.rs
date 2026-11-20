//! Conforming refinement along selected pattern boundaries.

use super::{boolean, distance2, edge, Edge, Point2, MAX_SIMULATION_VERTICES};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn distance_to_segment(point: Point2, [a, b]: [Point2; 2]) -> f64 {
    let length = distance2(a, b);
    if length <= 1e-12 {
        return distance2(point, a);
    }
    let ab = [b[0] - a[0], b[1] - a[1]];
    let t = (((point[0] - a[0]) * ab[0] + (point[1] - a[1]) * ab[1]) / length).clamp(0.0, 1.0);
    distance2(point, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

fn subdivide(t: [usize; 3], mid: [Option<usize>; 3], output: &mut Vec<[usize; 3]>) {
    match mid {
        [None, None, None] => output.push(t),
        [Some(ab), Some(bc), Some(ca)] => {
            let [a, b, c] = t;
            output.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        _ => {
            let two = mid.iter().flatten().count() == 2;
            let i = mid.iter().position(|slot| slot.is_none() == two).unwrap();
            let [a, b, c] = [t[i], t[(i + 1) % 3], t[(i + 2) % 3]];
            if two {
                let bc = mid[(i + 1) % 3].unwrap();
                let ca = mid[(i + 2) % 3].unwrap();
                output.extend([[c, ca, bc], [a, b, ca], [b, bc, ca]]);
            } else {
                let ab = mid[i].unwrap();
                output.extend([[a, ab, c], [ab, b, c]]);
            }
        }
    }
}

pub(super) fn refine(
    pattern: &Value,
    points: &mut Vec<Point2>,
    triangles: &mut Vec<[usize; 3]>,
    sides: &mut [Vec<usize>],
    distance: f64,
) -> Result<Vec<Edge>, String> {
    let props = &pattern["line_properties"];
    if props.is_null() || !distance.is_finite() {
        return Ok(Vec::new());
    }
    let all = boolean(&props["-1"]["shirring"], "on", false);
    let guides: Vec<_> = sides
        .iter()
        .enumerate()
        .filter(|(index, _)| boolean(&props[index.to_string()]["shirring"], "on", all))
        .flat_map(|(_, vertices)| {
            vertices
                .windows(2)
                .map(|pair| [points[pair[0]], points[pair[1]]])
        })
        .collect();
    if guides.is_empty() {
        return Ok(Vec::new());
    }

    let band2 = distance * distance;
    let boundary_spacing2 = guides
        .iter()
        .map(|&[a, b]| distance2(a, b))
        .fold(0.0, f64::max);
    let spacing2 = band2.min(boundary_spacing2) * 0.25;
    let mut added = Vec::new();
    for _ in 0..2 {
        let mut edges = BTreeSet::<Edge>::new();
        for &[a, b, c] in triangles.iter() {
            edges.extend([edge(a, b), edge(b, c), edge(c, a)]);
        }
        let mut midpoints = BTreeMap::new();
        for (a, b) in edges {
            let middle = std::array::from_fn(|i| (points[a][i] + points[b][i]) * 0.5);
            if distance2(points[a], points[b]) <= spacing2
                || !guides
                    .iter()
                    .any(|&segment| distance_to_segment(middle, segment) <= band2)
            {
                continue;
            }
            if points.len() >= MAX_SIMULATION_VERTICES {
                return Err("shirring refinement exceeds the simulation vertex limit".into());
            }
            midpoints.insert((a, b), points.len());
            points.push(middle);
            added.push((a, b));
        }
        if midpoints.is_empty() {
            break;
        }
        let mut refined = Vec::with_capacity(triangles.len());
        for &[a, b, c] in triangles.iter() {
            let split =
                [edge(a, b), edge(b, c), edge(c, a)].map(|key| midpoints.get(&key).copied());
            subdivide([a, b, c], split, &mut refined);
        }
        for side in &mut *sides {
            let mut expanded = Vec::with_capacity(side.len());
            for pair in side.windows(2) {
                expanded.push(pair[0]);
                expanded.extend(midpoints.get(&edge(pair[0], pair[1])).copied());
            }
            expanded.extend(side.last().copied());
            *side = expanded;
        }
        *triangles = refined;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat_fit::solver::flat_cross;

    #[test]
    fn every_edge_split_pattern_preserves_area_and_winding() {
        let p = [[0., 0.], [2., 0.], [0., 2.], [1., 0.], [1., 1.], [0., 1.]];
        for mask in 0..8 {
            let mid = std::array::from_fn(|i| (mask & (1 << i) != 0).then_some(i + 3));
            let mut faces = Vec::new();
            subdivide([0, 1, 2], mid, &mut faces);
            let areas: Vec<_> = faces
                .iter()
                .map(|&[a, b, c]| flat_cross(p[a], p[b], p[c]))
                .collect();
            assert!(areas.iter().all(|area| *area > 0.0));
            assert_eq!(areas.iter().sum::<f64>(), 4.0);
        }
    }
}
