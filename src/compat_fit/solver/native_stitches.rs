//! Sew archived outline and internal intervals without snapping endpoints to vertices.

use super::{array, distance2, flat_cross, length, Mesh, Point, Point2};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;

type Anchor = [(usize, f64); 3];

enum Path<'a> {
    Vertices(Cow<'a, [usize]>),
    Rest(Cow<'a, [Point2]>),
}

struct Span<'a> {
    path: Path<'a>,
    mesh: &'a Mesh,
    lengths: Vec<f64>,
    start: f64,
    end: f64,
    offset: usize,
}

fn joined_path<T: Copy + PartialEq>(
    lines: &[Vec<T>],
    start: usize,
    end: usize,
    forward: bool,
) -> Option<(Vec<T>, usize, usize)> {
    let mut path = Vec::new();
    let mut first_end = 0;
    let mut line = start;
    for _ in 0..lines.len() {
        let vertices = lines.get(line)?;
        if vertices.len() < 2 {
            return None;
        }
        let first = if forward {
            vertices.first()?
        } else {
            vertices.last()?
        };
        if path.last().is_some_and(|last| last != first) {
            // Do not bridge disconnected lines, including the ends of an open internal shape.
            return None;
        }
        let shared = usize::from(!path.is_empty());
        let line_start = path.len().saturating_sub(shared);
        if forward {
            path.extend(vertices.iter().copied().skip(shared));
        } else {
            path.extend(vertices.iter().rev().copied().skip(shared));
        }
        if line == start {
            first_end = path.len() - 1;
        }
        if line == end {
            return Some((path, first_end, line_start));
        }
        line = if forward {
            (line + 1) % lines.len()
        } else {
            line.checked_sub(1).unwrap_or(lines.len() - 1)
        };
    }
    None
}

fn span_path<T: Copy + PartialEq>(
    lines: &[Vec<T>],
    start: usize,
    end: usize,
    forward: bool,
) -> Option<(Cow<'_, [T]>, usize, usize)> {
    if start != end {
        let (path, first_end, last_start) = joined_path(lines, start, end, forward)?;
        return Some((Cow::Owned(path), first_end, last_start));
    }
    let line = lines.get(start)?;
    (line.len() >= 2).then(|| (Cow::Borrowed(line.as_slice()), line.len() - 1, 0))
}

fn triangle_anchor(mesh: &Mesh, point: Point2, offset: usize) -> Option<Anchor> {
    const EPSILON: f64 = 1e-7;
    for &[ia, ib, ic] in &mesh.triangles {
        let [a, b, c] = [mesh.points[ia], mesh.points[ib], mesh.points[ic]];
        let area = flat_cross(a, b, c);
        if area.abs() <= 1e-10 {
            continue;
        }
        let weights = [
            flat_cross(b, c, point) / area,
            flat_cross(c, a, point) / area,
            flat_cross(a, b, point) / area,
        ];
        if weights
            .iter()
            .all(|&w| (-EPSILON..=1.0 + EPSILON).contains(&w))
        {
            let weights = weights.map(|w| w.clamp(0.0, 1.0));
            let sum = weights.iter().sum::<f64>();
            let indices = [ia, ib, ic];
            return Some(std::array::from_fn(|i| {
                (offset + indices[i], weights[i] / sum)
            }));
        }
    }
    None
}

impl<'a> Span<'a> {
    fn new(mesh: &'a Mesh, endpoint: &Value, offset: usize) -> Option<Self> {
        let start_line = usize::try_from(endpoint["start_line"].as_u64()?).ok()?;
        let end_line = usize::try_from(endpoint["end_line"].as_u64()?).ok()?;
        let mut start = endpoint["start_ratio"].as_f64()?;
        let mut end = endpoint["end_ratio"].as_f64()?;
        let forward = endpoint["direction"].as_bool()?;
        if !(0.0..=1.0).contains(&start)
            || !(0.0..=1.0).contains(&end)
            || start_line == end_line && forward != (end > start)
        {
            return None;
        }
        if start_line != end_line && !forward {
            start = 1.0 - start;
            end = 1.0 - end;
        }

        let (path, first_end, last_start) = match endpoint["shape_depth"].as_u64()? {
            1 => {
                let (p, a, b) = span_path(&mesh.sides, start_line, end_line, forward)?;
                (Path::Vertices(p), a, b)
            }
            2 => {
                let shape = usize::try_from(endpoint["shape_index"].as_u64()?).ok()?;
                if let Some(lines) = mesh.inner_seam_paths.get(shape) {
                    let (p, a, b) = span_path(lines, start_line, end_line, forward)?;
                    (Path::Vertices(p), a, b)
                } else {
                    let lines = mesh.inner_seam_rest.get(shape)?;
                    let (p, a, b) = span_path(lines, start_line, end_line, forward)?;
                    (Path::Rest(p), a, b)
                }
            }
            _ => return None,
        };
        let mut lengths = vec![0.0];
        let mut append = |a, b| {
            lengths.push(lengths.last().copied().unwrap_or_default() + distance2(a, b).sqrt());
        };
        match &path {
            Path::Vertices(vertices) => {
                for pair in vertices.windows(2) {
                    append(mesh.points[pair[0]], mesh.points[pair[1]]);
                }
            }
            Path::Rest(points) => {
                for pair in points.windows(2) {
                    append(pair[0], pair[1]);
                }
            }
        }
        let total = *lengths.last()?;
        if !total.is_finite() || total <= 1e-9 {
            return None;
        }
        let start = start * lengths[first_end];
        let end = lengths[last_start] + end * (total - lengths[last_start]);
        if (end - start).abs() <= 1e-9 {
            return None;
        }
        Some(Self {
            path,
            mesh,
            lengths,
            start,
            end,
            offset,
        })
    }

    fn sample(&self, t: f64) -> Option<Anchor> {
        let x = (self.start + (self.end - self.start) * t)
            .clamp(0.0, self.lengths[self.lengths.len() - 1]);
        let hi = self
            .lengths
            .partition_point(|&l| l < x)
            .clamp(1, self.lengths.len() - 1);
        let lo = hi - 1;
        let distance = self.lengths[hi] - self.lengths[lo];
        let alpha = if distance > 0.0 {
            (x - self.lengths[lo]) / distance
        } else {
            0.0
        };
        match &self.path {
            Path::Vertices(vertices) => Some([
                (vertices[lo] + self.offset, 1.0 - alpha),
                (vertices[hi] + self.offset, alpha),
                (0, 0.0),
            ]),
            Path::Rest(points) => {
                let p = std::array::from_fn(|i| {
                    points[lo][i] + alpha * (points[hi][i] - points[lo][i])
                });
                triangle_anchor(self.mesh, p, self.offset)
            }
        }
    }

    fn breakpoints(&self) -> impl Iterator<Item = f64> + '_ {
        self.lengths
            .iter()
            .map(|l| (l - self.start) / (self.end - self.start))
            .filter(|&t| t > 0.0 && t < 1.0)
    }
}

pub(super) struct Stitch {
    terms: [(usize, f64); 6],
    used: usize,
    inverse_mass: f64,
}

impl Stitch {
    fn new(a: Anchor, b: Anchor, weights: &[f64]) -> Option<Self> {
        let mut terms = [(0, 0.0); 6];
        let mut used = 0;
        for (index, coefficient) in a.into_iter().chain(b.map(|(i, c)| (i, -c))) {
            if coefficient.abs() < 1e-12 {
                continue;
            }
            if let Some((_, existing)) = terms[..used].iter_mut().find(|(i, _)| *i == index) {
                *existing += coefficient;
            } else {
                terms[used] = (index, coefficient);
                used += 1;
            }
        }
        if !terms[..used].iter().any(|(_, c)| c.abs() > 1e-12) {
            return None;
        }
        let inverse_mass = terms[..used].iter().map(|&(i, c)| weights[i] * c * c).sum();
        Some(Self {
            terms,
            used,
            inverse_mass,
        })
    }

    fn difference(&self, points: &[Point]) -> Point {
        let mut delta = [0.0; 3];
        for &(index, c) in &self.terms[..self.used] {
            for (d, p) in delta.iter_mut().zip(points[index]) {
                *d += c * p;
            }
        }
        delta
    }

    pub(super) fn gap(&self, points: &[Point]) -> f64 {
        length(self.difference(points))
    }

    pub(super) fn apply(&self, points: &mut [Point], weights: &[f64]) {
        if self.inverse_mass <= 1e-12 {
            return;
        }
        let delta = self.difference(points);
        for &(index, c) in &self.terms[..self.used] {
            let scale = 0.8 * weights[index] * c / self.inverse_mass;
            for (p, d) in points[index].iter_mut().zip(delta) {
                *p -= scale * d;
            }
        }
    }
}

#[derive(Default)]
pub(super) struct NativeStitches {
    pub constraints: Vec<Stitch>,
    pub groups: usize,
    pub inner_groups: usize,
    pub unsupported: usize,
}

pub(super) fn build(
    state: &Value,
    meshes: &[Mesh],
    offsets: &[usize],
    weights: &[f64],
) -> NativeStitches {
    let by_id: HashMap<_, _> = meshes
        .iter()
        .zip(offsets)
        .map(|(mesh, &offset)| (&mesh.id, (mesh, offset)))
        .collect();
    let span = |endpoint: &Value| {
        let &(mesh, offset) = by_id.get(&endpoint["pattern_id"])?;
        Span::new(mesh, endpoint, offset)
    };
    let mut result = NativeStitches::default();
    for group in array(&state["native_seam_groups"]) {
        if group["restored"] == true || group["active"] == false {
            continue;
        }
        let segments = array(&group["segments"]);
        let spans = (group["active"] == true && !segments.is_empty())
            .then(|| {
                segments
                    .iter()
                    .map(|pair| Some((span(pair.get(0)?)?, span(pair.get(1)?)?)))
                    .collect::<Option<Vec<_>>>()
            })
            .flatten();
        let Some(spans) = spans else {
            result.unsupported += 1;
            continue;
        };
        let start = result.constraints.len();
        let attached = spans.into_iter().try_for_each(|(a, b)| {
            let mut samples: Vec<_> = std::iter::once(0.0)
                .chain(a.breakpoints())
                .chain(b.breakpoints())
                .chain(std::iter::once(1.0))
                .collect();
            samples.sort_unstable_by(f64::total_cmp);
            samples.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
            for t in samples {
                if let Some(stitch) = Stitch::new(a.sample(t)?, b.sample(t)?, weights) {
                    result.constraints.push(stitch);
                }
            }
            Some(())
        });
        if attached.is_none() {
            result.constraints.truncate(start);
        }
        if result.constraints.len() > start {
            result.groups += 1;
            result.inner_groups += usize::from(segments.iter().any(|pair| {
                array(pair)
                    .iter()
                    .any(|endpoint| endpoint["shape_depth"] == 2)
            }));
        } else {
            result.unsupported += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rebuilt_mesh_anchors_internal_seams_in_rest_coordinates() {
        let mesh = super::super::new_mesh(
            &json!({"points":[[0,0],[100,0],[100,50],[0,50]],
                    "inner_seam_rest":[[[[15,7],[50,21],[81,41]]]]}),
            24.0,
            &Value::Null,
        )
        .unwrap();
        let endpoint = json!({
            "shape_depth":2,"shape_index":0,"start_line":0,"end_line":0,
            "start_ratio":0,"end_ratio":1,"direction":true
        });
        let span = Span::new(&mesh, &endpoint, 7).unwrap();
        let coordinates = |fraction| {
            let anchor = span.sample(fraction).unwrap();
            std::array::from_fn::<_, 2, _>(|axis| {
                anchor
                    .iter()
                    .map(|&(i, weight)| weight * mesh.points[i - 7][axis])
                    .sum::<f64>()
            })
        };
        for (fraction, expected) in [(0.0, [15.0, 7.0]), (1.0, [81.0, 41.0])] {
            assert!(distance2(coordinates(fraction), expected) < 1e-18);
        }
        assert!(triangle_anchor(&mesh, [-1.0, 25.0], 0).is_none());
    }

    #[test]
    fn native_span_uses_distance_across_edges_in_either_direction() {
        let mesh = super::super::new_mesh(
            &json!({"points":[[0,0],[100,0],[100,50],[0,50]]}),
            f64::INFINITY,
            &Value::Null,
        )
        .unwrap();
        let make = |first, last, start_ratio, end_ratio, direction| {
            Span::new(
                &mesh,
                &json!({
                    "shape_depth":1,"start_line":first,"end_line":last,
                    "start_ratio":start_ratio,"end_ratio":end_ratio,"direction":direction
                }),
                0,
            )
            .unwrap()
        };
        let forward = make(0, 1, 0.25, 0.5, true);
        let reverse = make(1, 0, 0.5, 0.25, false);
        let position = |span: &Span, fraction| {
            let [(a, wa), (b, wb), _] = span.sample(fraction).unwrap();
            std::array::from_fn(|axis| mesh.points[a][axis] * wa + mesh.points[b][axis] * wb)
        };
        for (fraction, expected) in [
            (0.0, [25., 0.]),
            (0.5, [75., 0.]),
            (0.75, [100., 0.]),
            (1.0, [100., 25.]),
        ] {
            assert_eq!(position(&forward, fraction), expected);
            assert_eq!(position(&reverse, 1.0 - fraction), expected);
        }
        let wrap = make(3, 0, 0.5, 0.25, true);
        assert_eq!(position(&wrap, 0.5), [0., 0.]);
        assert!(joined_path(&[vec![0, 1], vec![2, 3]], 0, 1, true).is_none());
    }

    #[test]
    fn interior_samples_project_to_each_other_without_moving_frozen_vertices() {
        let mut points = [[0., 0., 0.], [10., 0., 0.], [0., 2., 0.], [10., 2., 0.]];
        let weights = [0., 0., 1., 1.];
        let stitch = Stitch::new(
            [(0, 0.75), (1, 0.25), (0, 0.0)],
            [(2, 0.75), (3, 0.25), (0, 0.0)],
            &weights,
        )
        .unwrap();
        assert_eq!(stitch.gap(&points), 2.0);
        stitch.apply(&mut points, &weights);
        assert!((stitch.gap(&points) - 0.4).abs() < 1e-9);
        assert_eq!(points[..2], [[0., 0., 0.], [10., 0., 0.]]);
    }
}
