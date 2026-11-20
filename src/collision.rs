use serde::Serialize;
use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
};

type Point = [f64; 3];
fn sub(a: Point, b: Point) -> Point {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[derive(Clone, Copy)]
struct Bounds {
    low: Point,
    high: Point,
}
impl Bounds {
    fn union(self, other: Self) -> Self {
        Self {
            low: std::array::from_fn(|i| self.low[i].min(other.low[i])),
            high: std::array::from_fn(|i| self.high[i].max(other.high[i])),
        }
    }
    fn segment(a: Point, b: Point) -> Self {
        Self {
            low: std::array::from_fn(|i| a[i].min(b[i])),
            high: std::array::from_fn(|i| a[i].max(b[i])),
        }
    }

    fn contains_interior(self, point: Point) -> bool {
        (0..3).all(|i| self.low[i] < point[i] && point[i] < self.high[i])
    }

    fn overlaps(self, other: Self) -> bool {
        (0..3).all(|i| self.low[i] <= other.high[i] && other.low[i] <= self.high[i])
    }
}

#[derive(Clone)]
struct Triangle {
    p: [Point; 3],
    bounds: Bounds,
}
impl Triangle {
    fn new(p: [Point; 3]) -> Self {
        let bounds = p
            .map(|v| Bounds { low: v, high: v })
            .into_iter()
            .reduce(Bounds::union)
            .unwrap();
        Self { p, bounds }
    }
    fn crosses_segment(&self, a: Point, b: Point) -> bool {
        self.segment_fraction(a, b)
            .is_some_and(|t| (1e-8..1.0 - 1e-8).contains(&t))
    }

    fn segment_fraction(&self, a: Point, b: Point) -> Option<f64> {
        let [p, q, r] = self.p;
        let (u, v, ray) = (sub(q, p), sub(r, p), sub(b, a));
        let x = cross(ray, v);
        let det = dot(u, x);
        if det.abs() < 1e-10 {
            return None;
        }
        let inv = det.recip();
        let delta = sub(a, p);
        let s = dot(delta, x) * inv;
        if !(0.0..=1.0).contains(&s) {
            return None;
        }
        let y = cross(delta, u);
        let t = dot(ray, y) * inv;
        if t < 0.0 || s + t > 1.0 {
            return None;
        }
        let k = dot(v, y) * inv;
        (0.0..=1.0).contains(&k).then_some(k)
    }
    fn crosses_edges(&self, other: &Self) -> bool {
        let [a, b, c] = other.p;
        [(a, b), (b, c), (c, a)]
            .into_iter()
            .any(|(a, b)| self.crosses_segment(a, b))
    }
}

enum Bvh {
    Leaf(Bounds, Vec<Triangle>),
    Branch(Bounds, Box<Bvh>, Box<Bvh>),
}
impl Bvh {
    fn new(mut faces: Vec<Triangle>) -> Self {
        let bounds = faces
            .iter()
            .map(|p| p.bounds)
            .reduce(Bounds::union)
            .unwrap();
        if faces.len() <= 8 {
            return Self::Leaf(bounds, faces);
        }
        let axis = (0..3)
            .max_by(|&a, &b| {
                (bounds.high[a] - bounds.low[a]).total_cmp(&(bounds.high[b] - bounds.low[b]))
            })
            .unwrap();
        let mid = faces.len() / 2;
        faces.select_nth_unstable_by(mid, |a, b| {
            let centre = |f: &Triangle| f.bounds.high[axis] + f.bounds.low[axis];
            centre(a).total_cmp(&centre(b))
        });
        let right = faces.split_off(mid);
        Self::Branch(
            bounds,
            Box::new(Self::new(faces)),
            Box::new(Self::new(right)),
        )
    }
    fn bounds(&self) -> Bounds {
        match self {
            Self::Leaf(bounds, _) | Self::Branch(bounds, _, _) => *bounds,
        }
    }

    fn ray_crossings(&self, path: Bounds, start: Point, end: Point, hits: &mut Vec<f64>) {
        if !self.bounds().overlaps(path) {
            return;
        }
        match self {
            Self::Branch(_, left, right) => {
                left.ray_crossings(path, start, end, hits);
                right.ray_crossings(path, start, end, hits);
            }
            Self::Leaf(_, faces) => {
                hits.extend(
                    faces
                        .iter()
                        .filter(|face| face.bounds.overlaps(path))
                        .filter_map(|face| face.segment_fraction(start, end)),
                );
            }
        }
    }

    fn first_contact(
        &self,
        bounds: Bounds,
        start: Point,
        end: Point,
        nearest: &mut Option<(f64, Point)>,
    ) {
        if !self.bounds().overlaps(bounds) {
            return;
        }
        match self {
            Self::Branch(_, left, right) => {
                left.first_contact(bounds, start, end, nearest);
                right.first_contact(bounds, start, end, nearest);
            }
            Self::Leaf(_, faces) => {
                for face in faces {
                    if !face.bounds.overlaps(bounds) {
                        continue;
                    }
                    if let Some(t) = face.segment_fraction(start, end) {
                        if nearest.is_none_or(|(best, _)| t < best) {
                            let [a, b, c] = face.p;
                            *nearest = Some((t, cross(sub(b, a), sub(c, a))));
                        }
                    }
                }
            }
        }
    }

    fn query(&self, cloth: &Triangle, counts: &mut Count) {
        let bounds = match self {
            Self::Leaf(b, _) | Self::Branch(b, _, _) => *b,
        };
        if !bounds.overlaps(cloth.bounds) {
            return;
        }
        match self {
            Self::Branch(_, a, b) => {
                a.query(cloth, counts);
                b.query(cloth, counts);
            }
            Self::Leaf(_, body) => {
                for face in body {
                    if !face.bounds.overlaps(cloth.bounds) {
                        continue;
                    }
                    counts.broad_candidates += 1;
                    let a = face.crosses_edges(cloth);
                    let b = cloth.crosses_edges(face);
                    counts.cloth_edge_body_face += usize::from(a);
                    counts.body_edge_cloth_face += usize::from(b);
                    counts.unique_triangle_pairs += usize::from(a || b);
                }
            }
        }
    }
}

/// Static triangle surfaces against which the cloth solver sweeps vertices.
pub(crate) struct MeshCollider(Vec<CollisionSurface>);

struct CollisionSurface {
    tree: Bvh,
    closed: bool,
}

impl CollisionSurface {
    fn new(points: &[Point], faces: &[[usize; 3]]) -> Option<Self> {
        let mut triangles = Vec::new();
        let mut edges: HashMap<(usize, usize), (u32, i64)> = HashMap::new();
        for &[a, b, c] in faces {
            let (Some(&pa), Some(&pb), Some(&pc)) = (points.get(a), points.get(b), points.get(c))
            else {
                continue;
            };
            if [pa, pb, pc]
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
            {
                continue;
            }
            let Some(triangle) = nondegenerate([pa, pb, pc]) else {
                continue;
            };
            triangles.push(triangle);
            for (u, v) in [(a, b), (b, c), (c, a)] {
                let (count, direction) = edges.entry((u.min(v), u.max(v))).or_default();
                *count += 1;
                *direction += if u < v { 1 } else { -1 };
            }
        }
        let closed = triangles.len() == faces.len() && edges.values().all(|pair| *pair == (2, 0));
        (!triangles.is_empty()).then(|| Self {
            tree: Bvh::new(triangles),
            closed,
        })
    }

    fn push_out(&self, point: &mut Point, clearance: f64, hits: &mut Vec<f64>) -> bool {
        let bounds = self.tree.bounds();
        if !self.closed || !bounds.contains_interior(*point) {
            return false;
        }

        let mut inside_votes = 0;
        let mut best_exit: Option<(usize, f64)> = None;
        for axis in 0..3 {
            let mut ray_end = *point;
            ray_end[axis] = bounds.high[axis] + 1.0;
            hits.clear();
            self.tree
                .ray_crossings(Bounds::segment(*point, ray_end), *point, ray_end, hits);
            hits.sort_unstable_by(f64::total_cmp);
            hits.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            if hits.first().is_some_and(|hit| *hit < 1e-9) {
                return false;
            }
            if hits.len() % 2 == 1 {
                inside_votes += 1;
                let dx = hits[0] * (ray_end[axis] - point[axis]);
                if best_exit.is_none_or(|(_, best)| dx < best) {
                    best_exit = Some((axis, dx));
                }
            }
        }
        if inside_votes < 2 {
            return false;
        }
        for axis in 0..3 {
            let mut ray_end = *point;
            ray_end[axis] = bounds.low[axis] - 1.0;
            let mut nearest = None;
            self.tree.first_contact(
                Bounds::segment(*point, ray_end),
                *point,
                ray_end,
                &mut nearest,
            );
            if let Some((t, _)) = nearest {
                let dx = t * (ray_end[axis] - point[axis]);
                if best_exit.is_none_or(|(_, best)| dx.abs() < best.abs()) {
                    best_exit = Some((axis, dx));
                }
            }
        }
        let Some((axis, dx)) = best_exit else {
            return false;
        };
        point[axis] += dx + clearance.copysign(dx);
        true
    }
}

impl MeshCollider {
    pub(crate) fn new(
        meshes: impl IntoIterator<Item = (Vec<Point>, Vec<[usize; 3]>)>,
    ) -> Option<Self> {
        let surfaces: Vec<_> = meshes
            .into_iter()
            .filter_map(|(points, faces)| CollisionSurface::new(&points, &faces))
            .collect();
        (!surfaces.is_empty()).then_some(Self(surfaces))
    }

    pub(crate) fn separate_initial(
        &self,
        points: &mut [Point],
        weights: &[f64],
        velocities: &mut [Point],
        clearance: f64,
    ) -> usize {
        let mut corrected = 0;
        let mut hits = Vec::new();
        for ((point, &weight), velocity) in points.iter_mut().zip(weights).zip(velocities) {
            if weight <= 0.0 {
                continue;
            }
            for surface in &self.0 {
                if surface.push_out(point, clearance, &mut hits) {
                    *velocity = [0.0; 3];
                    corrected += 1;
                }
            }
        }
        corrected
    }

    fn first_contact<'a>(
        surfaces: impl IntoIterator<Item = &'a Self>,
        start: Point,
        end: Point,
    ) -> Option<(f64, Point)> {
        if start == end {
            return None;
        }
        let bounds = Bounds::segment(start, end);
        let mut nearest = None;
        for collider in surfaces {
            for surface in &collider.0 {
                surface.tree.first_contact(bounds, start, end, &mut nearest);
            }
        }
        nearest
    }

    pub(crate) fn sweep<'a>(
        surfaces: impl IntoIterator<Item = &'a Self> + Clone,
        start: Point,
        end: &mut Point,
        clearance: f64,
        friction: Option<f64>,
    ) -> bool {
        let Some((t, normal)) = Self::first_contact(surfaces.clone(), start, *end) else {
            return false;
        };
        let movement = sub(*end, start);
        let normal = facing(movement, normal);
        let contact = std::array::from_fn(|i| start[i] + t * movement[i] + clearance * normal[i]);
        *end = contact;
        if let Some(friction) = friction {
            let remainder = movement.map(|value| (1.0 - t) * value);
            let inward = (-dot(remainder, normal)).max(0.0);
            let tangent: Point = std::array::from_fn(|i| remainder[i] + inward * normal[i]);
            let tangent_length = dot(tangent, tangent).sqrt();
            if tangent_length > 1e-9 {
                let factor = (1.0 - friction * inward / tangent_length).max(0.0);
                let slide = tangent.map(|value| value * factor);
                *end = std::array::from_fn(|i| contact[i] + slide[i]);
                // A separate surface can block the tangent displacement at a corner.
                if let Some((t, second_normal)) = Self::first_contact(surfaces, contact, *end) {
                    let second_normal = facing(slide, second_normal);
                    let approach = -dot(slide, second_normal);
                    let fraction = if approach > 0.0 {
                        (t - clearance / approach).max(0.0)
                    } else {
                        t
                    };
                    *end = std::array::from_fn(|i| contact[i] + fraction * slide[i]);
                }
            }
        }
        true
    }
}

fn facing(movement: Point, normal: Point) -> Point {
    let sign = if dot(movement, normal) > 0.0 {
        -1.0
    } else {
        1.0
    };
    let scale = sign / dot(normal, normal).sqrt();
    normal.map(|value| value * scale)
}

#[derive(Default, Serialize)]
pub struct Count {
    body_triangles: usize,
    cloth_triangles: usize,
    body_height_mm: f64,
    broad_candidates: usize,
    unique_triangle_pairs: usize,
    cloth_edge_body_face: usize,
    body_edge_cloth_face: usize,
}

pub fn inspect_obj(
    path: &str,
    body_names: &str,
    cloth_prefix: &str,
    output: Option<&str>,
) -> Result<Count, String> {
    if cloth_prefix.is_empty() {
        return Err("服装材质名前缀不能为空".into());
    }
    let body_names: BTreeSet<_> = body_names.split(',').collect();
    let f = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    let mut vertices = Vec::<Point>::new();
    let mut body = Vec::new();
    let mut cloth = Vec::new();
    let mut selected = 0;
    for line in f.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("v") => {
                let v = fields
                    .take(3)
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                let p: Point = v.try_into().map_err(|_| "OBJ 顶点不是三维坐标")?;
                if p.iter().any(|v| !v.is_finite()) {
                    return Err("OBJ 顶点含非有限值".into());
                }
                vertices.push(p);
            }
            Some("usemtl") => {
                let name = fields.next().unwrap_or_default();
                selected = if body_names.contains(name) {
                    1
                } else if name.starts_with(cloth_prefix) {
                    2
                } else {
                    0
                };
            }
            Some("f") if selected != 0 => {
                let points = fields
                    .map(|s| {
                        let raw = s
                            .split('/')
                            .next()
                            .unwrap_or_default()
                            .parse::<isize>()
                            .map_err(|e| e.to_string())?;
                        if raw == 0 {
                            return Err("OBJ 索引不能为 0".to_owned());
                        }
                        let index = if raw > 0 {
                            raw - 1
                        } else {
                            vertices.len() as isize + raw
                        };
                        vertices
                            .get(index as usize)
                            .copied()
                            .ok_or("OBJ 面引用不存在的顶点".to_owned())
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                if points.len() != 3 {
                    return Err("当前验证入口要求三角网格 OBJ".into());
                }
                let Some(tri) = nondegenerate([points[0], points[1], points[2]]) else {
                    continue;
                };
                if selected == 1 {
                    body.push(tri);
                } else {
                    cloth.push(tri);
                }
            }
            _ => {}
        }
    }
    if body.is_empty() || cloth.is_empty() {
        return Err("按给定材质选择不到人体或服装".into());
    }
    inspect_triangles(body, &cloth, output)
}

fn nondegenerate([a, b, c]: [Point; 3]) -> Option<Triangle> {
    let normal = cross(sub(b, a), sub(c, a));
    (dot(normal, normal) > 1e-18).then(|| Triangle::new([a, b, c]))
}

pub fn inspect_surfaces(
    body: &crate::avatar::Surface,
    cloth: &crate::avatar::Surface,
    output: Option<&str>,
) -> Result<Count, String> {
    let triangles = |surface: &crate::avatar::Surface| -> Vec<_> {
        surface.faces.iter()
            .filter_map(|face| nondegenerate(face.map(|index| surface.points[index])))
            .collect()
    };
    inspect_triangles(triangles(body), &triangles(cloth), output)
}

fn inspect_triangles(body: Vec<Triangle>, cloth: &[Triangle], output: Option<&str>) -> Result<Count, String> {
    if body.is_empty() || cloth.is_empty() {
        return Err("人台和服装都需要有效三角面".into());
    }
    let bounds = body.iter().map(|f| f.bounds).reduce(Bounds::union).unwrap();
    let mut count = Count {
        body_triangles: body.len(),
        cloth_triangles: cloth.len(),
        body_height_mm: bounds.high[1] - bounds.low[1],
        ..Default::default()
    };
    let tree = Bvh::new(body);
    let mut points = Vec::new();
    for triangle in cloth {
        let prior = count.unique_triangle_pairs;
        tree.query(triangle, &mut count);
        let hits = count.unique_triangle_pairs - prior;
        if output.is_some() && hits > 0 {
            let [a, b, c] = triangle.p;
            let centroid: Point = std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.0);
            points.push((centroid, hits));
        }
    }
    if let Some(path) = output {
        write_debug(path, &points).map_err(|e| e.to_string())?;
    }
    Ok(count)
}

fn write_debug(path: &str, points: &[(Point, usize)]) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    let count = points.len();
    writeln!(file,"# vtk DataFile Version 3.0\nCloth faces intersecting selected human triangles\nASCII\nDATASET POLYDATA\nPOINTS {count} double")?;
    for &([x, y, z], _) in points {
        writeln!(file, "{x} {y} {z}")?;
    }
    writeln!(file, "VERTICES {count} {}", count * 2)?;
    for index in 0..count {
        writeln!(file, "1 {index}")?;
    }
    writeln!(
        file,
        "POINT_DATA {count}\nSCALARS body_triangle_pairs int 1\nLOOKUP_TABLE default"
    )?;
    for &(_, hits) in points {
        writeln!(file, "{hits}")?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initial_separation_requires_enclosure_and_respects_frozen_vertices() {
        let mesh = MeshCollider::new([(
            vec![[0., 0., 0.], [4., 0., 0.], [0., 4., 0.], [0., 0., 4.]],
            vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        )])
        .unwrap();
        let mut points = [[0.2, 0.3, 0.4], [3.0, 3.0, 3.0], [0.1, 0.1, 0.1]];
        let mut velocities = [[1.0; 3]; 3];
        let count = mesh.separate_initial(&mut points, &[1.0, 1.0, 0.0], &mut velocities, 0.5);
        assert_eq!(count, 1);
        assert!((points[0][0] + 0.5).abs() < 1e-9);
        assert_eq!(points[1..], [[3.0; 3], [0.1; 3]]);
        assert_eq!(velocities, [[0.0; 3], [1.0; 3], [1.0; 3]]);
    }

    #[test]
    fn sweep_stops_at_nearest_surface_regardless_of_face_winding() {
        let plane = |y, triangle| {
            MeshCollider::new([(
                vec![[-4.0, y, -4.0], [4.0, y, -4.0], [0.0, y, 4.0]],
                vec![triangle],
            )])
            .unwrap()
        };
        let far = plane(1.0, [0, 1, 2]);
        let near = plane(3.0, [2, 1, 0]);
        let start = [0.0, 10.0, 0.0];
        let mut end = [0.0, -3.0, 0.0];
        assert!(MeshCollider::sweep(
            [&far, &near],
            start,
            &mut end,
            0.5,
            None
        ));
        assert!((end[1] - 3.5).abs() < 1e-9);
        let mut clear = [5.0, 10.0, 0.0];
        assert!(!MeshCollider::sweep([&near], start, &mut clear, 0.5, None));
    }

    #[test]
    fn sweep_uses_friction_to_limit_tangent_motion_and_checks_corners() {
        let floor = MeshCollider::new([(
            vec![[-30.0, 0.0, -30.0], [30.0, 0.0, -30.0], [0.0, 0.0, 30.0]],
            vec![[0, 1, 2]],
        )])
        .unwrap();
        let wall = MeshCollider::new([(
            vec![[7.0, -10.0, -10.0], [7.0, 10.0, -10.0], [7.0, 0.0, 10.0]],
            vec![[0, 1, 2]],
        )])
        .unwrap();
        for (surfaces, mu, expected) in [
            ([&floor, &floor], 0.0, 10.0),
            ([&floor, &floor], 0.5, 7.5),
            ([&floor, &floor], 1.0, 5.0),
            ([&floor, &wall], 0.0, 6.5),
        ] {
            let mut end = [10.0, -5.0, 0.0];
            assert!(MeshCollider::sweep(
                surfaces,
                [0.0, 5.0, 0.0],
                &mut end,
                0.5,
                Some(mu)
            ));
            assert_eq!(end, [expected, 0.5, 0.0]);
        }
    }

    #[test]
    fn isolated_and_crossing() {
        let body = Triangle::new([[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]]);
        let cross = Triangle::new([[0.1, 0.1, -1.0], [0.1, 0.1, 1.0], [1.0, 1.0, 1.0]]);
        let clear = Triangle::new([[0.1, 0.1, 1.0], [0.1, 0.1, 2.0], [1.0, 1.0, 1.0]]);
        let tree = Bvh::new(vec![body]);
        let mut out = Count::default();
        tree.query(&clear, &mut out);
        assert_eq!(out.unique_triangle_pairs, 0);
        tree.query(&cross, &mut out);
        assert_eq!(out.unique_triangle_pairs, 1);
    }
}
