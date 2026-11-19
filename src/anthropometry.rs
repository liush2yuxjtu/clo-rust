use crate::avatar::{self, Point};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufWriter, Write},
};

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct Sections {
    pub hips: f64,
    pub waist: f64,
    pub chest: f64,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct Dimensions {
    pub height_mm: f64,
    pub chest_mm: f64,
    pub waist_mm: f64,
    pub hips_mm: f64,
}

#[derive(Deserialize)]
pub struct Request {
    sections: Sections,
    target: Option<Dimensions>,
    mesh: Option<String>,
}

#[derive(Serialize)]
struct Transform {
    ground_mm: f64,
    height_scale: f64,
    center_xz_mm: [f64; 2],
    normalized_y_and_scale: [[f64; 2]; 5],
}

type Key = [i64; 2];
type Mesh = (Vec<Point>, Vec<[usize; 3]>);
const EPS_MM: f64 = 0.0001;
const TOLERANCE_MM: f64 = 0.5;

fn obj(path: &str, units: &str) -> Result<Mesh, String> {
    let scale = avatar::millimetres_per_unit(units)?;
    let mut points = Vec::new();
    let mut faces = Vec::new();
    for line in fs::read_to_string(path).map_err(|e| e.to_string())?.lines() {
        let mut columns = line.split_whitespace();
        match columns.next() {
            Some("v") => {
                let mut p = [0.0; 3];
                for axis in &mut p {
                    *axis = columns
                        .next()
                        .and_then(|v| v.parse::<f64>().ok())
                        .filter(|v| v.is_finite())
                        .ok_or("OBJ 顶点无效")?
                        * scale;
                }
                points.push(p);
            }
            Some("f") => {
                let values = columns
                    .take_while(|value| !value.starts_with('#'))
                    .collect::<Vec<_>>();
                let mut face = [0; 3];
                if values.len() != 3 {
                    return Err("人台 OBJ 必须使用三角面".into());
                }
                for (slot, value) in face.iter_mut().zip(values) {
                    let index = value
                        .split('/')
                        .next()
                        .and_then(|n| n.parse::<isize>().ok())
                        .ok_or("OBJ 面索引无效")?;
                    *slot = if index < 0 {
                        points.len().checked_add_signed(index)
                    } else {
                        index.checked_sub(1).filter(|n| *n >= 0).map(|n| n as usize)
                    }
                    .filter(|n| *n < points.len())
                    .ok_or("OBJ 面索引越界")?;
                }
                faces.push(face);
            }
            _ => {}
        }
    }
    if points.is_empty() || faces.is_empty() {
        return Err("未找到人台三角网格".into());
    }
    Ok((points, faces))
}

fn bounds(points: &[Point]) -> [Point; 2] {
    let mut bounds = [points[0]; 2];
    for point in points {
        for (axis, &value) in point.iter().enumerate() {
            bounds[0][axis] = bounds[0][axis].min(value);
            bounds[1][axis] = bounds[1][axis].max(value);
        }
    }
    bounds
}

fn torso_slice(
    points: &[Point],
    faces: &[[usize; 3]],
    y: f64,
    center_x: f64,
) -> Result<(f64, f64, f64), String> {
    // A tiny offset prevents hitting a vertex exactly, without affecting fitted precision.
    let y = y + EPS_MM * 0.11;
    let mut graph: BTreeMap<Key, BTreeSet<Key>> = BTreeMap::new();
    for face in faces {
        let mut intersections = Vec::with_capacity(2);
        for edge in 0..3 {
            let a = points[face[edge]];
            let b = points[face[(edge + 1) % 3]];
            if (a[1] > y) != (b[1] > y) {
                let t = (y - a[1]) / (b[1] - a[1]);
                intersections.push(
                    [0, 2]
                        .map(|axis| ((a[axis] + (b[axis] - a[axis]) * t) / EPS_MM).round() as i64),
                );
            }
        }
        if let [a, b] = intersections.as_slice() {
            if a != b {
                graph.entry(*a).or_default().insert(*b);
                graph.entry(*b).or_default().insert(*a);
            }
        }
    }
    let mut visited = BTreeSet::new();
    let mut selected: Option<(f64, f64, f64)> = None;
    for &start in graph.keys() {
        if !visited.insert(start) {
            continue;
        }
        let mut stack = vec![start];
        let mut minimum = start;
        let mut maximum = start;
        let mut length = 0.0;
        let mut closed = true;
        while let Some(node) = stack.pop() {
            let neighbours = &graph[&node];
            closed &= neighbours.len() == 2;
            for axis in 0..2 {
                minimum[axis] = minimum[axis].min(node[axis]);
                maximum[axis] = maximum[axis].max(node[axis]);
            }
            for &next in neighbours {
                if node < next {
                    length +=
                        ((node[0] - next[0]) as f64).hypot((node[1] - next[1]) as f64) * EPS_MM;
                }
                if visited.insert(next) {
                    stack.push(next);
                }
            }
        }
        let min_x = minimum[0] as f64 * EPS_MM;
        let max_x = maximum[0] as f64 * EPS_MM;
        let width = max_x - min_x;
        let depth_center = (minimum[1] as f64 + maximum[1] as f64) * EPS_MM / 2.0;
        if closed
            && min_x < center_x
            && center_x < max_x
            && selected.is_none_or(|(w, _, _)| width > w)
        {
            selected = Some((width, length, depth_center));
        }
    }
    selected
        .map(|(width, perimeter, z)| (perimeter, z, width))
        .ok_or_else(|| {
            format!("Y={y:.3} mm 找不到穿过身体中线的闭合横截面；请核对网格、姿势和量体平面")
        })
}

fn measurement(
    points: &[Point],
    faces: &[[usize; 3]],
    sections: [f64; 3],
    center: f64,
) -> Result<(Dimensions, f64), String> {
    let [low, high] = bounds(points);
    let height = high[1] - low[1];
    let [hips, waist, chest] =
        sections.map(|fraction| torso_slice(points, faces, low[1] + fraction * height, center));
    let (waist, z, waist_width) = waist?;
    let (hips, _, hip_width) = hips?;
    let (chest, _, chest_width) = chest?;
    if chest_width > 1.5 * waist_width.max(hip_width) {
        return Err("胸部截面过宽，可能包含手臂；请指定略低的胸围平面并核对预览".into());
    }
    Ok((
        Dimensions {
            height_mm: height,
            chest_mm: chest,
            waist_mm: waist,
            hips_mm: hips,
        },
        z,
    ))
}

impl Dimensions {
    fn circumferences(self) -> [f64; 3] {
        [self.hips_mm, self.waist_mm, self.chest_mm]
    }
}

fn section_scale(knots: &[[f64; 2]; 5], y: f64) -> f64 {
    for pair in knots.windows(2) {
        if y <= pair[1][0] {
            let t = ((y - pair[0][0]) / (pair[1][0] - pair[0][0])).clamp(0.0, 1.0);
            return pair[0][1] + (pair[1][1] - pair[0][1]) * t * t * (3.0 - 2.0 * t);
        }
    }
    1.0
}

pub fn run(
    input: &str,
    units: &str,
    request: Request,
    output: Option<&str>,
) -> Result<serde_json::Value, String> {
    let sections = [
        request.sections.hips,
        request.sections.waist,
        request.sections.chest,
    ];
    if !sections.iter().all(|&x| (0.15..0.90).contains(&x))
        || sections.windows(2).any(|x| x[1] - x[0] < 0.025)
    {
        return Err("量体平面须按臀、腰、胸的顺序，位于身高的 15% 至 90%，间隔至少 2.5%".into());
    }
    let (points, faces) = if let Some(name) = request.mesh {
        let surface = avatar::load(input, &name, units)?;
        (surface.points, surface.faces)
    } else {
        obj(input, units)?
    };
    let [low, high] = bounds(&points);
    let x = (low[0] + high[0]) / 2.0;
    let (before, z) = measurement(&points, &faces, sections, x)?;
    let Some(target) = request.target else {
        return Ok(
            serde_json::json!({"sections":request.sections,"measured":before,"vertices":points.len(),"triangles":faces.len()}),
        );
    };
    let path = output.ok_or("设置目标尺寸时需提供输出 OBJ")?;
    let height_scale = target.height_mm / before.height_mm;
    let wanted = target.circumferences();
    let original = before.circumferences();
    let mut factors: [f64; 3] = std::array::from_fn(|i| wanted[i] / original[i]);
    if !std::iter::once(height_scale)
        .chain(factors)
        .all(|v| (0.8..=1.2).contains(&v))
    {
        return Err("当前尺寸调整只接受相对于输入网格 ±20% 的目标".into());
    }
    for _ in 0..8 {
        if !factors.iter().all(|v| (0.79..=1.21).contains(v)) {
            return Err("截面调整超出允许的安全系数范围".into());
        }
        let knots = [
            [0.0, 1.0],
            [sections[0], factors[0]],
            [sections[1], factors[1]],
            [sections[2], factors[2]],
            [1.0, 1.0],
        ];
        let adjusted = points
            .iter()
            .map(|p| {
                let y = (p[1] - low[1]) / before.height_mm;
                let scale = section_scale(&knots, y);
                [
                    x + (p[0] - x) * scale,
                    low[1] + (p[1] - low[1]) * height_scale,
                    z + (p[2] - z) * scale,
                ]
            })
            .collect::<Vec<_>>();
        let (after, _) = measurement(&adjusted, &faces, sections, x)?;
        let values = after.circumferences();
        if values
            .iter()
            .zip(wanted)
            .all(|(a, b)| (a - b).abs() <= TOLERANCE_MM)
            && (after.height_mm - target.height_mm).abs() <= TOLERANCE_MM
        {
            let mut file = BufWriter::new(
                File::options()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|e| e.to_string())?,
            );
            writeln!(file, "# positions in millimetres, Y up\no avatar")
                .map_err(|e| e.to_string())?;
            for [a, b, c] in adjusted {
                writeln!(file, "v {a:.9} {b:.9} {c:.9}").map_err(|e| e.to_string())?;
            }
            for [a, b, c] in &faces {
                writeln!(file, "f {} {} {}", a + 1, b + 1, c + 1).map_err(|e| e.to_string())?;
            }
            file.flush().map_err(|e| e.to_string())?;
            return Ok(
                serde_json::json!({"status":"passed","sections":request.sections,"before":before,
                "target":target,"measured":after,"tolerance_mm":TOLERANCE_MM,"output_units":"mm",
                "vertices":points.len(),"triangles":faces.len(),"transform":Transform {
                    ground_mm:low[1],height_scale,center_xz_mm:[x,z],normalized_y_and_scale:knots}}),
            );
        }
        for i in 0..3 {
            factors[i] *= wanted[i] / values[i];
        }
    }
    Err("调整后横截面未达到 0.5 mm 容差，未交付候选网格".into())
}
