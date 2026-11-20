use super::{bounds, coordinate_key, flat_cross, Mesh, Point, Point2, SavedPoseMesh};
use std::collections::HashMap;

pub(super) fn transfer(
    mesh: &mut Mesh,
    saved: &SavedPoseMesh,
    velocities: &[Point],
    millimetres: bool,
) -> Option<()> {
    let old = &saved.texcoords;
    let query = if millimetres {
        &mesh.points
    } else {
        &mesh.texcoords
    };
    if old.len() < 3
        || old.len() != saved.positions.len()
        || old.len() != velocities.len()
        || old
            .iter()
            .flatten()
            .chain(saved.positions.iter().flatten())
            .chain(velocities.iter().flatten())
            .any(|v| !v.is_finite())
        || saved.triangles.is_empty()
        || saved.triangles.iter().flatten().any(|&i| i >= old.len())
    {
        return None;
    }
    let (low, high) = bounds(old);
    let extent = [high[0] - low[0], high[1] - low[1]];
    if extent.iter().any(|&v| v <= 1e-10 || !v.is_finite()) {
        return None;
    }
    let normalize = |p: Point2| std::array::from_fn(|axis| (p[axis] - low[axis]) / extent[axis]);
    let nodes: Vec<_> = old.iter().copied().map(normalize).collect();
    let grid = (saved.triangles.len() as f64)
        .sqrt()
        .ceil()
        .clamp(1.0, 64.0) as usize;
    let cell = |v: f64| ((v * grid as f64) as usize).min(grid - 1);
    const EPSILON: f64 = 1e-8;
    let mut bins = vec![Vec::new(); grid * grid];
    let mut references = 0;
    for &[a, b, c] in &saved.triangles {
        let triangle = [nodes[a], nodes[b], nodes[c]];
        let (min, max) = bounds(&triangle);
        let xs = cell(min[0] - EPSILON)..=cell(max[0] + EPSILON);
        let ys = cell(min[1] - EPSILON)..=cell(max[1] + EPSILON);
        references += (xs.end() - xs.start() + 1) * (ys.end() - ys.start() + 1);
        if references > 2_000_000 {
            return None;
        }
        for y in ys {
            for x in xs.clone() {
                bins[y * grid + x].push([a, b, c]);
            }
        }
    }
    let mut exact = HashMap::new();
    for (i, &point) in old.iter().enumerate() {
        exact.entry(coordinate_key(point)).or_insert(i);
    }
    let mut next_positions = Vec::with_capacity(query.len());
    let mut next_velocities = Vec::with_capacity(query.len());
    for &point in query {
        if let Some(&i) = exact.get(&coordinate_key(point)) {
            next_positions.push(saved.positions[i]);
            next_velocities.push(velocities[i]);
            continue;
        }
        let p = normalize(point);
        let (triangle, weights) =
            bins[cell(p[1]) * grid + cell(p[0])]
                .iter()
                .find_map(|&indices| {
                    let [a, b, c] = indices.map(|i| nodes[i]);
                    let area = flat_cross(a, b, c);
                    if area.abs() < 1e-14 {
                        return None;
                    }
                    let weights = [
                        flat_cross(b, c, p) / area,
                        flat_cross(c, a, p) / area,
                        flat_cross(a, b, p) / area,
                    ];
                    weights
                        .iter()
                        .all(|w| (-EPSILON..=1.0 + EPSILON).contains(w))
                        .then_some((indices, weights))
                })?;
        let blend = |values: &[Point]| {
            std::array::from_fn(|axis| {
                triangle
                    .iter()
                    .zip(weights)
                    .map(|(&index, weight)| values[index][axis] * weight)
                    .sum()
            })
        };
        next_positions.push(blend(&saved.positions));
        next_velocities.push(blend(velocities));
    }
    mesh.positions = next_positions;
    mesh.velocities = next_velocities;
    Some(())
}
