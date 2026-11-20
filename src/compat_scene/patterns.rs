use super::{array, flag, index, invalidate_geometry, next_id, number, string, use_partial_pose, vec_mut, Reply};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub x: f64,
    pub y: f64,
    pub kind: u64,
}

impl Point {
    pub fn from_json(point: &Value) -> Result<Self, String> {
        let x = point.get(0).and_then(Value::as_f64);
        let y = point.get(1).and_then(Value::as_f64);
        let kind = point.get(2).map_or(Some(0), |v| {
            v.as_f64()
                .filter(|k| [0., 2., 3.].contains(k))
                .map(|k| k as u64)
        });
        match (x, y, kind) {
            (Some(x), Some(y), Some(kind)) if x.is_finite() && y.is_finite() => {
                Ok(Self { x, y, kind })
            }
            _ => Err(
                "point must be [x, y] or [x, y, type] with finite coordinates and type 0, 2 or 3"
                    .into(),
            ),
        }
    }

    fn position(self) -> Value {
        json!({"x": self.x, "y": self.y})
    }

    pub fn record(self) -> Value {
        if self.kind == 0 {
            json!([self.x, self.y])
        } else {
            json!([self.x, self.y, self.kind])
        }
    }

    fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

pub(super) fn points(pattern: &Value) -> Result<Vec<Point>, String> {
    let result: Vec<_> = pattern["points"]
        .as_array()
        .ok_or("points must be an array")?
        .iter()
        .map(Point::from_json)
        .collect::<Result<_, _>>()?;
    if result.iter().filter(|p| p.kind == 0).count() < 3 {
        return Err("need at least 3 corner points".into());
    }
    Ok(result)
}

fn corner_offsets(points: &[Point]) -> Vec<usize> {
    points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| (p.kind == 0).then_some(i))
        .collect()
}

fn split_edges(points: &[Point]) -> Vec<Vec<Point>> {
    let corners = corner_offsets(points);
    (0..corners.len())
        .map(|i| {
            let start = corners[i];
            let end = corners[(i + 1) % corners.len()];
            if end > start {
                points[start..=end].to_vec()
            } else {
                points[start..]
                    .iter()
                    .chain(&points[..=end])
                    .copied()
                    .collect()
            }
        })
        .collect()
}

fn bezier(points: &[Point], t: f64) -> Point {
    let mut work = points.to_vec();
    for n in (1..points.len()).rev() {
        for i in 0..n {
            let next = work[i + 1];
            work[i].x = work[i].x * (1.0 - t) + next.x * t;
            work[i].y = work[i].y * (1.0 - t) + next.y * t;
        }
    }
    work[0]
}

fn curve(edge: &[Point], sampled: bool) -> Vec<Point> {
    if sampled || edge.len() < 3 {
        return edge.to_vec();
    }
    if edge[1..edge.len() - 1].iter().all(|p| p.kind == 3) {
        return (0..=32)
            .map(|i| bezier(edge, f64::from(i) / 32.0))
            .collect();
    }
    let mut result = Vec::new();
    for i in 0..edge.len() - 1 {
        let a = edge[i.saturating_sub(1)];
        let b = edge[i];
        let c = edge[i + 1];
        let d = edge[(i + 2).min(edge.len() - 1)];
        for step in 0..16 {
            let t = f64::from(step) / 16.0;
            let evaluate = |a: f64, b: f64, c: f64, d: f64| {
                0.5 * ((2.0 * b)
                    + (c - a) * t
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * t * t
                    + (-a + 3.0 * b - 3.0 * c + d) * t * t * t)
            };
            result.push(Point {
                x: evaluate(a.x, b.x, c.x, d.x),
                y: evaluate(a.y, b.y, c.y, d.y),
                kind: 0,
            });
        }
    }
    result.push(edge[edge.len() - 1]);
    result
}

pub(super) fn curve_length(edge: &[Point], sampled: bool) -> f64 {
    curve(edge, sampled)
        .windows(2)
        .map(|p| p[0].distance(p[1]))
        .sum()
}

pub(super) fn outline(pattern: &Value) -> Result<Vec<Point>, String> {
    let mut result = Vec::new();
    for edge in split_edges(&points(pattern)?) {
        let sampled = curve(&edge, flag(pattern, "sampled_polyline", false));
        result.extend_from_slice(&sampled[..sampled.len() - 1]);
    }
    Ok(result)
}

fn bounds(pattern: &Value) -> Result<[f64; 4], String> {
    Ok(point_bounds(outline(pattern)?, offset(pattern)))
}

fn point_bounds(points: impl IntoIterator<Item = Point>, [dx, dy]: [f64; 2]) -> [f64; 4] {
    points.into_iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[left, bottom, right, top], p| {
            [
                left.min(p.x + dx),
                bottom.min(p.y + dy),
                right.max(p.x + dx),
                top.max(p.y + dy),
            ]
        },
    )
}

fn bbox(bounds: [f64; 4]) -> Value {
    let [left, bottom, right, top] = bounds;
    json!({"Width": format!("{:.6}", right - left), "Height": format!("{:.6}", top - bottom)})
}

fn slot(n: f64, before: &'static str, after: &'static str) -> &'static str {
    if n < 0.34 {
        before
    } else if n > 0.66 {
        after
    } else {
        ""
    }
}

fn lines(pattern: &Value) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    let [dx, dy] = offset(pattern);
    let points = points(pattern)?;
    let [left, bottom, right, top] =
        point_bounds(points.iter().copied().filter(|p| p.kind == 0), [dx, dy]);
    for (line, edge) in split_edges(&points).iter().enumerate() {
        let start = edge[0];
        let end = edge[edge.len() - 1];
        let length = pattern["native_outline_metrics"][line]["length"]
            .as_f64()
            .unwrap_or_else(|| curve_length(edge, flag(pattern, "sampled_polyline", false)));
        let mid = Point {
            x: (start.x + end.x) / 2.0,
            y: (start.y + end.y) / 2.0,
            kind: 0,
        };
        let norm_x = if right > left {
            (mid.x + dx - left) / (right - left)
        } else {
            0.5
        };
        let norm_y = if top > bottom {
            (mid.y + dy - bottom) / (top - bottom)
        } else {
            0.5
        };
        let vertical = slot(norm_y, "bottom", "top");
        let horizontal = slot(norm_x, "left", "right");
        let location = match (vertical, horizontal) {
            ("", "") => "center".into(),
            ("", side) | (side, "") => side.into(),
            (first, second) => format!("{first}-{second}"),
        };
        let vx = (end.x - start.x).abs();
        let vy = (end.y - start.y).abs();
        let orientation = if vx >= 2. * vy {
            "horizontal"
        } else if vy >= 2. * vx {
            "vertical"
        } else {
            "diagonal"
        };
        let world = |p: Point| {
            Point {
                x: p.x + dx,
                y: p.y + dy,
                ..p
            }
            .position()
        };
        result.push(json!({
            "lineIndex": line, "length": length, "location": location,
            "start": world(start), "end": world(end), "mid": world(mid),
            "midNorm": {"x": norm_x, "y": norm_y}, "orientation": orientation
        }));
    }
    Ok(result)
}

pub(super) fn pattern_index(state: &Value, reference: &Value) -> Result<usize, String> {
    let patterns = array(state, "patterns");
    if let Some(index) = reference
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .or_else(|| reference.as_str().and_then(|s| s.parse().ok()))
    {
        return (index < patterns.len())
            .then_some(index)
            .ok_or_else(|| format!("pattern index {index} is out of range"));
    }
    let name = reference
        .as_str()
        .ok_or("pattern reference must be a name or index")?;
    patterns
        .iter()
        .position(|p| p["name"] == name)
        .ok_or_else(|| {
            let names: Vec<_> = patterns.iter().filter_map(|p| p["name"].as_str()).collect();
            format!("no pattern named {name:?} (have {names:?})")
        })
}

fn group_indices(state: &Value, pattern: &Value) -> Vec<usize> {
    super::annotations::indices(state, &pattern["id"])
}

fn arrangement<'a>(state: &'a Value, name: &str) -> Result<(usize, &'a Value), String> {
    let choices = array(&state["avatar"], "arrangement_points");
    choices
        .iter()
        .enumerate()
        .find(|(_, choice)| {
            choice.as_str() == Some(name) || choice["ArrangementName"].as_str() == Some(name)
        })
        .ok_or_else(|| format!("unknown arrangement point {name:?}"))
}

fn arrangement_details(state: &Value, name: &str) -> Value {
    match arrangement(state, name) {
        Ok((_, choice)) if choice.is_object() => choice.clone(),
        Ok((_, _)) => json!({"ArrangementName": name}),
        Err(_) if !name.is_empty() => json!({"ArrangementName":name}),
        Err(_) => Value::Null,
    }
}

pub(super) fn summary(pattern: &Value, i: usize) -> Value {
    json!({
        "index": i, "name": pattern["name"], "arrangement": pattern["arrangement"].as_str().unwrap_or(""),
        "fabric_index": pattern["fabric_index"].as_u64().unwrap_or(0)
    })
}

pub(super) fn list(args: &Value, state: &mut Value) -> Reply {
    let mut results = Vec::new();
    for (i, pattern) in array(state, "patterns").iter().enumerate() {
        let mut entry = summary(pattern, i);
        let limits = bounds(pattern)?;
        entry["bbox"] = bbox(limits);
        entry["pos_2d"] = json!(centre(limits));
        if flag(args, "detail", false) {
            entry["lines"] = Value::Array(lines(pattern)?.into_iter().map(|edge| {
                json!({"line": edge["lineIndex"], "length": (edge["length"].as_f64().unwrap_or(0.) * 10.).round() / 10.,
                    "location": edge["location"], "start": edge["start"], "end": edge["end"]})
            }).collect());
            entry["seam_groups"] = json!(group_indices(state, pattern));
        }
        results.push(entry);
    }
    Ok(
        json!({"count": results.len(), "patterns": results, "seam_groups": super::annotations::count(state)}),
    )
}

fn offset(pattern: &Value) -> [f64; 2] {
    let position = &pattern["pos_2d"];
    [
        position[0].as_f64().unwrap_or(0.),
        position[1].as_f64().unwrap_or(0.),
    ]
}

fn centre([left, bottom, right, top]: [f64; 4]) -> [f64; 2] {
    [(left + right) / 2., (bottom + top) / 2.]
}

fn input_points(pts: impl IntoIterator<Item = Point>, [dx, dy]: [f64; 2]) -> Vec<Value> {
    let mut records = vec![Value::Null];
    records.extend(pts.into_iter().enumerate().map(|(n, p)| {
        json!({"Point positionX": p.x + dx, "Point positionY": p.y + dy, "Point index": n.to_string()})
    }));
    records[0] = json!({"Point count": (records.len() - 1).to_string()});
    records
}

fn input_line(index: usize, length: &Value, straight: bool) -> Value {
    let kind = if straight { "Straight type" } else { "" };
    json!({"Line index": index.to_string(), "Line length": length, "Line type": kind})
}

fn input_children(pattern: &Value) -> Result<Vec<Value>, String> {
    let Some(children) = pattern["native_children"].as_array() else {
        return Ok(vec![json!({"Children shape count": "0"})]);
    };
    // CLO includes the parent shape in this count.
    let mut records = vec![json!({"Children shape count": (children.len() + 1).to_string()})];
    for (index, child) in children.iter().enumerate() {
        let points = array(child, "points")
            .iter()
            .map(Point::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let lines = array(child, "lines");
        let mut line_list = vec![json!({"Line count": lines.len().to_string()})];
        line_list.extend(lines.iter().enumerate().map(|(i, line)| {
            input_line(i, &line["length"], flag(line, "straight", false))
        }));
        records.push(json!({
            "Shape index": index.to_string(), "Shape name": pattern["name"],
            "Shape type": child["type"], "PointList": input_points(points, offset(pattern)),
            "LineList": line_list,
        }));
    }
    Ok(records)
}

pub(crate) fn internal_strokes(pattern: &Value) -> Result<Vec<Vec<[f64; 3]>>, String> {
    let [dx, dy] = offset(pattern);
    array(pattern, "native_children")
        .iter()
        .filter(|child| child["type"] == "InnerShape" && child["visible"].as_bool() != Some(false))
        .flat_map(|child| array(child, "lines"))
        .filter_map(|line| line["points"].as_array())
        .map(|points| {
            let points = points.iter().map(Point::from_json).collect::<Result<Vec<_>, _>>()?;
            Ok(curve(&points, false).into_iter().map(|p| [p.x + dx, p.y + dy, 0.0]).collect())
        })
        .collect()
}

pub(super) fn info(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let pattern = &state["patterns"][i];
    let [dx, dy] = offset(pattern);
    let pts = points(pattern)?;
    let records = input_points(pts.iter().copied().filter(|p| p.kind == 0), [dx, dy]);
    let edges = split_edges(&pts);
    let lines = lines(pattern)?;
    let mut line_list = vec![json!({"Line count": lines.len().to_string()})];
    line_list.extend(lines.iter().zip(edges).enumerate().map(|(n, (line, edge))| {
        let straight = pattern["native_outline_metrics"][n]["straight"]
            .as_bool()
            .unwrap_or(edge.len() == 2);
        input_line(n, &line["length"], straight)
    }));
    let limits = bounds(pattern)?;
    let [left, bottom, right, top] =
        point_bounds(pts.iter().copied().filter(|p| p.kind == 0), [dx, dy]);
    let arrangement = if pattern["saved_arrangement"].is_object()
        && pattern["saved_arrangement"]["ArrangementName"] == pattern["arrangement"]
    {
        pattern["saved_arrangement"].clone()
    } else {
        arrangement_details(state, pattern["arrangement"].as_str().unwrap_or(""))
    };
    Ok(json!({
        "index": i, "name": pattern["name"], "bbox": bbox(limits), "pos_2d": centre(limits),
        "line_info": {"lines": lines, "lineCount": lines.len(), "patternName": pattern["name"],
            "patternIndex": i, "status": "success", "boundingBox": {"minX": left, "minY": bottom,
                "maxX": right, "maxY": top, "width": right - left, "height": top - bottom}},
        "input_info": {"Pattern InputInformation": [{"Pattern index": i.to_string(), "Pattern name": pattern["name"],
            "PointList": records, "LineList": line_list, "Children Information": input_children(pattern)?}]},
        "fabric_index": pattern["fabric_index"].as_u64().unwrap_or(0),
        "arrangement": arrangement,
        "seam_groups": group_indices(state, pattern), "layer": pattern["layer"].as_i64().unwrap_or(0),
        "particle_distance": pattern["particle_distance"].as_f64().unwrap_or(20.0)
    }))
}

pub(super) fn outline_query(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let pattern = &state["patterns"][i];
    let [dx, dy] = offset(pattern);
    let points: Vec<_> = outline(pattern)?
        .into_iter()
        .map(|p| [p.x + dx, p.y + dy])
        .collect();
    Ok(json!({"points": points}))
}

pub(super) fn create(args: &Value, state: &mut Value) -> Reply {
    let i = array(state, "patterns").len();
    let name = args["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Pattern_{}", i + 1));
    let point = args["arrangement"].as_str().unwrap_or("");
    if !point.is_empty() {
        arrangement(state, point)?;
    }
    let pts = points(args)?;
    let pattern = json!({
        "id": next_id(state)?, "name": name, "points": pts.into_iter().map(Point::record).collect::<Vec<_>>(),
        "arrangement": point, "fabric_index": 0
    });
    vec_mut(state, "patterns").push(pattern);
    use_partial_pose(state);
    Ok(json!({"index": i, "name": name}))
}

fn discard_panel_pose(state: &mut Value, id: &Value) {
    let Some(meshes) = state["simulation"].get_mut("meshes").and_then(Value::as_array_mut) else {
        return;
    };
    meshes.retain(|mesh| mesh["id"] != *id);
    if meshes.is_empty() {
        invalidate_geometry(state);
    } else {
        use_partial_pose(state);
    }
}

pub(super) fn delete(args: &Value, state: &mut Value) -> Reply {
    let refs = args["patterns"]
        .as_array()
        .ok_or("patterns must be an array")?;
    let indices: BTreeSet<_> = refs
        .iter()
        .map(|r| pattern_index(state, r))
        .collect::<Result<_, _>>()?;
    let removed: Vec<_> = indices
        .iter()
        .map(|i| state["patterns"][i]["id"].clone())
        .collect();
    if !removed.is_empty() {
        super::annotations::remove_metadata_for(state, &removed);
        vec_mut(state, "seams")
            .retain(|s| !removed.contains(&s["pattern_a"]) && !removed.contains(&s["pattern_b"]));
        for i in indices.iter().rev() {
            vec_mut(state, "patterns").remove(*i);
        }
        crate::compat_runtime::remove_deleted_pattern_selection(state, &removed);
        if let Some(meshes) = state["simulation"].get_mut("meshes").and_then(Value::as_array_mut) {
            meshes.retain(|mesh| !removed.contains(&mesh["id"]));
            if meshes.is_empty() {
                invalidate_geometry(state);
            }
        }
    }
    Ok(json!({"deleted": indices.len(), "pattern_count": array(state, "patterns").len()}))
}

pub(super) fn arrange(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let name = string(args, "point")?;
    arrangement(state, name)?;
    let details = arrangement_details(state, name);
    state["patterns"][i]["arrangement"] = json!(name);
    state["patterns"][i]
        .as_object_mut()
        .expect("pattern record")
        .remove("saved_arrangement");
    let id = state["patterns"][i]["id"].clone();
    discard_panel_pose(state, &id);
    Ok(json!({"index": i, "arrangement": details}))
}

fn nested_integer(value: &Value) -> Option<i64> {
    value.as_i64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .or_else(|| {
            let number = value.as_f64()?.trunc();
            (i64::MIN as f64..-(i64::MIN as f64))
                .contains(&number)
                .then_some(number as i64)
        })
}

fn nested_number(value: &Value, key: &str) -> Result<f64, String> {
    value.as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("{key} must be a finite number"))
}

pub(super) fn edit(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let mut points = points(&state["patterns"][i])?;
    let original = points.clone();
    let corners = corner_offsets(&points);
    let [dx, dy] = offset(&state["patterns"][i]);
    let mut moved = Vec::new();
    for change in args["moves"].as_array().ok_or("moves must be an array")? {
        let cursor = nested_integer(&change["point"])
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("point must be a nonnegative integer")?;
        let position = *corners.get(cursor).ok_or("point index is out of range")?;
        let p = original[position];
        let read_coordinate = |key, default| match change.get(key) {
            Some(value) => nested_number(value, key),
            None => Ok(default),
        };
        let (x, y) = if change.get("x").is_some() && change.get("y").is_some() {
            (read_coordinate("x", 0.0)?, read_coordinate("y", 0.0)?)
        } else {
            (
                p.x + dx + read_coordinate("dx", 0.0)?,
                p.y + dy + read_coordinate("dy", 0.0)?,
            )
        };
        let local_x = if x == p.x + dx { p.x } else { x - dx };
        let local_y = if y == p.y + dy { p.y } else { y - dy };
        if ![x, y, local_x, local_y].into_iter().all(f64::is_finite) {
            return Err("point coordinates overflow".into());
        }
        points[position] = Point {
            x: local_x,
            y: local_y,
            ..p
        };
        moved.push(json!({"point": cursor, "x": x, "y": y}));
    }
    if points != original {
        set_points(&mut state["patterns"][i], points);
        let id = state["patterns"][i]["id"].clone();
        discard_panel_pose(state, &id);
    }
    Ok(json!({"index": i, "moved": moved}))
}

fn set_points(pattern: &mut Value, points: Vec<Point>) {
    pattern["points"] = Value::Array(points.into_iter().map(Point::record).collect());
    pattern
        .as_object_mut()
        .expect("pattern object")
        .remove("native_outline_metrics");
}

fn reflect(p: Point, a: Point, b: Point) -> Point {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / (dx * dx + dy * dy);
    Point {
        x: 2. * (a.x + t * dx) - p.x,
        y: 2. * (a.y + t * dy) - p.y,
        ..p
    }
}

fn flip(pattern: &mut Value, horizontal: bool) -> Result<(), String> {
    let mut pts = points(pattern)?;
    let midpoint = |x: fn(&Point) -> f64| {
        let min = pts.iter().map(x).fold(f64::INFINITY, f64::min);
        let max = pts.iter().map(x).fold(f64::NEG_INFINITY, f64::max);
        (min + max) / 2.
    };
    let centre = if horizontal {
        midpoint(|p| p.x)
    } else {
        midpoint(|p| p.y)
    };
    let mirror = |p: &mut Point| {
        if horizontal {
            p.x = 2. * centre - p.x;
        } else {
            p.y = 2. * centre - p.y;
        }
    };
    for p in &mut pts {
        mirror(p);
    }
    if let Some(children) = pattern["native_children"].as_array_mut() {
        let mirror_points = |points: &mut Value| -> Result<(), String> {
            if let Some(points) = points.as_array_mut() {
                for point in points {
                    let mut p = Point::from_json(point)?;
                    mirror(&mut p);
                    *point = p.record();
                }
            }
            Ok(())
        };
        for child in children {
            mirror_points(&mut child["points"])?;
            if let Some(lines) = child["lines"].as_array_mut() {
                for line in lines {
                    if let Some(points) = line.get_mut("points") {
                        mirror_points(points)?;
                    }
                }
            }
        }
    }
    if let Some(rest) = pattern.get_mut("inner_seam_rest") {
        let mut paths = super::InnerSeamRest::deserialize(&*rest)
            .map_err(|error| format!("invalid inner seam rest geometry: {error}"))?;
        let axis = usize::from(!horizontal);
        for point in paths.iter_mut().flatten().flatten() {
            point[axis] = 2.0 * centre - point[axis];
        }
        *rest = json!(paths);
    }
    set_points(pattern, pts);
    Ok(())
}

fn move_2d(pattern: &mut Value, args: &Value, default_dx: f64) -> Result<(), String> {
    let [dx, dy] = offset(pattern);
    let offset = |key, default| {
        if args.get(key).is_some() {
            number(args, key)
        } else {
            Ok(default)
        }
    };
    let (x, y) = if args.get("x").is_some() || args.get("y").is_some() {
        let [cx, cy] = centre(bounds(pattern)?);
        (dx + number(args, "x")? - cx, dy + number(args, "y")? - cy)
    } else {
        (dx + offset("dx", default_dx)?, dy + offset("dy", 0.)?)
    };
    if !x.is_finite() || !y.is_finite() {
        return Err("2D position overflow".into());
    }
    pattern["pos_2d"] = json!([x, y]);
    Ok(())
}

fn expand(pattern: &mut Value, line: usize, half_symmetry: bool) -> Result<(), String> {
    let pts = points(pattern)?;
    let corners = corner_offsets(&pts);
    let start = *corners.get(line).ok_or("unfold line is out of range")?;
    let end = corners[(line + 1) % corners.len()];
    let edges = split_edges(&pts);
    if edges[line].len() != 2 {
        return Err("unfold requires a straight line".into());
    }
    let a = pts[start];
    let b = pts[end];
    if a.distance(b) < 1e-9 {
        return Err("cannot unfold along a zero-length line".into());
    }
    let count = (start + pts.len() - end) % pts.len() + 1;
    let full: Vec<_> = pts.iter().cycle().skip(end).take(count).copied().collect();
    let mirrored = full[1..full.len() - 1]
        .iter()
        .rev()
        .map(|p| reflect(*p, a, b));
    let outline = full.iter().copied().chain(mirrored).collect();
    set_points(pattern, outline);
    pattern["half_symmetry"] = json!(half_symmetry);
    Ok(())
}

fn line_properties(pattern: &mut Value, op: &str, args: &Value, on: Option<bool>) -> Result<(), String> {
    let line = args
        .get("line")
        .map_or(Some(-1), nested_integer)
        .ok_or("line must be an integer")?;
    let count = corner_offsets(&points(pattern)?).len();
    if line < -1 || line >= count as i64 {
        return Err("line index is out of range".into());
    }
    let mut update = json!({});
    if let Some(on) = on {
        update["on"] = json!(on);
    }
    if op == "elastic" {
        for key in ["total_length", "strength"] {
            if let Some(value) = args.get(key) {
                let value = nested_number(value, key)?;
                if value < 0.0 {
                    return Err(format!("{key} must be nonnegative"));
                }
                update[key] = json!(value);
            }
        }
    }
    let fields = update.as_object().expect("line property update object");
    let properties = &mut pattern["line_properties"];
    if line == -1 {
        if let Some(entries) = properties.as_object_mut() {
            for (index, entry) in entries {
                if index == "-1" {
                    continue;
                }
                if let Some(setting) = entry.get_mut(op).and_then(Value::as_object_mut) {
                    setting.retain(|key, _| !fields.contains_key(key));
                }
            }
        }
    }
    let setting = &mut properties[line.to_string()][op];
    for (key, value) in fields {
        setting[key] = value.clone();
    }
    Ok(())
}

pub(super) fn set_elastic_values(args: &Value, state: &mut Value) -> Reply {
    let index = pattern_index(state, &args["pattern"])?;
    line_properties(&mut state["patterns"][index], "elastic", args, None)?;
    Ok(Value::Null)
}

fn remap_outline(state: &mut Value, pattern: &mut Value, lines: &[Option<usize>]) {
    let id = &pattern["id"];
    let remap = |line: usize| lines.get(line).copied().flatten();
    super::annotations::remap_outline(state, id, lines);
    vec_mut(state, "seams").retain_mut(|seam| {
        for (piece, line) in [("pattern_a", "line_a"), ("pattern_b", "line_b")] {
            if &seam[piece] != id {
                continue;
            }
            let Some(index) = seam[line]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .and_then(&remap)
            else {
                return false;
            };
            seam[line] = json!(index);
        }
        true
    });
    if let Some(properties) = pattern["line_properties"].as_object() {
        let migrated = properties.iter().filter_map(|(line, settings)| {
            let key = if line == "-1" {
                line.clone()
            } else {
                remap(line.parse().ok()?)?.to_string()
            };
            Some((key, settings.clone()))
        }).collect::<serde_json::Map<_, _>>();
        pattern["line_properties"] = json!(migrated);
    }
}

fn remove_point(pattern: &mut Value, corner: usize) -> Result<(), String> {
    let pts = points(pattern)?;
    let corners = corner_offsets(&pts);
    if corners.len() == 3 {
        return Err("a piece must retain at least three corner points".into());
    }
    let offset = *corners.get(corner).ok_or("point index is out of range")?;
    // Control points belonging to either adjoining edge are dropped when the edges merge.
    let mut remove = BTreeSet::from([offset]);
    for direction in [pts.len() - 1, 1] {
        let mut cursor = (offset + direction) % pts.len();
        while pts[cursor].kind != 0 {
            remove.insert(cursor);
            cursor = (cursor + direction) % pts.len();
        }
    }
    set_points(
        pattern,
        pts.into_iter()
            .enumerate()
            .filter_map(|(i, p)| (!remove.contains(&i)).then_some(p))
            .collect(),
    );
    Ok(())
}

pub(super) fn operate(request: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &request["pattern"])?;
    let op = string(request, "op")?;
    let args = &request["args"];
    let mut pattern = state["patterns"][i].clone();
    let mut result = json!({"index": i, "op": op, "ok": true});
    match op {
        "rename" => pattern["name"] = json!(string(args, "name")?),
        "move_2d" => move_2d(&mut pattern, args, 0.)?,
        "flip" => flip(&mut pattern, flag(args, "horizontal", true))?,
        "copy" | "symmetry" => {
            let old_id = pattern["id"].clone();
            let id = json!(next_id(state)?);
            pattern["id"] = id.clone();
            if op == "symmetry" {
                flip(&mut pattern, true)?;
            }
            if op == "copy" {
                move_2d(&mut pattern, args, 300.)?;
            }
            let new_index = array(state, "patterns").len();
            if op == "symmetry" && flag(args, "with_sewing", true) {
                super::annotations::duplicate_sewing(state, &old_id, &id)?;
            }
            vec_mut(state, "patterns").push(pattern);
            use_partial_pose(state);
            return Ok(if op == "copy" {
                json!({"new_index": new_index})
            } else {
                result
            });
        }
        "unfold" => {
            let fold = index(args, "line")?;
            let count = corner_offsets(&points(&pattern)?).len();
            expand(&mut pattern, fold, flag(args, "half_symmetry", true))?;
            let first = (fold + 1) % count;
            let mapping: Vec<_> = (0..count).map(|line| {
                (line != fold).then(|| (line + count - first) % count)
            }).collect();
            remap_outline(state, &mut pattern, &mapping);
            result = json!({"ok": true});
        }
        "layer" => {
            pattern["layer"] = json!(args["layer"].as_i64().ok_or("layer must be an integer")?)
        }
        "particle_distance" => {
            let mm = number(args, "mm")?;
            if mm <= 0. {
                return Err("particle distance must be positive".into());
            }
            pattern["particle_distance"] = json!(mm);
        }
        "freeze" | "hide" | "strengthen" => pattern[op] = json!(flag(args, "on", true)),
        "elastic" | "shirring" => {
            line_properties(&mut pattern, op, args, Some(flag(args, "on", true)))?;
        }
        "delete_line" | "delete_point" => {
            let corner = index(args, if op == "delete_line" { "line" } else { "point" })?;
            let count = corner_offsets(&points(&pattern)?).len();
            remove_point(&mut pattern, corner)?;
            let before = (corner + count - 1) % count;
            let mapping: Vec<_> = (0..count).map(|line| {
                (line != corner && line != before).then(|| line - usize::from(line > corner))
            }).collect();
            remap_outline(state, &mut pattern, &mapping);
        }
        _ => return Err(format!("unknown op {op:?}")),
    }
    if op == "rename" {
        if let Some(meshes) = state
            .get_mut("simulation")
            .and_then(|simulation| simulation.get_mut("meshes"))
            .and_then(Value::as_array_mut)
        {
            for mesh in meshes.iter_mut().filter(|mesh| mesh["id"] == pattern["id"]) {
                mesh["name"] = pattern["name"].clone();
            }
        }
    } else if !matches!(
        op,
        "elastic" | "shirring" | "freeze" | "layer" | "move_2d" | "hide" | "strengthen"
    ) {
        discard_panel_pose(state, &pattern["id"]);
    }
    state["patterns"][i] = pattern;
    Ok(result)
}

pub(super) fn sew(args: &Value, state: &mut Value) -> Reply {
    let ia = pattern_index(state, &args["pattern_a"])?;
    let ib = pattern_index(state, &args["pattern_b"])?;
    let la = index(args, "line_a")?;
    let lb = index(args, "line_b")?;
    let count = super::annotations::count(state);
    if la >= lines(&state["patterns"][ia])?.len()
        || lb >= lines(&state["patterns"][ib])?.len()
        || (ia == ib && la == lb)
    {
        return Ok(json!({"ok": false, "seam_groups": count}));
    }
    let a = state["patterns"][ia]["id"].clone();
    let b = state["patterns"][ib]["id"].clone();
    let duplicate = array(state, "seams").iter().any(|s| {
        (s["pattern_a"] == a && s["line_a"] == la && s["pattern_b"] == b && s["line_b"] == lb)
            || (s["pattern_a"] == b
                && s["line_a"] == lb
                && s["pattern_b"] == a
                && s["line_b"] == la)
    });
    if duplicate {
        return Ok(json!({"ok": false, "seam_groups": count}));
    }
    let seam = json!({
        "pattern_a": a, "line_a": la, "pattern_b": b, "line_b": lb,
        "dir_a": flag(args, "dir_a", true), "dir_b": flag(args, "dir_b", true),
        "name": format!("Seamline Pair Group {}", count + 1)
    });
    vec_mut(state, "seams").push(seam);
    // Sewing adds constraints without changing the fabric mesh. The next
    // simulation assembles the new stitches against the current drape.
    Ok(json!({"ok": true, "seam_groups": count + 1}))
}

pub(super) fn seams(_: &Value, state: &mut Value) -> Reply {
    Ok(super::annotations::list(state))
}
