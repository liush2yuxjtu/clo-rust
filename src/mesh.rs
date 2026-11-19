use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
};

type P3 = [f64; 3];
type P2 = [f64; 2];

fn sub(a: P3, b: P3) -> P3 {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(v: P3) -> f64 {
    dot(v, v).sqrt()
}
fn combine(a: P3, b: P3, p: f64, q: f64) -> P3 {
    std::array::from_fn(|i| a[i] * p + b[i] * q)
}

#[derive(Serialize)]
pub struct Metrics {
    piece_index: usize,
    vertex_count: usize,
    triangle_count: usize,
    analysed_triangles: usize,
    rest_area_mm2: f64,
    strain_p95_pct: Option<f64>,
    fold_interior_edges: usize,
    fold_p95_deg: Option<f64>,
}

struct Piece {
    xyz: Vec<P3>,
    uv: Vec<P2>,
    faces: Vec<[usize; 3]>,
}

struct Fields {
    strain: Vec<Option<f64>>,
    folds: Vec<f64>,
    metrics: Metrics,
}

fn major_strain(x: [P3; 3], r: [P2; 3]) -> Option<(f64, f64)> {
    let (a, b, c, d) = (
        r[1][0] - r[0][0],
        r[2][0] - r[0][0],
        r[1][1] - r[0][1],
        r[2][1] - r[0][1],
    );
    let det = a * d - b * c;
    let area = det.abs() * 0.5;
    if area <= 0.1 {
        return None;
    }
    let (e, f) = (sub(x[1], x[0]), sub(x[2], x[0]));
    let u = combine(e, f, d / det, -c / det);
    let v = combine(e, f, -b / det, a / det);
    let (g00, g11, g01) = (dot(u, u), dot(v, v), dot(u, v));
    let eigen = (g00 + g11 + (g00 - g11).hypot(2.0 * g01)) * 0.5;
    Some(((eigen.sqrt() - 1.0) * 100.0, area))
}

fn weighted_p95(values: &[(f64, f64)], total: f64) -> Option<f64> {
    let first = values.first()?.0;
    let target = total * 0.95;
    let mut sum = 0.0;
    let mut previous = first;
    for &(value, weight) in values {
        if sum + weight >= target {
            return Some(previous + (value - previous) * (target - sum) / weight);
        }
        previous = value;
        sum += weight;
    }
    Some(previous)
}

fn equal_p95(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let pos = (values.len() - 1) as f64 * 0.95;
    let low = pos.floor() as usize;
    Some(values[low] + (values[pos.ceil() as usize] - values[low]) * pos.fract())
}

#[derive(Clone, Copy)]
struct Incidence {
    face: usize,
    forward: bool,
}

fn calculate(piece: &Piece, index: usize) -> Fields {
    let mut straining = Vec::new();
    let mut strains = vec![None; piece.faces.len()];
    let mut folds = vec![0.0_f64; piece.faces.len()];
    let mut total = 0.0;
    let mut ids = BTreeMap::new();
    let bits = |f: f64| if f == 0.0 { 0 } else { f.to_bits() };
    let welded: Vec<usize> = piece
        .xyz
        .iter()
        .zip(&piece.uv)
        .map(|(p, u)| {
            let key = [p[0], p[1], p[2], u[0], u[1]].map(bits);
            let next = ids.len();
            *ids.entry(key).or_insert(next)
        })
        .collect();
    let mut edges: BTreeMap<(usize, usize), Vec<Incidence>> = BTreeMap::new();
    let mut normals = Vec::with_capacity(piece.faces.len());
    for (i, &face) in piece.faces.iter().enumerate() {
        let points = face.map(|v| piece.xyz[v]);
        let rest = face.map(|v| piece.uv[v]);
        if let Some((strain, area)) = major_strain(points, rest) {
            total += area;
            straining.push((strain, area));
            strains[i] = Some(strain);
        }
        let n = cross(sub(points[1], points[0]), sub(points[2], points[0]));
        let len = norm(n);
        normals.push((len > 1e-10).then(|| n.map(|v| v / len)));
        let [a, b, c] = face.map(|v| welded[v]);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            edges
                .entry((p.min(q), p.max(q)))
                .or_default()
                .push(Incidence {
                    face: i,
                    forward: p <= q,
                });
        }
    }
    let mut angles = Vec::new();
    for adjacent in edges.values() {
        let [a, b] = adjacent.as_slice() else {
            continue;
        };
        if a.face == b.face {
            continue;
        }
        let (Some(left), Some(mut right)) = (normals[a.face], normals[b.face]) else {
            continue;
        };
        if a.forward == b.forward {
            right = right.map(|x| -x);
        }
        let angle = norm(cross(left, right))
            .atan2(dot(left, right))
            .to_degrees();
        folds[a.face] = folds[a.face].max(angle);
        folds[b.face] = folds[b.face].max(angle);
        angles.push(angle);
    }
    straining.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    angles.sort_unstable_by(f64::total_cmp);
    Fields {
        strain: strains,
        folds,
        metrics: Metrics {
            piece_index: index,
            vertex_count: piece.xyz.len(),
            triangle_count: piece.faces.len(),
            analysed_triangles: straining.len(),
            rest_area_mm2: total,
            strain_p95_pct: weighted_p95(&straining, total),
            fold_interior_edges: angles.len(),
            fold_p95_deg: equal_p95(&angles),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretched_right_angle_hinge() {
        let piece = Piece {
            xyz: vec![[0.0, 0.0, 0.0], [12.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]],
            uv: vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0], [0.0, -10.0]],
            faces: vec![[0, 1, 2], [1, 0, 3]],
        };
        let fields = calculate(&piece, 0);
        assert_eq!(fields.metrics.analysed_triangles, 2);
        assert_eq!(fields.metrics.rest_area_mm2, 100.0);
        assert_eq!(fields.metrics.fold_interior_edges, 1);
        assert_eq!(fields.folds, [90.0, 90.0]);
        for value in fields.strain {
            assert!((value.unwrap() - 20.0).abs() < 1e-9);
        }
    }
}

fn load(path: &str) -> Result<Vec<Piece>, String> {
    let glb = gltf::Gltf::open(path).map_err(|e| e.to_string())?;
    let blob = glb.blob.as_deref().ok_or("输入必须是内嵌二进制的 GLB")?;
    let mesh = glb
        .meshes()
        .find(|m| m.name() == Some("Cloth_mesh"))
        .ok_or("缺少 CLO Cloth_mesh")?;
    mesh.primitives()
        .map(|primitive| {
            let reader = primitive.reader(|buf| (buf.index() == 0).then_some(blob));
            let xyz = reader
                .read_positions()
                .ok_or("缺少衣片三维位置")?
                .map(|p| p.map(f64::from))
                .collect::<Vec<_>>();
            let uv = reader
                .read_tex_coords(0)
                .ok_or("缺少衣片二维坐标")?
                .into_f32()
                .map(|p| p.map(f64::from))
                .collect::<Vec<_>>();
            let indices = reader
                .read_indices()
                .ok_or("缺少三角面索引")?
                .into_u32()
                .collect::<Vec<_>>();
            if xyz.len() != uv.len()
                || xyz.is_empty()
                || indices.len() % 3 != 0
                || indices.iter().any(|&v| v as usize >= xyz.len())
                || xyz
                    .iter()
                    .flatten()
                    .chain(uv.iter().flatten())
                    .any(|v| !v.is_finite())
            {
                return Err("衣片的数组、索引或坐标无效".into());
            }
            let faces = indices
                .chunks_exact(3)
                .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
                .collect();
            Ok(Piece { xyz, uv, faces })
        })
        .collect()
}

pub fn inspect(path: &str, out: Option<&str>) -> Result<Vec<Metrics>, String> {
    let pieces = load(path)?;
    let fields: Vec<_> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| calculate(p, i))
        .collect();
    if let Some(path) = out {
        save_vtk(path, &pieces, &fields).map_err(|e| e.to_string())?;
    }
    Ok(fields.into_iter().map(|f| f.metrics).collect())
}

fn save_vtk(path: &str, pieces: &[Piece], fields: &[Fields]) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    let n_points: usize = pieces.iter().map(|p| p.xyz.len()).sum();
    let n_triangles: usize = pieces.iter().map(|p| p.faces.len()).sum();
    writeln!(file,"# vtk DataFile Version 3.0\nCLO reference engineering strain and fold angle\nASCII\nDATASET POLYDATA\nPOINTS {n_points} double")?;
    for p in pieces {
        for &[x, y, z] in &p.xyz {
            writeln!(file, "{x} {y} {z}")?;
        }
    }
    writeln!(file, "POLYGONS {n_triangles} {}", n_triangles * 4)?;
    let mut offset = 0;
    for p in pieces {
        for &[a, b, c] in &p.faces {
            writeln!(file, "3 {} {} {}", a + offset, b + offset, c + offset)?;
        }
        offset += p.xyz.len();
    }
    writeln!(
        file,
        "CELL_DATA {n_triangles}\nSCALARS engineering_strain_pct double 1\nLOOKUP_TABLE default"
    )?;
    for f in fields {
        for val in &f.strain {
            writeln!(file, "{}", val.unwrap_or(0.0))?;
        }
    }
    writeln!(file, "SCALARS strain_is_valid int 1\nLOOKUP_TABLE default")?;
    for f in fields {
        for val in &f.strain {
            writeln!(file, "{}", u8::from(val.is_some()))?;
        }
    }
    writeln!(
        file,
        "SCALARS fold_angle_deg double 1\nLOOKUP_TABLE default"
    )?;
    for f in fields {
        for val in &f.folds {
            writeln!(file, "{val}")?;
        }
    }
    file.flush()
}
