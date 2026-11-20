//! Sew archived outline and internal intervals without snapping endpoints to vertices.

use super::{array, distance2, length, Mesh, Point};
use serde_json::Value;
use std::collections::HashMap;

struct Span<'a> {
    vertices: &'a [usize],
    lengths: Vec<f64>,
    start: f64,
    end: f64,
    offset: usize,
}

impl<'a> Span<'a> {
    fn new(mesh: &'a Mesh, endpoint: &Value, offset: usize) -> Option<Self> {
        if endpoint["start_line"] != endpoint["end_line"] {
            return None;
        }
        let side = usize::try_from(endpoint["start_line"].as_u64()?).ok()?;
        let vertices = match endpoint["shape_depth"].as_u64()? {
            1 => mesh.sides.get(side)?,
            2 => {
                let shape = usize::try_from(endpoint["shape_index"].as_u64()?).ok()?;
                mesh.inner_seam_paths.get(shape)?.get(side)?
            }
            _ => return None,
        };
        let start = endpoint["start_ratio"].as_f64()?;
        let end = endpoint["end_ratio"].as_f64()?;
        if !(0.0..=1.0).contains(&start)
            || !(0.0..=1.0).contains(&end)
            || (end - start).abs() < 1e-9
            || endpoint["direction"].as_bool()? != (end > start)
            || vertices.len() < 2
        {
            return None;
        }
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
        Some(Self {
            vertices,
            lengths,
            start: start * total,
            end: end * total,
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
