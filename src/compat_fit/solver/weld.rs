use super::{edge, mixed_meshes, native_sides, number, split_marked_edges, Mesh};
use crate::compat_fit::{array, array_mut};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

// Garment drafting supplies two congruent, horizontal waist edges with opposing
// perimeter directions. Refine their boundaries to the union of both samples
// before sharing vertices so that the joined mesh has no T-junctions.
fn waist_samples(mesh: &Mesh, line: usize) -> Result<Vec<f64>, String> {
    let side = mesh.sides.get(line).ok_or("missing waist mesh boundary")?;
    let height = mesh.points[side[0]][1];
    if side.iter().any(|&i| mesh.points[i][1] != height) {
        return Err("waist merge requires a straight horizontal join".into());
    }
    let mut samples: Vec<_> = side.iter().map(|&i| mesh.points[i][0]).collect();
    samples.sort_by(f64::total_cmp);
    samples.dedup();
    Ok(samples)
}

fn refine_waist(mesh: &mut Mesh, line: usize, samples: &[f64]) -> Result<Vec<usize>, String> {
    let mut vertices = BTreeMap::new();
    for &vertex in &mesh.sides[line] {
        let x = mesh.points[vertex][0];
        let slot = samples
            .binary_search_by(|other| other.total_cmp(&x))
            .map_err(|_| "waist sample is missing")?;
        vertices.insert(slot, vertex);
    }
    for (slot, &x) in samples.iter().enumerate() {
        if vertices.contains_key(&slot) {
            continue;
        }
        let &a = vertices
            .range(..slot)
            .next_back()
            .ok_or("waist starts differ")?
            .1;
        let &b = vertices.range(slot..).next().ok_or("waist ends differ")?.1;
        let t = (x - mesh.points[a][0]) / (mesh.points[b][0] - mesh.points[a][0]);
        let i = mesh.points.len();
        let interpolate = |values: &[[f64; 3]]| {
            std::array::from_fn(|axis| values[a][axis] + (values[b][axis] - values[a][axis]) * t)
        };
        mesh.points.push([x, mesh.points[a][1]]);
        mesh.positions.push(interpolate(&mesh.positions));
        mesh.velocities.push(interpolate(&mesh.velocities));
        split_marked_edges(
            &mut mesh.triangles,
            &mut mesh.sides,
            &BTreeMap::from([(edge(a, b), i)]),
        );
        vertices.insert(slot, i);
    }
    Ok(vertices.into_values().collect())
}

fn stitch_mesh(
    mut upper: Mesh,
    mut lower: Mesh,
    upper_line: usize,
    lower_line: usize,
    pattern: &Value,
) -> Result<Value, String> {
    let a = waist_samples(&upper, upper_line)?;
    let b = waist_samples(&lower, lower_line)?;
    if a.len() < 2
        || a.first() != b.first()
        || a.last() != b.last()
        || upper.points[upper.sides[upper_line][0]][1]
            != lower.points[lower.sides[lower_line][0]][1]
    {
        return Err("waist boundaries do not coincide in rest coordinates".into());
    }
    let mut samples = a;
    samples.extend(b);
    samples.sort_by(f64::total_cmp);
    samples.dedup();
    let upper_indices = refine_waist(&mut upper, upper_line, &samples)?;
    let lower_indices = refine_waist(&mut lower, lower_line, &samples)?;
    let mut remap = vec![None; lower.points.len()];
    for (a, b) in upper_indices.into_iter().zip(lower_indices) {
        remap[b] = Some(a);
        for axis in 0..3 {
            upper.positions[a][axis] = (upper.positions[a][axis] + lower.positions[b][axis]) * 0.5;
            upper.velocities[a][axis] =
                (upper.velocities[a][axis] + lower.velocities[b][axis]) * 0.5;
        }
    }
    for (i, mapped) in remap.iter_mut().enumerate() {
        if mapped.is_none() {
            *mapped = Some(upper.points.len());
            upper.points.push(lower.points[i]);
            upper.positions.push(lower.positions[i]);
            upper.velocities.push(lower.velocities[i]);
        }
    }
    upper.triangles.extend(
        lower
            .triangles
            .iter()
            .map(|triangle| triangle.map(|i| remap[i].unwrap())),
    );
    // Validate the final outline against the welded topology before publishing either change.
    native_sides(pattern, &upper.points, &upper.triangles)?;
    Ok(
        json!({"id":upper.id,"name":upper.name,"fabric_index":upper.fabric_index,
        "positions":upper.positions,"velocities":upper.velocities,"triangles":upper.triangles,
        "texcoords":upper.points,"texcoords_units":"mm"}),
    )
}

fn splice_outline(
    upper: &Value,
    lower: &Value,
    upper_line: usize,
    lower_line: usize,
) -> Result<(Value, [Vec<Option<usize>>; 2]), String> {
    let panels = [upper, lower];
    let records = panels.map(|p| array(&p["points"]));
    let corners = records.map(|points| {
        points
            .iter()
            .enumerate()
            .filter_map(|(i, point)| (point[2].as_u64().unwrap_or(0) == 0).then_some(i))
            .collect::<Vec<_>>()
    });
    let [a, b] = corners.each_ref().map(Vec::len);
    if upper_line >= a || lower_line >= b {
        return Err("waist seam line is outside garment outline".into());
    }
    let lines = (0..upper_line)
        .map(|i| (0, i))
        .chain((1..b).map(|i| (1, (i + lower_line) % b)))
        .chain((upper_line + 1..a).map(|i| (0, i)));
    let mut map = [vec![None; a], vec![None; b]];
    let mut merged = upper.clone();
    let mut points = Vec::new();
    let mut edges = Vec::new();
    let mut properties = Map::new();
    let mut elastic = Vec::new();
    for (index, (piece, line)) in lines.enumerate() {
        map[piece][line] = Some(index);
        let from = corners[piece][line];
        let to = corners[piece]
            .get(line + 1)
            .copied()
            .unwrap_or(records[piece].len());
        points.extend_from_slice(&records[piece][from..to]);
        let mut label = panels[piece]["edges"][line].clone();
        label["line"] = json!(index);
        edges.push(label);
        if let Some(value) = panels[piece]["line_properties"].get(line.to_string()) {
            properties.insert(index.to_string(), value.clone());
        }
        for entry in array(&panels[piece]["elastic"])
            .iter()
            .filter(|entry| entry["line"] == line)
        {
            let mut entry = entry.clone();
            entry["line"] = json!(index);
            elastic.push(entry);
        }
    }
    merged["points"] = json!(points);
    merged["edges"] = json!(edges);
    merged["line_properties"] = json!(properties);
    merged["elastic"] = json!(elastic);
    Ok((merged, map))
}

pub(in crate::compat_fit) fn merge_drafted_pair(
    state: &mut Value,
    base: &Value,
    other: &Value,
) -> Result<(), String> {
    let panels = array(&state["patterns"]);
    let find = |id: &Value| {
        panels
            .iter()
            .position(|panel| panel["id"] == *id)
            .ok_or("cannot find drafted panel for waist merge")
    };
    let upper = find(base)?;
    let lower = find(other)?;
    let seams = array(&state["seams"]);
    let join = seams
        .iter()
        .position(|seam| seam["pattern_a"] == *base && seam["pattern_b"] == *other)
        .ok_or("cannot find the drafted waist seam")?;
    let line = |key: &str| {
        seams[join][key]
            .as_u64()
            .map(|value| value as usize)
            .ok_or("missing waist line")
    };
    let (line_a, line_b) = (line("line_a")?, line("line_b")?);
    let (pattern, maps) = splice_outline(&panels[upper], &panels[lower], line_a, line_b)?;
    let old = &state["simulation"];
    let prior = array(&old["meshes"]);
    let target = number(old, "particle_distance", 20.0);
    let rebuild = |index| -> Result<Mesh, String> {
        mixed_meshes(state, &panels[index..index + 1], prior, target, false)?
            .pop()
            .ok_or_else(|| "cannot rebuild the drafted waist mesh".into())
    };
    let new_mesh = stitch_mesh(rebuild(upper)?, rebuild(lower)?, line_a, line_b, &pattern)?;
    let mut redirected = Vec::new();
    for (i, seam) in seams.iter().enumerate() {
        if i == join {
            continue;
        }
        let mut next = seam.clone();
        for (id_key, line_key) in [("pattern_a", "line_a"), ("pattern_b", "line_b")] {
            let mapping = if seam[id_key] == *base {
                &maps[0]
            } else if seam[id_key] == *other {
                &maps[1]
            } else {
                continue;
            };
            let index = seam[line_key]
                .as_u64()
                .and_then(|line| mapping.get(line as usize).copied().flatten())
                .ok_or("another seam references the internal waist line")?;
            next[id_key] = base.clone();
            next[line_key] = json!(index);
        }
        redirected.push(next);
    }
    state["patterns"][upper] = pattern;
    array_mut(state, "patterns").remove(lower);
    state["seams"] = json!(redirected);
    let meshes = array_mut(&mut state["simulation"], "meshes");
    meshes.retain(|mesh| mesh["id"] != *other);
    for mesh in meshes.iter_mut().filter(|mesh| mesh["id"] == *base) {
        *mesh = new_mesh.clone();
    }
    state["simulation"]["source"] = json!("mixed_saved_pose");
    Ok(())
}
