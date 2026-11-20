//! Sew archived outline and internal intervals without snapping endpoints to vertices.

use super::{array, distance2, length, Mesh, Point};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;

struct Span<'a> {
    vertices: Cow<'a, [usize]>,
    lengths: Vec<f64>,
    start: f64,
    end: f64,
    offset: usize,
}

fn joined_path(
    lines: &[Vec<usize>],
    start: usize,
    end: usize,
    forward: bool,
) -> Option<(Vec<usize>, usize, usize)> {
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

impl<'a> Span<'a> {
    fn new(mesh: &'a Mesh, endpoint: &Value, offset: usize) -> Option<Self> {
        let start_line = usize::try_from(endpoint["start_line"].as_u64()?).ok()?;
        let end_line = usize::try_from(endpoint["end_line"].as_u64()?).ok()?;
        let lines = match endpoint["shape_depth"].as_u64()? {
            1 => &mesh.sides,
            2 => {
                let shape = usize::try_from(endpoint["shape_index"].as_u64()?).ok()?;
                mesh.inner_seam_paths.get(shape)?
            }
            _ => return None,
        };
        let start = endpoint["start_ratio"].as_f64()?;
        let end = endpoint["end_ratio"].as_f64()?;
        let forward = endpoint["direction"].as_bool()?;
        if !(0.0..=1.0).contains(&start) || !(0.0..=1.0).contains(&end) {
            return None;
        }

        let (vertices, start, end, first_end, last_start) = if start_line == end_line {
            let vertices = lines.get(start_line)?;
            if vertices.len() < 2 || forward != (end > start) {
                return None;
            }
            (
                Cow::Borrowed(vertices.as_slice()),
                start,
                end,
                vertices.len() - 1,
                0,
            )
        } else {
            let (vertices, first_end, last_start) =
                joined_path(lines, start_line, end_line, forward)?;
            let orient = |ratio| if forward { ratio } else { 1.0 - ratio };
            (
                Cow::Owned(vertices),
                orient(start),
                orient(end),
                first_end,
                last_start,
            )
        };
        let mut lengths = vec![0.0];
        for pair in vertices.windows(2) {
            lengths.push(
                lengths.last()? + distance2(mesh.points[pair[0]], mesh.points[pair[1]]).sqrt(),
            );
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
            vertices,
            lengths,
            start,
            end,
            offset,
        })
    }

    fn sample(&self, t: f64) -> [(usize, f64); 2] {
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
        [
            (self.vertices[lo] + self.offset, 1.0 - alpha),
            (self.vertices[hi] + self.offset, alpha),
        ]
    }

    fn breakpoints(&self) -> impl Iterator<Item = f64> + '_ {
        self.lengths
            .iter()
            .map(|l| (l - self.start) / (self.end - self.start))
            .filter(|&t| t > 0.0 && t < 1.0)
    }
}

pub(super) struct Stitch {
    terms: [(usize, f64); 4],
    used: usize,
    inverse_mass: f64,
}

impl Stitch {
    fn new(a: [(usize, f64); 2], b: [(usize, f64); 2], weights: &[f64]) -> Option<Self> {
        let mut terms = [(0, 0.0); 4];
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
        for (a, b) in spans {
            let mut samples: Vec<_> = std::iter::once(0.0)
                .chain(a.breakpoints())
                .chain(b.breakpoints())
                .chain(std::iter::once(1.0))
                .collect();
            samples.sort_unstable_by(f64::total_cmp);
            samples.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
            result.constraints.extend(
                samples
                    .into_iter()
                    .filter_map(|t| Stitch::new(a.sample(t), b.sample(t), weights)),
            );
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
            let [(a, wa), (b, wb)] = span.sample(fraction);
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
        let stitch = Stitch::new([(0, 0.75), (1, 0.25)], [(2, 0.75), (3, 0.25)], &weights).unwrap();
        assert_eq!(stitch.gap(&points), 2.0);
        stitch.apply(&mut points, &weights);
        assert!((stitch.gap(&points) - 0.4).abs() < 1e-9);
        assert_eq!(points[..2], [[0., 0., 0.], [10., 0., 0.]]);
    }
}
