use super::{array, flag, invalidate_geometry, next_id, string, use_partial_pose, vec_mut, Reply};
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
    fn components(point: &Value) -> Result<(f64, f64, f64), String> {
        let x = point.get(0).and_then(Value::as_f64).filter(|x| x.is_finite());
        let y = point.get(1).and_then(Value::as_f64).filter(|y| y.is_finite());
        let kind = point.get(2).map_or(Some(0.0), |value| value.as_f64().filter(|kind| kind.is_finite()));
        match (x, y, kind) {
            (Some(x), Some(y), Some(kind)) => Ok((x, y, kind)),
            _ => Err("point must have finite x, y and optional type".into()),
        }
    }

    pub fn from_json(point: &Value) -> Result<Self, String> {
        let (x, y, kind) = Self::components(point)?;
        if ![0.0, 2.0, 3.0].contains(&kind) {
            return Err("point type must be 0, 2 or 3".into());
        }
        Ok(Self { x, y, kind: kind as u64 })
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
    validate_outline(&result)?;
    Ok(result)
}

fn validate_outline(points: &[Point]) -> Result<(), String> {
    // A closed contour can have two corners when one of the edges is curved.
    if points.len() < 3 || points.iter().filter(|point| point.kind == 0).count() < 2 {
        return Err("need at least 3 outline points and 2 corners".into());
    }
    Ok(())
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

const CURVE_STEPS: u32 = 29;

pub(super) fn curve(edge: &[Point], sampled: bool) -> Vec<Point> {
    curve_with_steps(edge, sampled, CURVE_STEPS)
}

fn curve_with_steps(edge: &[Point], sampled: bool, steps: u32) -> Vec<Point> {
    if sampled || edge.len() < 3 {
        return edge.to_vec();
    }
    if edge[1..edge.len() - 1].iter().all(|p| p.kind == 3) {
        return (0..=steps)
            .map(|i| bezier(edge, f64::from(i) / f64::from(steps)))
            .collect();
    }
    natural_spline(edge, steps)
}

fn natural_spline(edge: &[Point], steps: u32) -> Vec<Point> {
    let mut knots = edge.to_vec();
    knots.dedup_by(|a, b| a.x == b.x && a.y == b.y);
    if knots.len() < 3 {
        return knots;
    }
    let spans: Vec<_> = knots.windows(2).map(|p| p[0].distance(p[1])).collect();
    if spans.iter().any(|&h| h == 0.0 || !h.is_finite()) {
        return knots;
    }
    // Solve the tridiagonal system for second derivatives, with zero
    // curvature at the endpoints and chord-length knot parameters.
    let mut upper = vec![0.0; knots.len()];
    let mut curvature = vec![[0.0; 2]; knots.len()];
    for i in 1..knots.len() - 1 {
        let (left, right) = (spans[i - 1], spans[i]);
        let divisor = 2.0 * (left + right) - left * upper[i - 1];
        upper[i] = right / divisor;
        let derivative = |a, b, c, previous| {
            (6.0 * ((c - b) / right - (b - a) / left) - left * previous) / divisor
        };
        let (a, b, c) = (knots[i - 1], knots[i], knots[i + 1]);
        let [x, y] = curvature[i - 1];
        curvature[i] = [derivative(a.x, b.x, c.x, x), derivative(a.y, b.y, c.y, y)];
    }
    for i in (1..knots.len() - 1).rev() {
        let next = curvature[i + 1];
        for (value, following) in curvature[i].iter_mut().zip(next) {
            *value -= upper[i] * following;
        }
    }
    let mut result = Vec::new();
    for (i, h) in spans.into_iter().enumerate() {
        let (a, b) = (knots[i], knots[i + 1]);
        for step in 0..steps {
            let t = f64::from(step) / f64::from(steps);
            let s = 1.0 - t;
            let evaluate = |a, b, left, right| {
                s * a + t * b + h * h / 6.0
                    * ((s * s * s - s) * left + (t * t * t - t) * right)
            };
            result.push(Point {
                x: evaluate(a.x, b.x, curvature[i][0], curvature[i + 1][0]),
                y: evaluate(a.y, b.y, curvature[i][1], curvature[i + 1][1]),
                kind: 0,
            });
        }
    }
    result.push(knots[knots.len() - 1]);
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

pub(crate) fn layout_outline(pattern: &Value) -> Result<Vec<[f64; 3]>, String> {
    let [dx, dy] = offset(pattern);
    Ok(outline(pattern)?.into_iter().map(|point| [point.x + dx, point.y + dy, 0.0]).collect())
}

pub(crate) fn sampled_perimeter(pattern: &Value, max_samples: usize) -> Result<(Vec<[f64; 2]>, Vec<usize>), String> {
    let mut vertices = Vec::new();
    let mut corners = Vec::new();
    let outline = points(pattern)?;
    let steps = (max_samples / outline.len()).clamp(1, CURVE_STEPS as usize) as u32;
    for edge in split_edges(&outline) {
        corners.push(vertices.len());
        let sampled = curve_with_steps(&edge, flag(pattern, "sampled_polyline", false), steps);
        vertices.extend(sampled[..sampled.len() - 1].iter().map(|point| [point.x, point.y]));
    }
    Ok((vertices, corners))
}

fn bounds(pattern: &Value) -> Result<[f64; 4], String> {
    if let Ok([left, bottom, right, top]) = serde_json::from_value::<[f64; 4]>(pattern["native_outline_bounds"].clone()) {
        let [dx, dy] = offset(pattern);
        return Ok([left + dx, bottom + dy, right + dx, top + dy]);
    }
    Ok(point_bounds(outline(pattern)?, offset(pattern)))
}

pub(super) fn point_bounds(points: impl IntoIterator<Item = Point>, [dx, dy]: [f64; 2]) -> [f64; 4] {
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
        let angular_limit = 20_f64.to_radians().tan();
        let orientation = if vy < vx * angular_limit {
            "horizontal"
        } else if vx < vy * angular_limit {
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
    named_pattern_index(patterns, name)
}

fn named_pattern_index(patterns: &[Value], name: &str) -> Result<usize, String> {
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
    if name.is_empty() || name == "Arrangement Point" {
        let mut settings = json!({"ArrangementName": "Arrangement Point"});
        super::append_curved_arrangement_fields(&mut settings, [Some(0.0); 4]);
        return settings;
    }
    if let Some(record) = array(&state["avatar"], "arrangement_records").iter()
        .find(|record| record["ArrangementName"] == name)
    {
        let mut details = record.clone();
        if let Some(fields) = details.as_object_mut() {
            fields.remove("ArrangementIndex");
        }
        return details;
    }
    match arrangement(state, name) {
        Ok((_, choice)) if choice.is_object() => choice.clone(),
        Ok((_, _)) => json!({"ArrangementName": name}),
        Err(_) if !name.is_empty() => json!({"ArrangementName":name}),
        Err(_) => Value::Null,
    }
}

pub(super) fn summary(pattern: &Value, i: usize) -> Value {
    let arrangement = pattern["arrangement"].as_str().filter(|name| !name.is_empty())
        .unwrap_or("Arrangement Point");
    json!({
        "index": i, "name": pattern["name"], "arrangement": arrangement,
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

fn input_children(pattern: &Value, outline_points: &[Value], outline_lines: &[Value]) -> Result<Vec<Value>, String> {
    let outline_copy = json!({"type": "Base Line", "outline_copy": true});
    let children = pattern["native_children"].as_array().map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(&outline_copy));
    // CLO includes the parent shape in this count.
    let mut records = vec![json!({"Children shape count": (children.len() + 1).to_string()})];
    for (index, child) in children.iter().enumerate() {
        if child["outline_copy"] == true && pattern["native_outline_metrics"].is_null() {
            records.push(json!({
                "Shape index": index.to_string(), "Shape name": pattern["name"],
                "Shape type": child["type"], "PointList": outline_points, "LineList": outline_lines,
            }));
            continue;
        }
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
        "input_info": {"Pattern InputInformation": [{"Pattern index": "0", "Pattern name": pattern["name"],
            "PointList": records, "LineList": line_list, "Children Information": input_children(pattern, &records, &line_list)?}]},
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
    let patterns = array(state, "patterns");
    let i = patterns.len();
    let name = args["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            (i + 1..).map(|suffix| format!("Pattern_{suffix}"))
                .find(|name| !patterns.iter().any(|pattern| pattern["name"] == name.as_str()))
                .expect("at most one generated name per pattern")
        });
    let point = args["arrangement"].as_str().unwrap_or("");
    let components = args["points"].as_array().ok_or("points must be an array")?
        .iter().map(Point::components).collect::<Result<Vec<_>, _>>()?;
    if components.len() < 3 {
        return Err("need at least 3 points".into());
    }
    let (first_x, first_y, _) = components[0];
    if components.iter().any(|(_, _, kind)| ![0.0, 2.0, 3.0].contains(&kind.trunc()))
        || components.iter().all(|&(x, y, _)| x == first_x && y == first_y)
    {
        if !point.is_empty() {
            arrangement(state, point)?;
        }
        return Ok(json!({"index": -1, "name": ""}));
    }
    let pts = components.into_iter().map(|(x, y, kind)| Point { x, y, kind: kind.trunc() as u64 })
        .collect::<Vec<_>>();
    validate_outline(&pts)?;
    let pattern = json!({
        "id": next_id(state)?, "name": name, "points": pts.into_iter().map(Point::record).collect::<Vec<_>>(),
        "arrangement": "", "fabric_index": state["current_fabric"].as_u64().unwrap_or(0)
    });
    vec_mut(state, "patterns").push(pattern);
    use_partial_pose(state);
    crate::compat_runtime::invalidate_scene_ui_snapshot(state);
    if !point.is_empty() {
        arrangement(state, point)?;
        state["patterns"][i]["arrangement"] = json!(point);
    }
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

pub(crate) fn bridge_pattern_index(patterns: &[Value], reference: &Value) -> Result<i64, String> {
    if let Some(index) = reference.as_i64() {
        return Ok(index);
    }
    let reference = reference.as_str().ok_or("pattern reference must be a name or index")?;
    let digits = reference.trim_start_matches('-');
    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        if reference.starts_with("--") {
            return Err(format!("invalid pattern index {reference:?}"));
        }
        return Ok(reference.parse().unwrap_or_else(|_| {
            if reference.starts_with('-') { i64::MIN } else { i64::MAX }
        }));
    }
    named_pattern_index(patterns, reference)?
        .try_into().map_err(|_| "pattern index is too large".into())
}

pub(super) fn delete(args: &Value, state: &mut Value) -> Reply {
    let refs = args["patterns"]
        .as_array()
        .ok_or("patterns must be an array")?;
    let patterns = array(state, "patterns");
    let requested: BTreeSet<_> = refs
        .iter()
        .map(|reference| bridge_pattern_index(patterns, reference))
        .collect::<Result<_, _>>()?;
    // CLO sorts descending. A positive overflow fails before any valid deletion,
    // and a negative overflow fails after valid indices have been handled.
    let overflow = "Pattern index must be a 32-bit integer";
    if requested.last().is_some_and(|index| *index > i64::from(i32::MAX)) {
        return Err(overflow.into());
    }
    let indices: Vec<_> = requested.iter().filter_map(|&index| usize::try_from(index).ok())
        .filter(|&index| index < patterns.len()).collect();
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
    if requested.first().is_some_and(|index| *index < i64::from(i32::MIN)) {
        return Err(overflow.into());
    }
    Ok(json!({"deleted": requested.len(), "pattern_count": array(state, "patterns").len()}))
}

pub(super) fn arrange(args: &Value, state: &mut Value) -> Reply {
    let patterns = array(state, "patterns");
    let raw_index = bridge_pattern_index(patterns, &args["pattern"])?;
    let name = string(args, "point")?;
    arrangement(state, name)?;
    let index = i32::try_from(raw_index).map_err(|_| "Pattern index must be a 32-bit integer")?;
    let Some(i) = usize::try_from(index).ok().filter(|&index| index < patterns.len()) else {
        return Ok(json!({"index": index, "arrangement": {}}));
    };
    let details = arrangement_details(state, name);
    state["patterns"][i]["arrangement"] = json!(name);
    state["patterns"][i]
        .as_object_mut()
        .expect("pattern record")
        .remove("saved_arrangement");
    let id = state["patterns"][i]["id"].clone();
    discard_panel_pose(state, &id);
    crate::compat_runtime::invalidate_scene_ui_snapshot(state);
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

fn move_edge_controls(
    points: &mut [Point], start: usize, end: usize, delta: [f64; 2], move_start: bool,
    sampled: bool,
) {
    let edge_indices: Vec<_> = (start..points.len())
        .chain(0..start)
        .take_while(|&index| index != end)
        .chain(std::iter::once(end))
        .collect();
    if edge_indices.len() <= 2 {
        return;
    }
    let controls = &edge_indices[1..edge_indices.len() - 1];
    let translate = |point: &mut Point, weight: f64| {
        point.x += weight * delta[0];
        point.y += weight * delta[1];
    };
    if !sampled && controls.iter().all(|&index| points[index].kind == 3) {
        let adjacent = if move_start { controls[0] } else { controls[controls.len() - 1] };
        translate(&mut points[adjacent], 1.0);
        return;
    }

    // The native spline distributes the endpoint displacement by sampled arc length.
    let edge: Vec<_> = edge_indices.iter().map(|&index| points[index]).collect();
    let samples = curve(&edge, sampled);
    let distances: Vec<_> = std::iter::once(0.0).chain(samples.windows(2).scan(0.0, |sum, pair| {
        *sum += pair[0].distance(pair[1]);
        Some(*sum)
    })).collect();
    let length = distances.last().copied().unwrap_or_default();
    if length <= 0.0 || !length.is_finite() {
        return;
    }
    let mut knot = 0;
    for &index in controls {
        let p = points[index];
        if let Some(step) = samples[knot..].iter().position(|s| s.x == p.x && s.y == p.y) {
            knot += step;
            let fraction = distances[knot] / length;
            let weight = if move_start { 1.0 - fraction } else { fraction };
            translate(&mut points[index], weight);
        }
    }
}

pub(super) fn edit(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let pattern = &state["patterns"][i];
    let native_controls = pattern["native_outline_controls"].as_array();
    let sampled = native_controls.is_none() && flag(pattern, "sampled_polyline", false);
    let mut points = match native_controls {
        Some(controls) => controls.iter().map(Point::from_json).collect::<Result<_, _>>()?,
        None => points(pattern)?,
    };
    let original = points.clone();
    let corners = corner_offsets(&points);
    let [dx, dy] = offset(&state["patterns"][i]);
    let mut moved = Vec::new();
    let changes = args["moves"].as_array().ok_or("moves must be an array")?;
    let outcome = changes.iter().try_for_each(|change| -> Result<(), String> {
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
        let delta = [local_x - points[position].x, local_y - points[position].y];
        if delta != [0.0; 2] {
            let previous = corners[(cursor + corners.len() - 1) % corners.len()];
            let next = corners[(cursor + 1) % corners.len()];
            move_edge_controls(&mut points, previous, position, delta, false, sampled);
            move_edge_controls(&mut points, position, next, delta, true, sampled);
        }
        points[position] = Point {
            x: local_x,
            y: local_y,
            ..p
        };
        moved.push(json!({"point": cursor, "x": x, "y": y}));
        Ok(())
    });
    if points != original {
        if native_controls.is_some() {
            state["patterns"][i]["sampled_polyline"] = json!(false);
        }
        set_points(&mut state["patterns"][i], points);
        let id = state["patterns"][i]["id"].clone();
        discard_panel_pose(state, &id);
        crate::compat_runtime::invalidate_scene_ui_snapshot(state);
    }
    outcome?;
    Ok(json!({"index": i, "moved": moved}))
}

fn set_points(pattern: &mut Value, points: Vec<Point>) {
    pattern["points"] = Value::Array(points.into_iter().map(Point::record).collect());
    let record = pattern.as_object_mut().expect("pattern object");
    record.remove("native_outline_metrics");
    record.remove("native_outline_bounds");
    record.remove("native_outline_controls");
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

fn move_2d(pattern: &mut Value, args: &Value, copy: bool) -> Result<(), String> {
    let [dx, dy] = offset(pattern);
    let offset = |key, default| {
        if let Some(value) = args.get(key) {
            nested_number(value, key)
        } else {
            Ok(default)
        }
    };
    let (x, y) = if !copy && args.get("x").is_some() {
        let [cx, cy] = centre(bounds(pattern)?);
        (dx + nested_number(&args["x"], "x")? - cx, dy + nested_number(&args["y"], "y")? - cy)
    } else {
        let default_dx = if copy { 300.0 } else { 0.0 };
        (dx + offset("dx", default_dx)?, dy + offset("dy", 0.)?)
    };
    if !x.is_finite() || !y.is_finite() {
        return Err("2D position overflow".into());
    }
    if x != dx || y != dy {
        pattern["pos_2d"] = json!([x, y]);
    }
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
    let outline = pts[..=start]
        .iter()
        .copied()
        .chain(mirrored)
        .chain(pts[start + 1..].iter().copied())
        .collect();
    set_points(pattern, outline);
    pattern["half_symmetry"] = json!(half_symmetry);
    Ok(())
}

fn option_truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
    }
}

fn line_properties(pattern: &mut Value, op: &str, args: &Value, on: Option<bool>) -> Result<(), String> {
    let line = args
        .get("line")
        .map_or(Some(-1), nested_integer)
        .ok_or("line must be an integer")?;
    let count = corner_offsets(&points(pattern)?).len();
    i32::try_from(line).map_err(|_| "line must be a 32-bit integer")?;
    let valid_line = (-1..count as i64).contains(&line);
    if !valid_line && on.is_none() {
        return Err("line index is out of range".into());
    }
    let properties = &mut pattern["line_properties"];
    let line_key = line.to_string();
    let mut set = |key: &str, value: Value| {
        if !valid_line {
            return;
        }
        if line == -1 {
            if let Some(entries) = properties.as_object_mut() {
                for (index, entry) in entries {
                    if index == "-1" {
                        continue;
                    }
                    if let Some(setting) = entry.get_mut(op).and_then(Value::as_object_mut) {
                        setting.remove(key);
                    }
                }
            }
        }
        properties[&line_key][op][key] = value;
    };
    if let Some(on) = on {
        set("on", json!(on));
    }
    if op == "elastic" {
        for key in ["total_length", "strength"] {
            if let Some(value) = args.get(key).filter(|value| on.is_none() || option_truth(value)) {
                let value = match value.as_bool() {
                    Some(value) => f64::from(u8::from(value)),
                    None => nested_number(value, key)?,
                };
                if value < 0.0 {
                    return Err(format!("{key} must be nonnegative"));
                }
                set(key, json!(value));
            }
        }
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

fn operation_integer(args: &Value, key: &str) -> Result<i32, String> {
    nested_integer(&args[key])
        .or_else(|| args[key].as_bool().map(i64::from))
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("{key} must be a 32-bit integer"))
}

fn copy_name(state: &Value, name: &str) -> String {
    let (base, next, width) = name
        .rsplit_once('_')
        .filter(|(_, suffix)| suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|(base, suffix)| {
            let next = suffix.parse::<u64>().ok()?.checked_add(1)?;
            Some((base, next, suffix.len()))
        })
        .unwrap_or((name, 1, 1));
    let occupied: BTreeSet<_> = array(state, "patterns")
        .iter()
        .filter_map(|pattern| pattern["name"].as_str())
        .collect();
    (next..)
        .map(|suffix| format!("{base}_{suffix:0width$}"))
        .find(|candidate| !occupied.contains(candidate.as_str()))
        .expect("the number of occupied names is finite")
}

pub(super) fn operate(request: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &request["pattern"])?;
    let op = string(request, "op")?;
    let args = &request["args"];
    let mut pattern = state["patterns"][i].clone();
    let mut result = json!({"index": i, "op": op, "ok": true});
    match op {
        "rename" => pattern["name"] = json!(string(args, "name")?),
        "move_2d" => move_2d(&mut pattern, args, false)?,
        "flip" => flip(&mut pattern, flag(args, "horizontal", true))?,
        "copy" | "symmetry" => {
            let old_id = pattern["id"].clone();
            let id = json!(next_id(state)?);
            pattern["id"] = id.clone();
            pattern["name"] = json!(copy_name(state, string(&pattern, "name")?));
            if op == "symmetry" {
                let [left, _, right, _] = bounds(&pattern)?;
                flip(&mut pattern, true)?;
                move_2d(&mut pattern, &json!({"dx": (right - left) / 2. + 100.}), true)?;
            } else {
                move_2d(&mut pattern, args, true)?;
            }
            let new_index = array(state, "patterns").len();
            if op == "symmetry" && flag(args, "with_sewing", true) {
                super::annotations::duplicate_sewing(state, &old_id, &id)?;
            }
            vec_mut(state, "patterns").push(pattern);
            use_partial_pose(state);
            crate::compat_runtime::invalidate_scene_ui_snapshot(state);
            return Ok(if op == "copy" {
                json!({"new_index": new_index})
            } else {
                result
            });
        }
        "unfold" => {
            let fold = operation_integer(args, "line")?;
            let count = corner_offsets(&points(&pattern)?).len();
            let Some(fold) = usize::try_from(fold).ok().filter(|&fold| fold < count) else {
                return Ok(json!({"ok": false}));
            };
            expand(&mut pattern, fold, flag(args, "half_symmetry", true))?;
            let mapping: Vec<_> = (0..count)
                .map(|line| (line != fold).then(|| line + usize::from(line > fold) * (count - 2)))
                .collect();
            remap_outline(state, &mut pattern, &mapping);
            result = json!({"ok": true});
        }
        "layer" => {
            pattern["layer"] = json!(operation_integer(args, "layer")?)
        }
        "particle_distance" => {
            let mm = nested_number(&args["mm"], "mm")?.max(0.8) as f32;
            if !mm.is_finite() {
                return Err("particle distance exceeds the supported float range".into());
            }
            pattern["particle_distance"] = json!(f64::from(mm));
        }
        "freeze" | "hide" | "strengthen" => pattern[op] = json!(flag(args, "on", true)),
        "elastic" => {
            let on = args.get("on").is_none_or(option_truth);
            line_properties(&mut state["patterns"][i], op, args, Some(on))?;
            return Ok(result);
        }
        "shirring" => {
            line_properties(&mut pattern, op, args, Some(args.get("on").is_none_or(option_truth)))?;
        }
        "delete_line" | "delete_point" => {
            let target = operation_integer(args, if op == "delete_line" { "line" } else { "point" })?;
            let count = corner_offsets(&points(&pattern)?).len();
            let Some(corner) = usize::try_from(target).ok().filter(|&target| target < count) else {
                return Ok(result);
            };
            // The native deletion operates on whole shapes for outline lines,
            // and deleting a triangle corner would leave no valid outline.
            if op == "delete_line" || count == 3 {
                delete(&json!({"patterns": [i]}), state)?;
                return Ok(result);
            }
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
    if op == "move_2d" && pattern["pos_2d"] != state["patterns"][i]["pos_2d"] {
        crate::compat_runtime::invalidate_scene_ui_snapshot(state);
    }
    state["patterns"][i] = pattern;
    Ok(result)
}

pub(super) fn sew(args: &Value, state: &mut Value) -> Reply {
    let panels = array(state, "patterns");
    let panel_index = |key| bridge_pattern_index(panels, &args[key]);
    let line_index = |key: &str| args[key].as_i64()
        .ok_or_else(|| format!("{key} must be a 32-bit integer"));
    let (ia, ib, la, lb) = (
        panel_index("pattern_a")?, panel_index("pattern_b")?,
        line_index("line_a")?, line_index("line_b")?,
    );
    let checked = [ia, ib, la, lb].map(i32::try_from);
    let [ia, ib, la, lb] = match checked {
        [Ok(ia), Ok(ib), Ok(la), Ok(lb)] => [ia, ib, la, lb],
        _ => return Err("sewing indices must fit in 32 bits".into()),
    };
    let count = super::annotations::count(state);
    let invalid = || Ok(json!({"ok": false, "seam_groups": count}));
    let [Some(ia), Some(ib), Some(la), Some(lb)] = [ia, ib, la, lb].map(|n| usize::try_from(n).ok()) else {
        return invalid();
    };
    let (Some(panel_a), Some(panel_b)) = (panels.get(ia), panels.get(ib)) else {
        return invalid();
    };
    if la >= corner_offsets(&points(panel_a)?).len()
        || lb >= corner_offsets(&points(panel_b)?).len()
    {
        return invalid();
    }
    let a = &panel_a["id"];
    let b = &panel_b["id"];
    let duplicate = array(state, "seams").iter().any(|s| {
        (&s["pattern_a"] == a && s["line_a"] == la && &s["pattern_b"] == b && s["line_b"] == lb)
            || (&s["pattern_a"] == b
                && s["line_a"] == lb
                && &s["pattern_b"] == a
                && s["line_b"] == la)
    });
    if duplicate {
        return Ok(json!({"ok": true, "seam_groups": count}));
    }
    let existing: BTreeSet<_> = super::annotations::seam_group_names(state)
        .filter_map(Value::as_str)
        .filter_map(|name| name.strip_prefix("Sewing_"))
        .filter_map(|suffix| suffix.parse::<usize>().ok())
        .collect();
    let suffix = (count + 1..).find(|suffix| !existing.contains(suffix))
        .expect("the number of occupied names is finite");
    let seam = json!({
        "pattern_a": a, "line_a": la, "pattern_b": b, "line_b": lb,
        "dir_a": flag(args, "dir_a", true), "dir_b": flag(args, "dir_b", true),
        "name": format!("Sewing_{suffix}")
    });
    vec_mut(state, "seams").push(seam);
    // Sewing adds constraints without changing the fabric mesh. The next
    // simulation assembles the new stitches against the current drape.
    Ok(json!({"ok": true, "seam_groups": count + 1}))
}

pub(super) fn seams(_: &Value, state: &mut Value) -> Reply {
    Ok(super::annotations::list(state))
}
