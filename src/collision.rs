use serde::Serialize;
use std::{
    collections::BTreeSet,
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
        let [p, q, r] = self.p;
        let (u, v, ray) = (sub(q, p), sub(r, p), sub(b, a));
        let x = cross(ray, v);
        let det = dot(u, x);
        if det.abs() < 1e-10 {
            return false;
        }
        let inv = det.recip();
        let delta = sub(a, p);
        let s = dot(delta, x) * inv;
        if !(0.0..=1.0).contains(&s) {
            return false;
        }
        let y = cross(delta, u);
        let t = dot(ray, y) * inv;
        if t < 0.0 || s + t > 1.0 {
            return false;
        }
        let k = dot(v, y) * inv;
        (1e-8..1.0 - 1e-8).contains(&k)
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
