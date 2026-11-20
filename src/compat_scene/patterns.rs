use super::{array, flag, index, invalidate_geometry, next_id, number, string, vec_mut, Reply};
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

pub(super) fn outline(pattern: &Value) -> Result<Vec<Point>, String> {
    let mut result = Vec::new();
    for edge in split_edges(&points(pattern)?) {
        let sampled = curve(&edge, flag(pattern, "sampled_polyline", false));
        result.extend_from_slice(&sampled[..sampled.len() - 1]);
    }
    Ok(result)
}

fn bounds(pattern: &Value) -> Result<[f64; 4], String> {
    let [dx, dy] = offset(pattern);
    Ok(outline(pattern)?.into_iter().fold(
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
    ))
}

fn bbox(bounds: [f64; 4]) -> Value {
    let [left, bottom, right, top] = bounds;
    json!({"Width": format!("{:.6}", right - left), "Height": format!("{:.6}", top - bottom)})
}

fn slot(n: f64, before: &'static str, after: &'static str) -> &'static str {
    if n < 1.0 / 3.0 {
        before
    } else if n > 2.0 / 3.0 {
        after
    } else {
        ""
    }
}

fn lines(pattern: &Value) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    let [dx, dy] = offset(pattern);
    let [left, bottom, right, top] = bounds(pattern)?;
    for (line, edge) in split_edges(&points(pattern)?).iter().enumerate() {
        let sampled = curve(edge, flag(pattern, "sampled_polyline", false));
        let length: f64 = sampled.windows(2).map(|p| p[0].distance(p[1])).sum();
        let mut walked = 0.;
        let mut mid = sampled[0];
        for pair in sampled.windows(2) {
            let step = pair[0].distance(pair[1]);
            if walked + step >= length / 2. && step > 0. {
                let t = (length / 2. - walked) / step;
                mid.x = pair[0].x + t * (pair[1].x - pair[0].x);
                mid.y = pair[0].y + t * (pair[1].y - pair[0].y);
                break;
            }
            walked += step;
        }
        let start = edge[0];
        let end = edge[edge.len() - 1];
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

pub(super) fn info(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let pattern = &state["patterns"][i];
    let [dx, dy] = offset(pattern);
    let pts = points(pattern)?;
    let mut records = vec![json!({"Point count": corner_offsets(&pts).len().to_string()})];
    records.extend(pts.iter().filter(|p| p.kind == 0).enumerate().map(|(n, p)| {
        json!({"Point positionX": p.x + dx, "Point positionY": p.y + dy, "Point index": n.to_string()})
    }));
    let edges = split_edges(&pts);
    let lines = lines(pattern)?;
    let mut line_list = vec![json!({"Line count": lines.len().to_string()})];
    line_list.extend(lines.iter().zip(edges).enumerate().map(|(n, (line, edge))| {
        let kind = if edge.len() == 2 { "Straight type" } else { "" };
        json!({"Line index": n.to_string(), "Line length": line["length"], "Line type": kind})
    }));
    let limits = bounds(pattern)?;
    let [left, bottom, right, top] = limits;
    Ok(json!({
        "index": i, "name": pattern["name"], "bbox": bbox(limits), "pos_2d": centre(limits),
        "line_info": {"lines": lines, "lineCount": lines.len(), "patternName": pattern["name"],
            "patternIndex": i, "status": "success", "boundingBox": {"minX": left, "minY": bottom,
                "maxX": right, "maxY": top, "width": right - left, "height": top - bottom}},
        "input_info": {"Pattern InputInformation": [{"Pattern index": i.to_string(), "Pattern name": pattern["name"],
            "PointList": records, "LineList": line_list, "Children Information": [{"Children shape count": "0"}]}]},
        "fabric_index": pattern["fabric_index"].as_u64().unwrap_or(0),
        "arrangement": arrangement_details(state, pattern["arrangement"].as_str().unwrap_or("")),
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
    invalidate_geometry(state);
    Ok(json!({"index": i, "name": name}))
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
        invalidate_geometry(state);
    }
    Ok(json!({"deleted": indices.len(), "pattern_count": array(state, "patterns").len()}))
}

pub(super) fn arrange(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let name = string(args, "point")?;
    arrangement(state, name)?;
    let details = arrangement_details(state, name);
    state["patterns"][i]["arrangement"] = json!(name);
    invalidate_geometry(state);
    Ok(json!({"index": i, "arrangement": details}))
}

pub(super) fn edit(args: &Value, state: &mut Value) -> Reply {
    let i = pattern_index(state, &args["pattern"])?;
    let mut points = points(&state["patterns"][i])?;
    let corners = corner_offsets(&points);
    let [dx, dy] = offset(&state["patterns"][i]);
    let mut moved = Vec::new();
    for change in args["moves"].as_array().ok_or("moves must be an array")? {
        let cursor = index(change, "point")?;
        let position = *corners.get(cursor).ok_or("point index is out of range")?;
        let p = points[position];
        let (x, y) = if change.get("x").is_some() || change.get("y").is_some() {
            (number(change, "x")?, number(change, "y")?)
        } else {
            let offset = |key| {
                if change.get(key).is_some() {
                    number(change, key)
                } else {
                    Ok(0.)
                }
            };
            (p.x + dx + offset("dx")?, p.y + dy + offset("dy")?)
        };
        if !x.is_finite() || !y.is_finite() {
            return Err("point coordinates overflow".into());
        }
        points[position] = Point {
            x: x - dx,
            y: y - dy,
            ..p
        };
        moved.push(json!({"point": cursor, "x": x, "y": y}));
    }
    set_points(&mut state["patterns"][i], points);
    if !moved.is_empty() {
        invalidate_geometry(state);
    }
    Ok(json!({"index": i, "moved": moved}))
}

fn set_points(pattern: &mut Value, points: Vec<Point>) {
    pattern["points"] = Value::Array(points.into_iter().map(Point::record).collect());
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
    for p in &mut pts {
        if horizontal {
            p.x = 2. * centre - p.x;
        } else {
            p.y = 2. * centre - p.y;
        }
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

fn line_properties(pattern: &mut Value, op: &str, args: &Value) -> Result<(), String> {
    let line = args
        .get("line")
        .map_or(Some(-1), Value::as_i64)
        .ok_or("line must be an integer")?;
    let count = corner_offsets(&points(pattern)?).len();
    if line < -1 || line >= count as i64 {
        return Err("line index is out of range".into());
    }
    let mut update = json!({"on": flag(args, "on", true)});
    if op == "elastic" {
        for key in ["total_length", "strength"] {
            if args.get(key).is_some() {
                let value = number(args, key)?;
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

fn remap_seams(state: &mut Value, id: &Value, remap: impl Fn(usize) -> Option<usize>) {
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
    pattern["line_properties"] = Value::Null;
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
            vec_mut(state, "patterns").push(pattern);
            if op == "symmetry" && flag(args, "with_sewing", true) {
                let mirrored: Vec<_> = array(state, "seams")
                    .iter()
                    .filter(|s| s["pattern_a"] == old_id || s["pattern_b"] == old_id)
                    .cloned()
                    .map(|mut seam| {
                        if let Some(record) = seam.as_object_mut() {
                            record.remove("native_group");
                        }
                        for key in ["pattern_a", "pattern_b"] {
                            if seam[key] == old_id {
                                seam[key] = id.clone();
                            }
                        }
                        seam
                    })
                    .collect();
                vec_mut(state, "seams").extend(mirrored);
            }
            invalidate_geometry(state);
            return Ok(if op == "copy" {
                json!({"new_index": new_index})
            } else {
                result
            });
        }
        "unfold" => {
            let id = pattern["id"].clone();
            let fold = index(args, "line")?;
            let count = corner_offsets(&points(&pattern)?).len();
            expand(&mut pattern, fold, flag(args, "half_symmetry", true))?;
            pattern["line_properties"] = Value::Null;
            super::annotations::remove_metadata_for(state, &[id.clone()]);
            let first = (fold + 1) % count;
            remap_seams(state, &id, |line| {
                (line != fold).then(|| (line + count - first) % count)
            });
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
        "elastic" | "shirring" => line_properties(&mut pattern, op, args)?,
        "delete_line" | "delete_point" => {
            let corner = index(args, if op == "delete_line" { "line" } else { "point" })?;
            let count = corner_offsets(&points(&pattern)?).len();
            remove_point(&mut pattern, corner)?;
            super::annotations::remove_metadata_for(state, &[pattern["id"].clone()]);
            let before = (corner + count - 1) % count;
            remap_seams(state, &pattern["id"], |line| {
                (line != corner && line != before).then(|| line - usize::from(line > corner))
            });
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
    } else if !matches!(op, "elastic" | "shirring") {
        invalidate_geometry(state);
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
    invalidate_geometry(state);
    Ok(json!({"ok": true, "seam_groups": count + 1}))
}

pub(super) fn seams(_: &Value, state: &mut Value) -> Reply {
    Ok(super::annotations::list(state))
}
