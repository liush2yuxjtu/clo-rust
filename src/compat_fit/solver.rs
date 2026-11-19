use super::{array, assets, boolean, number, string};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

type Point = [f64; 3];
type Point2 = [f64; 2];
type Edge = (usize, usize);

pub fn sub(a: Point, b: Point) -> Point {
    std::array::from_fn(|i| a[i] - b[i])
}

pub fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: Point, b: Point) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn length(a: Point) -> f64 {
    dot(a, a).sqrt()
}

fn flat_cross(a: Point2, b: Point2, c: Point2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn edge(a: usize, b: usize) -> Edge {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

#[derive(Clone, Serialize)]
pub struct Mesh {
    pub id: Value,
    pub name: String,
    pub fabric_index: Value,
    pub positions: Vec<Point>,
    pub triangles: Vec<[usize; 3]>,
    pub velocities: Vec<Point>,
    #[serde(skip)]
    points: Vec<Point2>,
    #[serde(skip)]
    sides: Vec<Vec<usize>>,
    #[serde(skip)]
    stiffness: f64,
    #[serde(skip)]
    frozen: bool,
}

fn perimeter(pattern: &Value) -> Result<(Vec<Point2>, Vec<usize>), String> {
    let mut vertices: Vec<Point2> = Vec::new();
    let mut corners = Vec::new();
    for (index, p) in array(&pattern["points"]).iter().enumerate() {
        let coords = p.as_array().ok_or("pattern point must be an array")?;
        let x = coords
            .first()
            .and_then(Value::as_f64)
            .filter(|v| v.abs() < 1e7)
            .ok_or("pattern x invalid")?;
        let y = coords
            .get(1)
            .and_then(Value::as_f64)
            .filter(|v| v.abs() < 1e7)
            .ok_or("pattern y invalid")?;
        let kind = coords.get(2).and_then(Value::as_u64).unwrap_or(0);
        if kind == 3 {
            continue;
        }
        if index == 0 || kind == 0 {
            corners.push(vertices.len());
        }
        vertices.push([x, y]);
    }
    if vertices.len() >= 2 && vertices.last() == vertices.first() {
        vertices.pop();
        corners.retain(|&i| i < vertices.len());
    }
    if vertices.len() < 3 || corners.len() < 3 {
        return Err("pattern requires at least three distinct corners".into());
    }
    if vertices.len() > 512 {
        return Err(
            "mesh triangulation currently supports at most 512 perimeter points per pattern".into(),
        );
    }
    Ok((vertices, corners))
}

fn triangulate(points: &[Point2]) -> Result<Vec<[usize; 3]>, String> {
    let area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - a[1] * b[0])
        .sum::<f64>();
    if area.abs() < 1e-8 {
        return Err("pattern has zero signed area".into());
    }
    let mut order: Vec<_> = (0..points.len()).collect();
    if area < 0.0 {
        order.reverse();
    }
    let mut triangles = Vec::new();
    while order.len() > 3 {
        let slot=(0..order.len()).find(|&i|{
            let a=order[(i+order.len()-1)%order.len()];
            let b=order[i];
            let c=order[(i+1)%order.len()];
            if flat_cross(points[a],points[b],points[c])<1e-8 {
                return false;
            }
            !order.iter().filter(|&&id| id!=a && id!=b && id!=c).any(|&j|{
                flat_cross(points[a],points[b],points[j])>=-1e-8 &&
                flat_cross(points[b],points[c],points[j])>=-1e-8 &&
                flat_cross(points[c],points[a],points[j])>=-1e-8
            })
        }).ok_or("cannot triangulate panel perimeter (overlapping, duplicated, or self-crossing points)")?;
        triangles.push([
            order[(slot + order.len() - 1) % order.len()],
            order[slot],
            order[(slot + 1) % order.len()],
        ]);
        order.remove(slot);
    }
    if flat_cross(points[order[0]], points[order[1]], points[order[2]]) < 1e-8 {
        return Err("cannot triangulate final panel triangle".into());
    }
    triangles.push([order[0], order[1], order[2]]);
    Ok(triangles)
}

fn refine(
    points: &mut Vec<Point2>,
    triangles: &mut Vec<[usize; 3]>,
    sides: &mut [Vec<usize>],
    distance: f64,
) {
    for _ in 0..6 {
        if triangles.len() > 10_000 {
            break;
        }
        let longest = triangles
            .iter()
            .map(|t| {
                (0..3)
                    .map(|i| distance2(points[t[i]], points[t[(i + 1) % 3]]))
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max);
        if longest <= distance * distance {
            break;
        }
        let mut midpoints = BTreeMap::<Edge, usize>::new();
        let mut midpoint = |a: usize, b: usize| -> usize {
            *midpoints.entry(edge(a, b)).or_insert_with(|| {
                let middle = [
                    (points[a][0] + points[b][0]) / 2.0,
                    (points[a][1] + points[b][1]) / 2.0,
                ];
                let i = points.len();
                points.push(middle);
                i
            })
        };
        let mut subtris = Vec::with_capacity(triangles.len() * 4);
        for &[a, b, c] in triangles.iter() {
            let ab = midpoint(a, b);
            let bc = midpoint(b, c);
            let ca = midpoint(c, a);
            subtris.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        for side in &mut *sides {
            let mut expanded = Vec::with_capacity(side.len() * 2 - 1);
            for pair in side.windows(2) {
                expanded.extend([pair[0], midpoints[&edge(pair[0], pair[1])]]);
            }
            expanded.push(*side.last().unwrap());
            *side = expanded;
        }
        *triangles = subtris;
    }
}

fn distance2(a: Point2, b: Point2) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn bounds(points: &[Point2]) -> (Point2, Point2) {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in points {
        for i in 0..2 {
            low[i] = low[i].min(point[i]);
            high[i] = high[i].max(point[i]);
        }
    }
    (low, high)
}

fn placement(pattern: &Value, points: &[Point2], avatar: &Value) -> Vec<Point> {
    let (low, high) = bounds(points);
    let center = std::array::from_fn::<_, 2, _>(|i| (low[i] + high[i]) / 2.0);
    let arrangement = string(pattern, "arrangement", "").to_lowercase();
    let body = &avatar["body"];
    let anchor = if arrangement.contains("waist") {
        number(body, "h_waist", 1141.0) - (high[1] - low[1]) / 2.0
    } else if arrangement.contains("hip") {
        number(body, "h_hip", 879.0)
    } else {
        number(body, "h_shoulder", 1454.0) - (high[1] - low[1]) / 2.0 + 45.0
    };
    let depth = body_radius(body, number(body, "h_apex", 1290.0)).map_or(110.0, |r| r[1].max(80.0))
        + 20.0
        + number(pattern, "layer", 0.0) * 5.0;
    let side = if arrangement.contains("left") || arrangement.ends_with("_l") {
        -1.0
    } else {
        1.0
    };
    let drafted = pattern.get("draft_offset_x").is_some();
    points
        .iter()
        .map(|&[u, v]| {
            let local_x = if drafted {
                u - number(pattern, "draft_offset_x", 0.0)
            } else {
                u - center[0]
            };
            let y = if drafted { v } else { v - center[1] + anchor };
            if arrangement.contains("arm") {
                [
                    side * (number(body, "across_shoulder", 377.0) * 0.5 + 42.0),
                    y,
                    local_x,
                ]
            } else if (arrangement.contains("leg") && !arrangement.contains("skirt"))
                || arrangement == "left"
                || arrangement == "right"
            {
                [side * (number(body, "hip", 953.0) / 5.4 + 22.0), y, local_x]
            } else if arrangement.contains("back") {
                [if drafted { -local_x } else { local_x }, y, -depth]
            } else if arrangement.contains("front") {
                [local_x, y, depth]
            } else {
                [u, y, depth + 100.0]
            }
        })
        .collect()
}

fn new_mesh(pattern: &Value, distance: f64, state: &Value) -> Result<Mesh, String> {
    let (mut points, corners) = perimeter(pattern)?;
    let count = points.len();
    let mut sides: Vec<Vec<usize>> = corners
        .iter()
        .enumerate()
        .map(|(i, &first)| {
            let last = corners[(i + 1) % corners.len()];
            (first..=if last > first { last } else { count + last })
                .map(|i| i % count)
                .collect()
        })
        .collect();
    let mut triangles = triangulate(&points)?;
    if distance.is_finite() {
        refine(&mut points, &mut triangles, &mut sides, distance);
    }
    let positions = placement(pattern, &points, &state["avatar"]);
    let velocities = vec![[0.0; 3]; positions.len()];
    Ok(Mesh {
        id: pattern["id"].clone(),
        name: string(pattern, "name", "Pattern").to_owned(),
        fabric_index: pattern["fabric_index"].clone(),
        positions,
        triangles,
        velocities,
        points,
        sides,
        stiffness: if boolean(pattern, "strengthen", false) {
            0.995
        } else {
            assets::material_stiffness(state, &pattern["fabric_index"])
        },
        frozen: boolean(pattern, "freeze", false),
    })
}

pub fn initial_meshes(state: &Value) -> Result<Vec<Mesh>, String> {
    array(&state["patterns"])
        .iter()
        .map(|pattern| new_mesh(pattern, f64::INFINITY, state))
        .collect()
}

fn body_radius(body: &Value, y: f64) -> Option<Point2> {
    let points = [
        (
            number(body, "h_crotch", 841.0) - 35.0,
            number(body, "hip", 953.0) * 0.79,
        ),
        (number(body, "h_hip", 879.0), number(body, "hip", 953.0)),
        (
            number(body, "h_high_hip", 1011.0),
            number(body, "high_hip", 805.0),
        ),
        (
            number(body, "h_waist", 1141.0),
            number(body, "waist", 608.0),
        ),
        (
            number(body, "h_underbust", 1235.0),
            number(body, "underbust", 662.0),
        ),
        (number(body, "h_apex", 1290.0), number(body, "bust", 814.0)),
        (
            number(body, "h_shoulder", 1454.0),
            number(body, "across_shoulder", 377.0) * 2.37,
        ),
        (number(body, "h_hps", 1500.0), number(body, "neck", 355.0)),
        (
            number(body, "h_hps", 1500.0) + 60.0,
            number(body, "neck", 355.0),
        ),
    ];
    let girth = points.windows(2).find_map(|p| {
        let (lo, a) = p[0];
        let (hi, b) = p[1];
        (hi > lo && y >= lo && y <= hi).then(|| a + (b - a) * (y - lo) / (hi - lo))
    })?;
    Some([girth / 5.45, girth / 5.45 * 0.64])
}

#[derive(Clone, Copy)]
struct Proxy {
    center: Point,
    radii: Point,
}

fn limbs(body: &Value) -> Vec<Proxy> {
    let height = number(body, "height", 1755.0);
    let shoulder = number(body, "across_shoulder", 377.0) / 2.0;
    let hip = number(body, "hip", 953.0) / 5.45;
    let crotch = number(body, "h_crotch", 841.0);
    let arm_length = number(body, "arm_length", 589.0);
    let h_shoulder = number(body, "h_shoulder", 1454.0);
    let mut result = vec![Proxy {
        center: [0.0, height * 0.942, 0.0],
        radii: [height * 0.047, height * 0.058, height * 0.050],
    }];
    for sign in [-1.0, 1.0] {
        result.extend([
            Proxy {
                center: [
                    sign * (shoulder + 12.0),
                    h_shoulder - arm_length * 0.42,
                    0.0,
                ],
                radii: [
                    number(body, "bicep", 257.0) / 5.8,
                    arm_length * 0.53,
                    number(body, "bicep", 257.0) / 6.9,
                ],
            },
            Proxy {
                center: [sign * hip * 0.49, crotch * 0.54, 0.0],
                radii: [hip * 0.46, crotch * 0.54, hip * 0.52],
            },
        ]);
    }
    result
}

fn project_ellipse(p: &mut Point, radii: Point2) {
    let r = [radii[0] + 3.0, radii[1] + 3.0];
    let q = (p[0] / r[0]).hypot(p[2] / r[1]);
    if q < 1.0 {
        if q < 1e-9 {
            p[2] = r[1];
        } else {
            p[0] /= q;
            p[2] /= q;
        }
    }
}

fn project_proxy(p: &mut Point, body: &Value, proxies: &[Proxy]) {
    p[1] = p[1].max(2.0);
    if body.is_null() {
        return;
    }
    if let Some(r) = body_radius(body, p[1]) {
        project_ellipse(p, r);
    }
    for proxy in proxies {
        let radial = sub(*p, proxy.center);
        let r = proxy.radii.map(|d| d + 2.0);
        let normalized = std::array::from_fn(|i| radial[i] / r[i]);
        let offset = length(normalized);
        if offset < 1.0 {
            if offset > 1e-9 {
                *p = std::array::from_fn(|i| proxy.center[i] + radial[i] / offset);
            } else {
                p[2] = proxy.center[2] + r[2];
            }
        }
    }
}

pub fn avatar_geometry(avatar: &Value) -> Vec<(Vec<Point>, Vec<[usize; 3]>)> {
    if avatar.is_null() {
        return Vec::new();
    }
    if let (Ok(points), Ok(tris)) = (
        serde_json::from_value(avatar["mesh"]["positions"].clone()),
        serde_json::from_value(avatar["mesh"]["triangles"].clone()),
    ) {
        return vec![(points, tris)];
    }
    let body = &avatar["body"];
    let h = number(body, "height", 1755.0);
    let lower = number(body, "h_crotch", 841.0) - 35.0;
    let upper = number(body, "h_hps", 1500.0) + 60.0;
    let torso = (0..=36)
        .map(|i| {
            let y = lower + (upper - lower) * f64::from(i) / 36.0;
            let r = body_radius(body, y).unwrap_or([h * 0.027, h * 0.018]);
            ([0.0, y, 0.0], r)
        })
        .collect::<Vec<_>>();
    let mut entities = vec![rings(&torso)];
    entities.extend(limbs(body).into_iter().map(|proxy| {
        let els = (0..=20)
            .map(|i| {
                let angle = std::f64::consts::PI * f64::from(i) / 20.0;
                (
                    [
                        proxy.center[0],
                        proxy.center[1] - proxy.radii[1] * angle.cos(),
                        proxy.center[2],
                    ],
                    [
                        proxy.radii[0] * angle.sin().max(0.001),
                        proxy.radii[2] * angle.sin().max(0.001),
                    ],
                )
            })
            .collect::<Vec<_>>();
        rings(&els)
    }));
    entities
}

fn rings(rows: &[(Point, Point2)]) -> (Vec<Point>, Vec<[usize; 3]>) {
    let mut points = Vec::new();
    let mut faces = Vec::new();
    let slices = 36;
    for &(center, r) in rows {
        for slice in 0..slices {
            let a = std::f64::consts::TAU * (slice as f64) / (slices as f64);
            points.push([
                center[0] + r[0] * a.cos(),
                center[1],
                center[2] + r[1] * a.sin(),
            ]);
        }
    }
    for row in 1..rows.len() {
        for slice in 0..slices {
            let cur = row * slices + slice;
            let next = row * slices + (slice + 1) % slices;
            faces.extend([
                [cur, cur - slices, next],
                [next, cur - slices, next - slices],
            ]);
        }
    }
    (points, faces)
}

struct Constraint {
    a: usize,
    b: usize,
    rest: f64,
    stiffness: f64,
}

fn internal_constraints(mesh: &Mesh, offset: usize) -> Vec<Constraint> {
    let mut edges = BTreeMap::new();
    let mut bending = BTreeSet::new();
    for triangle in &mesh.triangles {
        for i in 0..3 {
            let a = triangle[i];
            let b = triangle[(i + 1) % 3];
            let opposite = triangle[(i + 2) % 3];
            let edge = edge(a, b);
            if let Some(previous) = edges.insert(edge, opposite) {
                if opposite != previous {
                    bending.insert((opposite.min(previous), opposite.max(previous)));
                }
            }
        }
    }
    let constraint = |a: usize, b: usize, stiffness: f64| Constraint {
        a: a + offset,
        b: b + offset,
        rest: distance2(mesh.points[a], mesh.points[b]).sqrt(),
        stiffness,
    };
    edges
        .into_keys()
        .map(|(a, b)| constraint(a, b, mesh.stiffness))
        .chain(
            bending
                .into_iter()
                .map(|(a, b)| constraint(a, b, mesh.stiffness * 0.12)),
        )
        .collect()
}

fn side_samples(
    mesh: &Mesh,
    side: usize,
    oriented: bool,
    samples: usize,
    offset: usize,
) -> Result<Vec<usize>, String> {
    let vertices = mesh
        .sides
        .get(side)
        .ok_or_else(|| format!("seam edge {side} is outside panel {}", mesh.name))?;
    let mut lengths = vec![0.0];
    for pair in vertices.windows(2) {
        lengths.push(
            lengths.last().unwrap() + distance2(mesh.points[pair[0]], mesh.points[pair[1]]).sqrt(),
        );
    }
    let total = *lengths.last().unwrap();
    Ok((0..samples)
        .map(|i| {
            let i = if oriented { i } else { samples - 1 - i };
            let at = total * (i as f64) / (samples as f64 - 1.0);
            let j = lengths.partition_point(|&l| l < at);
            let j = if j > 0
                && j < lengths.len()
                && (lengths[j - 1] - at).abs() < (lengths[j] - at).abs()
            {
                j - 1
            } else {
                j.min(lengths.len() - 1)
            };
            offset + vertices[j]
        })
        .collect())
}

fn stitches(state: &Value, mesh: &[Mesh], offsets: &[usize]) -> Result<Vec<Constraint>, String> {
    let mut all = BTreeSet::new();
    for s in array(&state["seams"]) {
        let a = mesh
            .iter()
            .position(|p| p.id == s["pattern_a"])
            .ok_or("seam references missing pattern_a")?;
        let b = mesh
            .iter()
            .position(|p| p.id == s["pattern_b"])
            .ok_or("seam references missing pattern_b")?;
        let side_a = s["line_a"]
            .as_u64()
            .ok_or("seam line_a requires nonnegative integer")? as usize;
        let side_b = s["line_b"]
            .as_u64()
            .ok_or("seam line_b requires nonnegative integer")? as usize;
        let count = mesh[a]
            .sides
            .get(side_a)
            .map_or(2, Vec::len)
            .max(mesh[b].sides.get(side_b).map_or(2, Vec::len));
        let edge_a = side_samples(
            &mesh[a],
            side_a,
            boolean(s, "dir_a", true),
            count,
            offsets[a],
        )?;
        let edge_b = side_samples(
            &mesh[b],
            side_b,
            boolean(s, "dir_b", true),
            count,
            offsets[b],
        )?;
        all.extend(
            edge_a
                .into_iter()
                .zip(edge_b)
                .filter(|(a, b)| a != b)
                .map(|(a, b)| edge(a, b)),
        );
    }
    Ok(all
        .into_iter()
        .map(|(a, b)| Constraint {
            a,
            b,
            rest: 0.0,
            stiffness: 0.96,
        })
        .collect())
}

fn apply_constraint(points: &mut [Point], weights: &[f64], c: &Constraint) {
    let shift = sub(points[c.b], points[c.a]);
    let distance = length(shift);
    let mass = weights[c.a] + weights[c.b];
    if distance > 1e-9 && mass > 0.0 {
        let correction = (distance - c.rest) / distance * c.stiffness / mass;
        for (i, x) in shift.into_iter().enumerate() {
            let correction = x * correction;
            points[c.a][i] += correction * weights[c.a];
            points[c.b][i] -= correction * weights[c.b];
        }
    }
}

pub fn simulate(args: &Value, state: &mut Value) -> Result<Value, String> {
    let start = std::time::Instant::now();
    let existing = &state["simulation"];
    let steps = args["steps"].as_u64().unwrap_or(100);
    let distance = number(
        args,
        "particle_distance",
        number(existing, "particle_distance", 20.0),
    );
    let quality = args["quality"]
        .as_u64()
        .or(existing["quality"].as_u64())
        .unwrap_or(0);
    if steps > 3000 || !(3.0..=500.0).contains(&distance) || quality > 3 {
        return Err(
            "simulation accepts 0..3000 steps, particle_distance 3..500 mm, and quality 0..3"
                .into(),
        );
    }
    if quality == 3 {
        return Err("quality 3 requires a GPU cloth solver; CLO Rust currently supports CPU qualities 0, 1, and 2".into());
    }
    let patterns = array(&state["patterns"]);
    if patterns.is_empty() {
        return Err("simulation requires at least one pattern".into());
    }
    let mut meshes = patterns
        .iter()
        .map(|pattern| {
            let target = if args["particle_distance"].is_number() {
                distance
            } else {
                number(pattern, "particle_distance", distance)
            };
            if !(3.0..=500.0).contains(&target) {
                return Err("individual pattern particle_distance must be 3..500 mm".into());
            }
            new_mesh(pattern, target, state)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let prior_meshes = array(&existing["meshes"]);
    let resume = (distance - number(existing, "particle_distance", -1.0)).abs() < 1e-9
        && meshes.len() == prior_meshes.len()
        && meshes.iter().zip(prior_meshes).all(|(mesh, prior)| {
            mesh.id == prior["id"] && mesh.positions.len() == array(&prior["positions"]).len()
        });
    if resume {
        for (mesh, prior) in meshes.iter_mut().zip(prior_meshes) {
            mesh.positions = serde_json::from_value(prior["positions"].clone())
                .map_err(|e| format!("invalid saved simulation mesh: {e}"))?;
            mesh.velocities = serde_json::from_value(prior["velocities"].clone())
                .map_err(|e| format!("invalid saved simulation velocities: {e}"))?;
            if mesh.velocities.len() != mesh.positions.len() {
                return Err("saved simulation velocities are inconsistent".into());
            }
        }
    }
    let body = &state["avatar"]["body"];
    let proxies = if body.is_null() { vec![] } else { limbs(body) };
    let mut offsets = Vec::new();
    let mut positions = Vec::new();
    let mut velocities = Vec::new();
    let mut weights = Vec::new();
    let mut constraints = Vec::new();
    for mesh in &meshes {
        let offset = positions.len();
        offsets.push(offset);
        constraints.extend(internal_constraints(mesh, offset));
        positions.extend(mesh.positions.iter().copied());
        velocities.extend(mesh.velocities.iter().copied());
        weights.resize(positions.len(), if mesh.frozen { 0.0 } else { 1.0 });
    }
    if positions.len() > 130_000 {
        return Err("simulation scene exceeds 130000 vertices; increase particle_distance or split the garment".into());
    }
    let seam_constraints = stitches(state, &meshes, &offsets)?;
    let iterations = match quality {
        0 => 4,
        1 => 7,
        _ => 12,
    };
    let dt = 1.0 / 30.0;
    let mut old = positions.clone();
    for _ in 0..steps {
        old.copy_from_slice(&positions);
        for ((p, v), &weight) in positions
            .iter_mut()
            .zip(velocities.iter_mut())
            .zip(&weights)
        {
            if weight == 0.0 {
                *v = [0.0; 3];
                continue;
            }
            v[1] -= 9810.0 * dt;
            for i in 0..3 {
                p[i] += v[i] * dt;
            }
        }
        for _ in 0..iterations {
            for c in constraints.iter().chain(&seam_constraints) {
                apply_constraint(&mut positions, &weights, c);
            }
            for (p, &weight) in positions.iter_mut().zip(&weights) {
                if weight > 0.0 {
                    project_proxy(p, body, &proxies);
                }
            }
        }
        for ((p, prev), v) in positions.iter().zip(&old).zip(velocities.iter_mut()) {
            let delta = sub(*p, *prev);
            *v = delta.map(|x| (x / dt * 0.84).clamp(-1800.0, 1800.0));
        }
    }
    let gap = seam_constraints
        .iter()
        .map(|c| length(sub(positions[c.a], positions[c.b])))
        .fold(0.0, f64::max);
    let effective = constraints.iter().map(|c| c.rest).fold(0.0, f64::max);
    for (mesh, &offset) in meshes.iter_mut().zip(&offsets) {
        let end = offset + mesh.positions.len();
        mesh.positions.copy_from_slice(&positions[offset..end]);
        mesh.velocities.copy_from_slice(&velocities[offset..end]);
    }
    let total = steps
        + if resume {
            existing["steps"].as_u64().unwrap_or(0)
        } else {
            0
        };
    if args["particle_distance"].is_number() {
        if let Some(patterns) = state["patterns"].as_array_mut() {
            for pattern in patterns {
                pattern["particle_distance"] = json!(distance);
            }
        }
    }
    state["simulation"] =
        json!({"steps":total,"particle_distance":distance,"quality":quality,"meshes":meshes});
    Ok(
        json!({"ok":true,"steps":steps,"seconds":start.elapsed().as_secs_f64(),"solver":"rust_cpu_pbd",
        "vertices":positions.len(),"stitch_count":seam_constraints.len(),"max_stitch_gap_mm":gap,
        "max_constraint_length_mm":effective,"limitations":["self-collision unavailable","avatar collisions use an analytic proxy","fabric and edge properties are partially implemented","no CLO material calibration"]}),
    )
}

pub fn layout_meshes(state: &Value) -> Result<Vec<Mesh>, String> {
    array(&state["patterns"])
        .iter()
        .map(|pattern| {
            let mut mesh = new_mesh(pattern, f64::INFINITY, state)?;
            let offset = |axis| pattern["pos_2d"][axis].as_f64().unwrap_or(0.0);
            mesh.positions = mesh
                .points
                .iter()
                .map(|p| [p[0] + offset(0usize), p[1] + offset(1usize), 0.0])
                .collect();
            Ok(mesh)
        })
        .collect()
}
