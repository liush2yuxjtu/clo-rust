mod native_stitches;
mod shirring;

use super::{array, assets, boolean, number, string};
use crate::collision::MeshCollider;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

type Point = [f64; 3];
type Point2 = [f64; 2];
type Edge = (usize, usize);

#[derive(Clone, Copy)]
struct ClothMaterial {
    stretch: [f64; 3],
    bend: [f64; 3],
    density_g_per_mm2: Option<f64>,
}

impl ClothMaterial {
    fn for_panel(pattern: &Value, state: &Value) -> Self {
        let fabric = assets::assigned_material(state, &pattern["fabric_index"]);
        let properties = &fabric["info"]["physical_properties"];
        let density_g_per_mm2 = properties["density"]
            .as_f64()
            .filter(|value| (f64::from(f32::from_bits(1))..=f64::from(f32::MAX)).contains(value));
        if boolean(pattern, "strengthen", false) {
            return Self {
                stretch: [0.995; 3],
                bend: [0.995 * 0.12; 3],
                density_g_per_mm2,
            };
        }
        let fallback = number(fabric, "stiffness", 0.9).clamp(0.0, 1.0);
        // Map the stored coefficients to the bounded responses used by this solver.
        // The reference values are Rust solver parameters, independent of CLO calibration.
        let response = |key: &str, reference: f64| {
            properties[key]
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map_or(fallback, |value| value / (value + reference))
        };
        Self {
            stretch: ["stretch_u", "stretch_v", "shear"].map(|key| response(key, 20_000.0)),
            bend: ["bend_u", "bend_v", "bend_bias"].map(|key| 0.12 * response(key, 2_000.0)),
            density_g_per_mm2,
        }
    }

    fn along([u, v, bias]: [f64; 3], a: Point2, b: Point2) -> f64 {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let length_squared = dx * dx + dy * dy;
        if length_squared == 0.0 || (u == v && v == bias) {
            return u;
        }
        let along_u = dx * dx / length_squared;
        let along_v = 1.0 - along_u;
        let along_bias = 4.0 * along_u * along_v;
        (u * along_u + v * along_v) * (1.0 - along_bias) + bias * along_bias
    }
}

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
    pub texcoords: Vec<Point2>,
    pub velocities: Vec<Point>,
    #[serde(skip)]
    points: Vec<Point2>,
    #[serde(skip)]
    sides: Vec<Vec<usize>>,
    #[serde(skip)]
    material: ClothMaterial,
    #[serde(skip)]
    frozen: bool,
    #[serde(skip)]
    shirring_edges: Vec<Edge>,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_shirring_base: Option<NativeShirringBase>,
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

pub(crate) fn uv_coordinates(points: &[Point2]) -> Vec<Point2> {
    let (low, high) = bounds(points);
    let width = (high[0] - low[0]).max(1e-10);
    let height = (high[1] - low[1]).max(1e-10);
    points
        .iter()
        .map(|[x, y]| [(x - low[0]) / width, (high[1] - y) / height])
        .collect()
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

fn sampled_sides(indices: &[usize], corners: &[usize]) -> Vec<Vec<usize>> {
    let count = indices.len();
    corners
        .iter()
        .enumerate()
        .map(|(i, &first)| {
            let last = corners[(i + 1) % corners.len()];
            (first..=if last > first { last } else { count + last })
                .map(|i| indices[i % count])
                .collect()
        })
        .collect()
}

fn new_mesh(pattern: &Value, distance: f64, state: &Value) -> Result<Mesh, String> {
    let (mut points, corners) = perimeter(pattern)?;
    if points.len() > 512 {
        return Err(
            "mesh triangulation currently supports at most 512 perimeter points per pattern".into(),
        );
    }
    let indices: Vec<_> = (0..points.len()).collect();
    let mut sides = sampled_sides(&indices, &corners);
    let mut triangles = triangulate(&points)?;
    if distance.is_finite() {
        refine(&mut points, &mut triangles, &mut sides, distance);
    }
    let shirring_edges =
        shirring::refine(pattern, &mut points, &mut triangles, &mut sides, distance)?;
    let positions = placement(pattern, &points, &state["avatar"]);
    let velocities = vec![[0.0; 3]; positions.len()];
    Ok(Mesh {
        id: pattern["id"].clone(),
        name: string(pattern, "name", "Pattern").to_owned(),
        fabric_index: pattern["fabric_index"].clone(),
        positions,
        triangles,
        velocities,
        texcoords: uv_coordinates(&points),
        points,
        sides,
        material: ClothMaterial::for_panel(pattern, state),
        frozen: boolean(pattern, "freeze", false),
        shirring_edges,
        native_shirring_base: None,
    })
}

const MAX_SIMULATION_VERTICES: usize = 130_000;
const NATIVE_SAVED_POSE: &str = "native_saved_pose";
const RUST_NATIVE_REST: &str = "rust_cpu_from_native_saved_pose";

fn coordinate_key(point: Point2) -> [u64; 2] {
    point.map(|v| if v == 0.0 { 0 } else { v.to_bits() })
}

fn mesh_boundary(vertices: usize, triangles: &[[usize; 3]]) -> Result<Vec<Vec<usize>>, String> {
    let mut incidence = BTreeMap::new();
    for &[a, b, c] in triangles {
        if a >= vertices || b >= vertices || c >= vertices || a == b || b == c || c == a {
            return Err("saved native pose triangle has invalid vertex indices".into());
        }
        for boundary_edge in [edge(a, b), edge(b, c), edge(c, a)] {
            let count = incidence.entry(boundary_edge).or_insert(0u8);
            *count += 1;
            if *count > 2 {
                return Err("saved native pose has a non-manifold triangle edge".into());
            }
        }
    }
    let mut neighbors = vec![Vec::new(); vertices];
    for ((a, b), count) in incidence {
        if count == 1 {
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
    }
    Ok(neighbors)
}

fn map_boundary_cycle(
    outline: &[[u64; 2]],
    rest: &[[u64; 2]],
    neighbors: &[Vec<usize>],
) -> Option<Vec<usize>> {
    // A rest coordinate may be shared by multiple vertex indices. Walk actual
    // boundary connectivity, then backtrack once a path closes.
    for start in (0..rest.len()).filter(|&i| rest[i] == outline[0] && !neighbors[i].is_empty()) {
        let mut history = vec![BTreeMap::from([(start, start)])];
        for &expected in &outline[1..] {
            let mut layer = BTreeMap::new();
            for &vertex in history.last().expect("initial boundary layer").keys() {
                for &next in &neighbors[vertex] {
                    if rest[next] == expected {
                        layer.entry(next).or_insert(vertex);
                    }
                }
            }
            if layer.is_empty() {
                break;
            }
            history.push(layer);
        }
        if history.len() == outline.len() {
            if let Some(&last) = history
                .last()
                .expect("last boundary layer")
                .keys()
                .find(|&&i| neighbors[i].contains(&start))
            {
                let mut vertex = last;
                let mut path = vec![start; outline.len()];
                for (i, layer) in history.iter().enumerate().skip(1).rev() {
                    path[i] = vertex;
                    vertex = layer[&vertex];
                }
                return Some(path);
            }
        }
    }
    None
}

fn native_sides(
    pattern: &Value,
    rest: &[Point2],
    triangles: &[[usize; 3]],
) -> Result<Vec<Vec<usize>>, String> {
    let (outline, corners) = perimeter(pattern)?;
    let mut distinct = Vec::new();
    let mut vertex_slots = Vec::with_capacity(outline.len());
    for p in outline {
        let key = coordinate_key(p);
        if distinct.last() != Some(&key) {
            distinct.push(key);
        }
        vertex_slots.push(distinct.len() - 1);
    }
    if distinct.len() > 1 && distinct.first() == distinct.last() {
        let end = distinct.len() - 1;
        distinct.pop();
        for slot in vertex_slots
            .iter_mut()
            .rev()
            .take_while(|slot| **slot == end)
        {
            *slot = 0;
        }
    }
    if distinct.len() < 3 {
        return Err("saved native pose outline has fewer than three physical samples".into());
    }
    let neighbors = mesh_boundary(rest.len(), triangles)?;
    let rest_keys: Vec<_> = rest.iter().copied().map(coordinate_key).collect();
    let selected = map_boundary_cycle(&distinct, &rest_keys, &neighbors)
        .ok_or("saved native rest mesh boundary does not match the panel outline samples")?;
    let vertex_indices: Vec<_> = vertex_slots
        .into_iter()
        .map(|slot| selected[slot])
        .collect();
    let mut sides = sampled_sides(&vertex_indices, &corners);
    for side in &mut sides {
        side.dedup();
        if side.len() < 2 {
            return Err("saved native pose outline edge has zero physical length".into());
        }
    }
    Ok(sides)
}

#[derive(Clone, Serialize, Deserialize)]
struct NativeShirringBase {
    vertices: usize,
    triangles: Vec<[usize; 3]>,
}

fn transfer_shirred_pose(
    current_uvs: &[Point2],
    edges: &[Edge],
    previous_uvs: &[Point2],
    previous_positions: &[Point],
    previous_velocities: &[Point],
) -> Option<(Vec<Point>, Vec<Point>)> {
    let base = current_uvs.len().checked_sub(edges.len())?;
    if previous_uvs.len() != previous_positions.len()
        || previous_uvs.len() != previous_velocities.len()
        || current_uvs[..base] != *previous_uvs.get(..base)?
    {
        return None;
    }
    let mut positions = previous_positions[..base].to_vec();
    let mut velocities = previous_velocities[..base].to_vec();
    let mut by_uv = HashMap::new();
    for (i, &uv) in previous_uvs.iter().enumerate().skip(base) {
        by_uv
            .entry(coordinate_key(uv))
            .and_modify(|index| *index = None)
            .or_insert(Some(i));
    }
    for (j, &(a, b)) in edges.iter().enumerate() {
        let i = base + j;
        let uv = current_uvs[i];
        let prior = if previous_uvs.get(i) == Some(&uv) {
            Some(i)
        } else {
            by_uv.get(&coordinate_key(uv)).copied().flatten()
        };
        let midpoint = |points: &[Point]| {
            std::array::from_fn(|axis| (points[a][axis] + points[b][axis]) * 0.5)
        };
        positions.push(prior.map_or_else(|| midpoint(&positions), |i| previous_positions[i]));
        velocities.push(prior.map_or_else(|| midpoint(&velocities), |i| previous_velocities[i]));
    }
    Some((positions, velocities))
}

#[derive(Deserialize)]
struct SavedPoseMesh {
    #[serde(default)]
    native_shirring_base: Option<NativeShirringBase>,
    positions: Vec<Point>,
    triangles: Vec<[usize; 3]>,
    texcoords: Vec<Point2>,
}

fn native_mesh(
    pattern: &Value,
    prior: &Value,
    state: &Value,
    saved_pose: bool,
) -> Result<Mesh, String> {
    let saved = SavedPoseMesh::deserialize(prior)
        .map_err(|error| format!("invalid saved native pose mesh: {error}"))?;
    let vertex_count = saved.positions.len();
    if vertex_count < 3 || saved.texcoords.len() != vertex_count {
        return Err("saved native positions and rest coordinate counts do not match".into());
    }
    if saved.triangles.is_empty() || saved.triangles.len() > vertex_count * 4 {
        return Err("saved native pose has an invalid triangle count".into());
    }
    if saved
        .positions
        .iter()
        .flatten()
        .chain(saved.texcoords.iter().flatten())
        .any(|v| !v.is_finite() || v.abs() >= 1e7)
    {
        return Err("saved native position or rest coordinate is invalid".into());
    }
    let (base_vertices, base_triangles) = saved
        .native_shirring_base
        .as_ref()
        .map_or((vertex_count, saved.triangles.as_slice()), |base| {
            (base.vertices, base.triangles.as_slice())
        });
    if base_vertices < 3 || base_vertices > vertex_count || base_triangles.is_empty() {
        return Err("invalid base mesh in saved shirring state".into());
    }
    let mut points = saved.texcoords[..base_vertices].to_vec();
    let mut triangles = base_triangles.to_vec();
    let mut sides = native_sides(pattern, &points, &triangles)?;
    let velocities = if saved_pose {
        vec![[0.0; 3]; vertex_count]
    } else {
        let velocities = Vec::<Point>::deserialize(&prior["velocities"])
            .map_err(|error| format!("invalid Rust continuation velocities: {error}"))?;
        if velocities.len() != vertex_count || velocities.iter().flatten().any(|v| !v.is_finite()) {
            return Err(
                "Rust continuation velocities do not match the saved native pose mesh".into(),
            );
        }
        velocities
    };
    let distance = number(pattern, "particle_distance", 20.0);
    if !(3.0..=500.0).contains(&distance) {
        return Err("individual pattern particle_distance must be 3..500 mm".into());
    }
    let shirring_edges =
        shirring::refine(pattern, &mut points, &mut triangles, &mut sides, distance)?;
    let native_shirring_base = (!shirring_edges.is_empty()).then(|| NativeShirringBase {
        vertices: base_vertices,
        triangles: base_triangles.to_vec(),
    });
    let (positions, velocities) = transfer_shirred_pose(
        &points,
        &shirring_edges,
        &saved.texcoords,
        &saved.positions,
        &velocities,
    )
    .ok_or("native shirring state is inconsistent")?;
    Ok(Mesh {
        id: pattern["id"].clone(),
        name: string(pattern, "name", "Pattern").to_owned(),
        fabric_index: pattern["fabric_index"].clone(),
        positions,
        triangles,
        velocities,
        texcoords: points.clone(),
        points,
        sides,
        material: ClothMaterial::for_panel(pattern, state),
        frozen: boolean(pattern, "freeze", false),
        shirring_edges,
        native_shirring_base,
    })
}

fn native_meshes(
    patterns: &[Value],
    prior_meshes: &[Value],
    state: &Value,
    saved_pose: bool,
) -> Result<Vec<Mesh>, String> {
    if patterns.len() != prior_meshes.len()
        || patterns
            .iter()
            .zip(prior_meshes)
            .any(|(panel, mesh)| panel["id"].is_null() || panel["id"] != mesh["id"])
    {
        return Err(
            "saved native meshes do not match the current panel identities and order".into(),
        );
    }
    let count: usize = prior_meshes
        .iter()
        .map(|mesh| array(&mesh["positions"]).len())
        .sum();
    if count > MAX_SIMULATION_VERTICES {
        return Err("saved native pose exceeds 130000 vertices; supply a new particle_distance to remesh or split the garment".into());
    }
    patterns
        .iter()
        .zip(prior_meshes)
        .map(|(pattern, prior)| {
            native_mesh(pattern, prior, state, saved_pose)
                .map_err(|error| format!("native panel {}: {error}", pattern["name"]))
        })
        .collect()
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
    let constraint = |(a, b): Edge, coefficients| Constraint {
        a: a + offset,
        b: b + offset,
        rest: distance2(mesh.points[a], mesh.points[b]).sqrt(),
        stiffness: ClothMaterial::along(coefficients, mesh.points[a], mesh.points[b]),
    };
    edges
        .into_keys()
        .map(|edge| constraint(edge, mesh.material.stretch))
        .chain(
            bending
                .into_iter()
                .map(|edge| constraint(edge, mesh.material.bend)),
        )
        .collect()
}

fn elastic_constraints(
    mesh: &Mesh,
    pattern: &Value,
    offset: usize,
) -> Result<Vec<Constraint>, String> {
    let properties = &pattern["line_properties"];
    if properties.is_null() {
        return Ok(Vec::new());
    }
    let all = &properties["-1"]["elastic"];
    let segment_length =
        |pair: &[usize]| distance2(mesh.points[pair[0]], mesh.points[pair[1]]).sqrt();
    let perimeter: f64 = mesh
        .sides
        .iter()
        .flat_map(|side| side.windows(2))
        .map(segment_length)
        .sum();
    let all_ratio = all["total_length"]
        .as_f64()
        .map_or(0.8, |target| target / perimeter);
    let mut constraints = Vec::new();
    for (line, side) in mesh.sides.iter().enumerate() {
        let elastic = &properties[line.to_string()]["elastic"];
        if !boolean(elastic, "on", boolean(all, "on", false)) {
            continue;
        }
        let length: f64 = side.windows(2).map(segment_length).sum();
        if length <= 1e-9 {
            continue;
        }
        let ratio = elastic["total_length"]
            .as_f64()
            .map_or(all_ratio, |target| target / length);
        let strength = number(elastic, "strength", number(all, "strength", 10.0));
        if !ratio.is_finite() || ratio < 0.0 || !strength.is_finite() || strength < 0.0 {
            return Err("invalid saved elastic length or strength".into());
        }
        if strength == 0.0 {
            continue;
        }
        // Map the user-visible relative strength to a bounded CPU projection weight.
        let stiffness = strength / (strength + 10.0);
        constraints.extend(side.windows(2).map(|pair| Constraint {
            a: offset + pair[0],
            b: offset + pair[1],
            rest: segment_length(pair) * ratio,
            stiffness,
        }));
    }
    Ok(constraints)
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

fn append_inverse_masses(mesh: &Mesh, use_density: bool, weights: &mut Vec<f64>) {
    let offset = weights.len();
    weights.resize(
        offset + mesh.points.len(),
        if mesh.frozen { 0.0 } else { 1.0 },
    );
    if mesh.frozen || !use_density {
        return;
    }
    let areas = &mut weights[offset..];
    areas.fill(0.0);
    for &[a, b, c] in &mesh.triangles {
        let share = flat_cross(mesh.points[a], mesh.points[b], mesh.points[c]).abs() / 6.0;
        for vertex in [a, b, c] {
            areas[vertex] += share;
        }
    }
    // Source files may contain isolated vertices or zero-area triangles.
    // Preserve their participation with the mean area of the other vertices.
    const MIN_AREA: f64 = 1e-9;
    let (area, count) = areas
        .iter()
        .filter(|&&area| area >= MIN_AREA)
        .fold((0.0, 0), |(sum, count), &area| (sum + area, count + 1));
    let fallback_area = if count > 0 {
        area / f64::from(count)
    } else {
        1.0
    };
    // The CLO default simulation fabric stores 300 g/m².
    let density = mesh.material.density_g_per_mm2.unwrap_or(0.0003);
    for vertex in areas {
        let area = if *vertex >= MIN_AREA {
            *vertex
        } else {
            fallback_area
        };
        *vertex = (area * density).recip();
    }
}

pub fn simulate(args: &Value, state: &mut Value) -> Result<Value, String> {
    let start = std::time::Instant::now();
    let existing = &state["simulation"];
    let unsigned = |key: &str, default: u64| {
        if args[key].is_null() {
            Ok(default)
        } else {
            args[key]
                .as_u64()
                .ok_or_else(|| format!("{key} must be a nonnegative integer"))
        }
    };
    let steps = unsigned("steps", 100)?;
    let distance = number(
        args,
        "particle_distance",
        number(existing, "particle_distance", 20.0),
    );
    let quality = unsigned("quality", existing["quality"].as_u64().unwrap_or(0))?;
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
    let explicit_distance = args["particle_distance"].is_number();
    let saved_pose = existing["source"] == NATIVE_SAVED_POSE;
    let use_native = !explicit_distance && (saved_pose || existing["source"] == RUST_NATIVE_REST);
    let prior_meshes = array(&existing["meshes"]);
    let (mut meshes, resume) = if use_native {
        (
            native_meshes(patterns, prior_meshes, state, saved_pose)?,
            !saved_pose,
        )
    } else {
        let mut meshes = patterns
            .iter()
            .map(|pattern| {
                let target = if explicit_distance {
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
        let mut resume = (distance - number(existing, "particle_distance", -1.0)).abs() < 1e-9
            && meshes.len() == prior_meshes.len()
            && meshes
                .iter()
                .zip(prior_meshes)
                .all(|(mesh, prior)| mesh.id == prior["id"]);
        if resume {
            let mut transferred = Vec::with_capacity(meshes.len());
            for (mesh, prior) in meshes.iter().zip(prior_meshes) {
                let saved = SavedPoseMesh::deserialize(prior)
                    .map_err(|error| format!("invalid saved simulation mesh: {error}"))?;
                let velocities = Vec::<Point>::deserialize(&prior["velocities"])
                    .map_err(|error| format!("invalid saved simulation velocities: {error}"))?;
                if saved.positions.len() != velocities.len() {
                    return Err("saved simulation velocities are inconsistent".into());
                }
                let Some(data) = transfer_shirred_pose(
                    &mesh.texcoords,
                    &mesh.shirring_edges,
                    &saved.texcoords,
                    &saved.positions,
                    &velocities,
                ) else {
                    resume = false;
                    break;
                };
                transferred.push(data);
            }
            if resume {
                for (mesh, (positions, velocities)) in meshes.iter_mut().zip(transferred) {
                    mesh.positions = positions;
                    mesh.velocities = velocities;
                }
            }
        }
        (meshes, resume)
    };
    let primary_avatar = &state["avatar"];
    let use_saved_avatar_proxy = |avatar: &Value| use_native && !avatar["source_project"].is_null();
    let mesh_geometry = |avatar: &Value| {
        (!avatar["mesh"].is_null() && !use_saved_avatar_proxy(avatar))
            .then(|| avatar_geometry(avatar))
            .into_iter()
            .flatten()
    };
    let primary_triangles = MeshCollider::new(mesh_geometry(primary_avatar));
    let additional_triangles = MeshCollider::new(
        array(&state["additional_avatars"])
            .iter()
            .flat_map(mesh_geometry),
    );
    let colliders = [primary_triangles.as_ref(), additional_triangles.as_ref()];
    let body = if primary_triangles.is_some() {
        &Value::Null
    } else {
        &primary_avatar["body"]
    };
    let proxies = if body.is_null() { vec![] } else { limbs(body) };
    let mut offsets = Vec::new();
    let mut positions = Vec::new();
    let mut velocities = Vec::new();
    let mut weights = Vec::new();
    let mut constraints = Vec::new();
    let mut elastic = Vec::new();
    let use_density = meshes
        .iter()
        .any(|mesh| mesh.material.density_g_per_mm2.is_some());
    for (mesh, pattern) in meshes.iter().zip(patterns) {
        let offset = positions.len();
        offsets.push(offset);
        constraints.extend(internal_constraints(mesh, offset));
        elastic.extend(elastic_constraints(mesh, pattern, offset)?);
        positions.extend(mesh.positions.iter().copied());
        velocities.extend(mesh.velocities.iter().copied());
        append_inverse_masses(mesh, use_density, &mut weights);
    }
    if positions.len() > MAX_SIMULATION_VERTICES {
        return Err("simulation scene exceeds 130000 vertices; increase particle_distance or split the garment".into());
    }
    let seam_constraints = stitches(state, &meshes, &offsets)?;
    let native_stitches = native_stitches::build(state, &meshes, &offsets, &weights);
    let iterations = match quality {
        0 => 4,
        1 => 7,
        _ => 12,
    };
    let dt = 1.0 / 30.0;
    let mut contacts = 0usize;
    let use_mesh_collision = colliders.iter().any(Option::is_some);
    let mut start_of_iteration = if use_mesh_collision {
        positions.clone()
    } else {
        Vec::new()
    };
    let reserve_contact = |start: Point, end: &mut Point| {
        usize::from(MeshCollider::sweep(
            colliders.iter().flatten().copied(),
            start,
            end,
            0.5,
        ))
    };
    let using_proxy = !body.is_null();
    let mut initial_overlap_corrections = 0;
    if steps > 0 {
        for collider in colliders.iter().flatten() {
            initial_overlap_corrections +=
                collider.separate_initial(&mut positions, &weights, &mut velocities, 0.5);
        }
    }
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
        if use_mesh_collision {
            for ((p, &start), &weight) in positions.iter_mut().zip(&old).zip(&weights) {
                if weight > 0.0 {
                    contacts += reserve_contact(start, p);
                }
            }
        }
        for _ in 0..iterations {
            if use_mesh_collision {
                start_of_iteration.copy_from_slice(&positions);
            }
            for c in constraints.iter().chain(&elastic).chain(&seam_constraints) {
                apply_constraint(&mut positions, &weights, c);
            }
            for stitch in &native_stitches.constraints {
                stitch.apply(&mut positions, &weights);
            }
            for (i, (p, &weight)) in positions.iter_mut().zip(&weights).enumerate() {
                if weight > 0.0 {
                    project_proxy(p, body, &proxies);
                    if use_mesh_collision {
                        contacts += reserve_contact(start_of_iteration[i], p);
                    }
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
        .chain(
            native_stitches
                .constraints
                .iter()
                .map(|stitch| stitch.gap(&positions)),
        )
        .fold(0.0, f64::max);
    let effective = constraints.iter().map(|c| c.rest).fold(0.0, f64::max);
    for (mesh, &offset) in meshes.iter_mut().zip(&offsets) {
        let end = offset + mesh.positions.len();
        mesh.positions.copy_from_slice(&positions[offset..end]);
        mesh.velocities.copy_from_slice(&velocities[offset..end]);
    }
    let previous_steps = if resume {
        existing["steps"].as_u64().unwrap_or(0)
    } else {
        0
    };
    let total = steps
        .checked_add(previous_steps)
        .ok_or("saved simulation step count overflow")?;
    if explicit_distance {
        if let Some(patterns) = state["patterns"].as_array_mut() {
            for pattern in patterns {
                pattern["particle_distance"] = json!(distance);
            }
        }
    }
    let shirring_vertices: usize = meshes.iter().map(|mesh| mesh.shirring_edges.len()).sum();
    state["simulation"] = json!({"steps":total,"quality":quality,"meshes":meshes});
    if use_native {
        state["simulation"]["source"] = json!(RUST_NATIVE_REST);
        state["simulation"]["texcoords_units"] = json!("mm");
    } else {
        state["simulation"]["particle_distance"] = json!(distance);
    }
    let mut limitations = vec![
        "self-collision unavailable".to_owned(),
        "fabric and edge properties are partially implemented".to_owned(),
        "no CLO material calibration".to_owned(),
    ];
    if use_mesh_collision {
        limitations.push("Static mesh contacts sweep cloth vertices; initial penetration correction requires a closed avatar mesh. Moving avatars are unavailable.".into());
    }
    if using_proxy {
        limitations.push("The primary avatar uses an analytic collision proxy.".into());
    }
    if use_native {
        limitations.push("Native topology and saved drape initialize the independent CPU solver. Official CLO solver velocities and state are unavailable.".into());
    }
    if native_stitches.unsupported > 0 {
        limitations.push(format!(
            "Unsupported or unverified native seam groups omitted from constraints: {}",
            native_stitches.unsupported
        ));
    }
    let mut result = json!({"ok":true,"steps":steps,"seconds":start.elapsed().as_secs_f64(),"solver":"rust_cpu_pbd",
        "vertices":positions.len(),"stitch_count":seam_constraints.len()+native_stitches.constraints.len(),"max_stitch_gap_mm":gap,
        "elastic_constraint_count":elastic.len(),"shirring_refined_vertices":shirring_vertices,"mesh_collision_contacts":contacts,"mesh_initial_overlap_corrections":initial_overlap_corrections,"native_interval_seam_groups":native_stitches.groups,"native_unsupported_seam_groups":native_stitches.unsupported,
        "max_constraint_length_mm":effective,"limitations":limitations});
    if use_native {
        result["initialization"] = json!(if saved_pose {
            "native_saved_pose_zero_velocity"
        } else {
            "native_rest_rust_velocities"
        });
    }
    Ok(result)
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

#[cfg(test)]
mod native_pose_tests {
    use super::*;

    #[test]
    fn native_material_uses_separate_axial_and_bias_coefficients() {
        let state = json!({"fabrics":[{
            "index": 1, "stiffness": 0.8, "info":{"physical_properties":{
                "stretch_u":20000, "stretch_v":60000, "shear":5000,
                "bend_u":2000, "bend_v":6000, "bend_bias":500
            }}
        }]});
        let pattern = json!({"fabric_index": 1});
        let material = ClothMaterial::for_panel(&pattern, &state);
        let origin = [0.0, 0.0];
        for (axis, stretch, bend) in [
            ([100.0, 0.0], 0.5, 0.06),
            ([0.0, 100.0], 0.75, 0.09),
            ([100.0, 100.0], 0.2, 0.024),
        ] {
            assert!((ClothMaterial::along(material.stretch, origin, axis) - stretch).abs() < 1e-12);
            assert!((ClothMaterial::along(material.bend, origin, axis) - bend).abs() < 1e-12);
        }
        let legacy = ClothMaterial::for_panel(&pattern, &Value::Null);
        assert_eq!(legacy.stretch, [0.9; 3]);
        assert_eq!(legacy.bend, [0.9 * 0.12; 3]);
    }

    #[test]
    fn elastic_total_length_is_distributed_and_disabled_edges_are_excluded() {
        let pattern = json!({
            "id": 1,
            "points": [[0, 0], [100, 0], [100, 50], [0, 50]],
            "line_properties": {
                "-1": {"elastic": {"on": true, "total_length": 180, "strength": 20}},
                "1": {"elastic": {"on": false}},
                "2": {"elastic": {"on": true, "total_length": 50, "strength": 5}}
            }
        });
        let mesh = new_mesh(&pattern, 17.0, &Value::Null).unwrap();
        let constraints = elastic_constraints(&mesh, &pattern, 0).unwrap();
        let sum: f64 = constraints.iter().map(|entry| entry.rest).sum();
        assert!((sum - 140.0).abs() < 1e-9);
        assert!(constraints
            .iter()
            .any(|entry| (entry.stiffness - 1.0 / 3.0).abs() < 1e-9));
        assert!(constraints
            .iter()
            .any(|entry| (entry.stiffness - 2.0 / 3.0).abs() < 1e-9));
    }

    fn fixture() -> Value {
        let outline = json!([
            [0.0, 0.0],
            [0.0, -0.0, 2],
            [50.0, 0.0, 2],
            [100.0, 0.0],
            [100.0, 100.0],
            [0.0, 100.0]
        ]);
        // Indices 0 and 4 share an XY coordinate. Only 4 belongs to the outline.
        let rest = [
            [0.0, 0.0],
            [100.0, 0.0],
            [100.0, 100.0],
            [0.0, 100.0],
            [0.0, -0.0],
            [-40.0, -40.0],
            [-40.0, 0.0],
            [50.0, 0.0],
        ];
        let triangles = [[4, 7, 3], [7, 2, 3], [7, 1, 2], [0, 5, 6]];
        let mesh = |id: usize, z: f64| {
            json!({
                "id":id,
                "positions":rest.iter().map(|[x,y]|[*x,*y+240.0,z]).collect::<Vec<_>>(),
                "triangles":triangles,"texcoords":rest,
                // Archived values must never become velocity inputs for the independent solver.
                "velocities":vec![[11.0,13.0,17.0];8]
            })
        };
        json!({
            "patterns":[{"id":1,"name":"A","points":outline},{"id":2,"name":"B","points":outline}],
            "seams":[{"pattern_a":1,"line_a":0,"pattern_b":2,"line_b":0,"dir_a":true,"dir_b":true}],
            "simulation":{"source":NATIVE_SAVED_POSE,"meshes":[mesh(1,147.0),mesh(2,152.0)],"steps":120},
            "native_seam_groups":[{"id":31,"restored":false}]
        })
    }

    #[test]
    fn saved_mesh_uses_initial_positions_and_resumes_only_rust_velocities() {
        let saved = fixture();
        let mut state = saved.clone();
        let first = simulate(&json!({"steps":0}), &mut state).unwrap();
        assert_eq!(first["initialization"], "native_saved_pose_zero_velocity");
        assert_eq!(first["vertices"], 16);
        assert_eq!(first["stitch_count"], 3);
        assert_eq!(first["native_unsupported_seam_groups"], 1);
        assert_eq!(state["simulation"]["source"], RUST_NATIVE_REST);
        assert_eq!(state["simulation"]["texcoords_units"], "mm");
        assert_eq!(state["simulation"]["steps"], 0);
        assert!(state["simulation"]["particle_distance"].is_null());
        for i in 0..2 {
            let restored = &state["simulation"]["meshes"][i];
            let original = &saved["simulation"]["meshes"][i];
            for property in ["positions", "texcoords", "triangles"] {
                assert_eq!(restored[property], original[property]);
            }
            assert_eq!(restored["velocities"], json!(vec![[0.0; 3]; 8]));
        }
        let a = native_mesh(
            &state["patterns"][0],
            &state["simulation"]["meshes"][0],
            &state,
            false,
        )
        .unwrap();
        assert_eq!(a.sides[0], [4, 7, 1]);
        let continued = simulate(&json!({"steps":1}), &mut state).unwrap();
        assert_eq!(continued["initialization"], "native_rest_rust_velocities");
        assert_ne!(
            state["simulation"]["meshes"][0]["positions"],
            saved["simulation"]["meshes"][0]["positions"]
        );
        assert_ne!(
            state["simulation"]["meshes"][0]["velocities"],
            json!(vec![[0.0; 3]; 8])
        );
        simulate(&json!({"steps":1}), &mut state).unwrap();
        let mut single_run = saved.clone();
        simulate(&json!({"steps":2}), &mut single_run).unwrap();
        assert_eq!(state["simulation"], single_run["simulation"]);
    }

    #[test]
    fn rejects_bad_native_topology_and_allows_explicit_remesh() {
        let mut state = fixture();
        state["simulation"]["meshes"][0]["triangles"][0][0] = json!(999);
        let before = state.clone();
        let error = simulate(&json!({"steps":1}), &mut state).unwrap_err();
        assert!(error.contains("triangle has invalid vertex indices"));
        assert_eq!(before, state);
        let mut state = fixture();
        state["patterns"][1]["points"][2][0] = json!(53.0);
        let error = simulate(&json!({"steps":0}), &mut state).unwrap_err();
        assert!(error.contains("boundary does not match"));
        let normal_outline = json!([[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]]);
        state["patterns"][0]["points"] = normal_outline.clone();
        state["patterns"][1]["points"] = normal_outline;
        let outcome = simulate(&json!({"steps":0,"particle_distance":200.0}), &mut state).unwrap();
        assert!(outcome["initialization"].is_null());
        assert_eq!(outcome["vertices"], 8);
        assert_eq!(state["simulation"]["particle_distance"], 200.0);
        assert!(state["simulation"]["source"].is_null());
        assert_ne!(
            state["simulation"]["meshes"][0]["positions"],
            before["simulation"]["meshes"][0]["positions"]
        );
    }
}
