use super::{array, array_mut, assets, boolean, number, render, solver, string};
use serde_json::{json, Map, Value};

struct Edge {
    name: &'static str,
    start: [f64; 2],
    intermediate: Vec<[f64; 3]>,
}

fn edge(name: &'static str, x: f64, y: f64) -> Edge {
    Edge {
        name,
        start: [x, y],
        intermediate: Vec::new(),
    }
}

struct Panel {
    name: &'static str,
    role: &'static str,
    arrange: &'static str,
    edges: Vec<Edge>,
    elastic: Vec<(usize, f64)>,
}

impl Panel {
    fn line_length(&self, line: usize) -> f64 {
        let edge = &self.edges[line];
        let next = self.edges[(line + 1) % self.edges.len()].start;
        edge_length(edge, next)
    }

    fn json(&self, offset: f64) -> Value {
        let mut points = Vec::new();
        let mut edges = Vec::new();
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for (line, edge) in self.edges.iter().enumerate() {
            let [x, y] = edge.start;
            points.push(json!([rounded(x + offset), rounded(y), 0]));
            let next = self.edges[(line + 1) % self.edges.len()].start;
            for &[cx, cy, kind] in &edge.intermediate {
                points.push(json!([rounded(cx + offset), rounded(cy), kind as u64]));
                low = low.min(cy);
                high = high.max(cy);
            }
            let length = edge_length(edge, next);
            low = low.min(y);
            high = high.max(y);
            edges.push(json!({"name":edge.name,"line":line,"length":rounded_mm(length)}));
        }
        json!({"name":self.name,"role":self.role,"points":points,"edges":edges,
            "arrangement":self.arrange,"x_percent":50,"offset_mm":30,"centre_height":rounded_mm((low+high)/2.0),
            "height":rounded_mm(high-low),"fabric":"main","layer":0,"elastic":self.elastic.iter().map(|(line,total)|json!({"line":line,"total_length":total})).collect::<Vec<_>>(),"draft_offset_x":offset})
    }
}

fn edge_length(edge: &Edge, end: [f64; 2]) -> f64 {
    let mut previous = edge.start;
    let mut total = 0.0;
    for &[x, y, _] in &edge.intermediate {
        total += (x - previous[0]).hypot(y - previous[1]);
        previous = [x, y];
    }
    total + (end[0] - previous[0]).hypot(end[1] - previous[1])
}

fn rounded_mm(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn rounded(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn normalize(spec: &Value) -> Result<Value, String> {
    let mut normalized: Map<String, Value> = spec
        .as_object()
        .ok_or("garment spec must be an object")?
        .iter()
        .map(|(key, val)| (key.to_lowercase().replace(' ', "_"), val.clone()))
        .collect();
    let garment = normalized
        .get("garment")
        .and_then(Value::as_str)
        .unwrap_or("dress")
        .to_lowercase();
    if !["dress", "top", "skirt"].contains(&garment.as_str()) {
        return Err(format!("unsupported Rust garment type {garment:?}"));
    }
    let silhouette = normalized
        .get("silhouette")
        .and_then(Value::as_str)
        .unwrap_or(if garment == "dress" {
            "a-line"
        } else {
            "regular"
        })
        .replace('_', "-")
        .to_lowercase();
    let (one_piece, fit, flare, skirt, waist) = match (garment.as_str(), silhouette.as_str()) {
        ("dress", "sheath") => (true, "fitted", -0.03, "", "natural"),
        ("dress", "pencil") => (true, "fitted", -0.05, "", "natural"),
        ("dress", "bodycon") => (true, "bodycon", -0.04, "", "natural"),
        ("dress", "shift") => (true, "regular", 0.05, "", "natural"),
        ("dress", "a-line") => (true, "regular", 0.30, "", "natural"),
        ("dress", "tent" | "trapeze") => (true, "loose", 0.55, "", "natural"),
        ("dress", "shirt" | "shirt-dress") => (true, "loose", 0.15, "", "natural"),
        ("dress", "fit-and-flare" | "fit and flare" | "peplum") => {
            (false, "fitted", 0.0, "flare", "natural")
        }
        ("dress", "skater" | "ball gown" | "ballgown" | "princess" | "circle") => {
            (false, "fitted", 0.0, "full-circle", "natural")
        }
        ("dress", "mermaid") => (false, "fitted", 0.0, "mermaid", "natural"),
        (
            "dress",
            "slip" | "strapless" | "tube" | "empire" | "babydoll" | "gathered" | "dirndl"
            | "tiered" | "drop-waist" | "wrap" | "smock",
        ) => {
            return Err(format!(
                "Rust drafting for silhouette {silhouette:?} is not implemented"
            ))
        }
        ("dress", _) => (true, "regular", 0.30, "", "natural"),
        (
            "skirt",
            "straight" | "a-line" | "pencil" | "flare" | "half-circle" | "full-circle" | "mermaid",
        ) => (false, "regular", 0.0, silhouette.as_str(), "natural"),
        ("skirt", _) => (false, "regular", 0.0, "a-line", "natural"),
        _ => (true, "regular", 0.05, "", "natural"),
    };
    normalized.insert("garment".into(), json!(garment));
    normalized.insert("silhouette".into(), json!(silhouette));
    let default_length = if garment == "top" { "hip" } else { "knee" };
    if garment != "skirt" {
        normalized.entry("one_piece").or_insert(json!(one_piece));
        if one_piece {
            normalized.entry("flare").or_insert(json!(flare));
        }
    }
    if !skirt.is_empty() {
        normalized.entry("skirt").or_insert(json!(skirt));
    }
    normalized.entry("fit").or_insert(json!(fit));
    normalized.entry("neckline").or_insert(json!("round"));
    let back = if ["deep-v", "halter"]
        .iter()
        .any(|s| normalized["neckline"] == *s)
    {
        "v"
    } else {
        "round"
    };
    normalized.entry("back_neckline").or_insert(json!(back));
    normalized.entry("sleeve").or_insert(json!("none"));
    normalized.entry("sleeve_style").or_insert(json!("regular"));
    normalized.entry("waist").or_insert(json!(waist));
    normalized.entry("length").or_insert(json!(default_length));
    normalized.entry("straps").or_insert(json!("none"));
    normalized.entry("strapless").or_insert(json!(false));
    let normalized = Value::Object(normalized);
    if string(&normalized, "straps", "none") != "none" || boolean(&normalized, "strapless", false) {
        return Err("Rust drafting for straps and strapless shapes is not implemented".into());
    }
    Ok(normalized)
}

struct Measurements {
    bust: f64,
    waist: f64,
    hip: f64,
    neck: f64,
    shoulder: f64,
    top_y: f64,
    shoulder_y: f64,
    chest_y: f64,
    underbust_y: f64,
    standard_waist_y: f64,
    waist_y: f64,
    hip_y: f64,
}

fn measurements(body: &Value, spec: &Value) -> Result<Measurements, String> {
    let (bust, waist, hip) = match string(spec, "fit", "regular") {
        "bodycon" => (0.0, -10.0, 0.0),
        "fitted" => (40.0, 30.0, 40.0),
        "regular" => (80.0, 70.0, 80.0),
        "loose" => (150.0, 140.0, 140.0),
        "oversized" => (260.0, 260.0, 240.0),
        other => return Err(format!("unknown ease profile {other:?}")),
    };
    let width = |key, ease| (number(body, key, 800.0) + number(&spec["ease"], key, ease)) / 4.0;
    let standard_waist = number(body, "h_waist", 1141.0);
    let underbust = number(body, "h_underbust", 1235.0);
    let waist_height = match string(spec, "waist", "natural") {
        "natural" | "none" => standard_waist,
        "high" => standard_waist + 40.0,
        "empire" => underbust - 10.0,
        "drop" => number(body, "h_high_hip", 1011.0) + 10.0,
        other => return Err(format!("unknown waist setting {other:?}")),
    };
    Ok(Measurements {
        bust: width("bust", bust),
        waist: width("waist", waist),
        hip: width("hip", hip),
        neck: number(body, "neck", 355.0) / 6.0 + 8.0,
        shoulder: number(body, "across_shoulder", 377.0) * 0.5,
        top_y: number(body, "h_hps", 1500.0),
        shoulder_y: number(body, "h_shoulder", 1454.0),
        chest_y: number(body, "h_apex", 1290.0),
        waist_y: waist_height,
        underbust_y: underbust,
        standard_waist_y: standard_waist,
        hip_y: number(body, "h_hip", 879.0),
    })
}

fn hem_height(body: &Value, spec: &Value) -> Result<f64, String> {
    if let Some(length) = spec["length"].as_f64() {
        let mm = if length < 40.0 {
            length * 1000.0
        } else {
            length
        };
        if mm >= 0.0 && mm < number(body, "height", 1755.0) {
            return Ok(mm);
        }
        return Err("length must put the hem between the floor and avatar height".into());
    }
    let waist = number(body, "h_waist", 1141.0);
    let hip = number(body, "h_hip", 879.0);
    let high_hip = number(body, "h_high_hip", 1011.0);
    let knee = number(body, "h_knee", 483.0);
    let ankle = number(body, "h_ankle", 86.0);
    let name = string(spec, "length", "knee")
        .to_lowercase()
        .replace('_', "-");
    let height = match name.as_str() {
        "cropped" => waist + 60.0,
        "waist" => waist - 15.0,
        "hip" => high_hip - 20.0,
        "tunic" => hip - 60.0,
        "micro" => number(body, "h_crotch", 841.0) + 60.0,
        "mini" => knee + 190.0,
        "above-knee" | "above the knee" => knee + 90.0,
        "knee" => knee - 15.0,
        "below-knee" => knee - 90.0,
        "midi" => (knee + ankle) / 2.0 + 20.0,
        "tea" | "tea-length" => ankle + 160.0,
        "ankle" => ankle + 30.0,
        "maxi" => ankle + 15.0,
        "floor" | "floor-length" => 12.0,
        _ if spec["garment"] == "top" => high_hip - 20.0,
        _ => knee - 15.0,
    };
    Ok(height)
}

fn neck_shape(kind: &str, front: bool) -> Result<(f64, f64, bool), String> {
    let curve = match (kind, front) {
        ("round", true) => (85.0, 5.0, true),
        ("round", false) => (25.0, 0.0, true),
        ("crew", _) => (70.0, 0.0, true),
        ("scoop", true) => (135.0, 10.0, true),
        ("scoop", false) => (90.0, 10.0, true),
        ("v", true) => (170.0, 5.0, false),
        ("v", false) => (150.0, 5.0, false),
        ("deep-v", true) => (250.0, 8.0, false),
        ("deep-v", false) => (260.0, 8.0, false),
        ("square", _) => (if front { 130.0 } else { 60.0 }, 25.0, false),
        ("sweetheart", true) => (150.0, 30.0, true),
        ("boat", _) => (40.0, 55.0, true),
        ("high", true) => (35.0, 0.0, true),
        ("high", false) => (15.0, 0.0, true),
        ("open", false) => (330.0, 0.0, true),
        _ => {
            return Err(format!(
                "Rust drafting for neckline {kind:?} is not implemented"
            ))
        }
    };
    Ok(curve)
}

fn edge_with_points(name: &'static str, start: [f64; 2], intermediate: Vec<[f64; 3]>) -> Edge {
    Edge {
        name,
        start,
        intermediate,
    }
}

fn side_edge(name: &'static str, stations: &[[f64; 2]], reverse: bool) -> Edge {
    if reverse {
        let last = stations.last().unwrap();
        edge_with_points(
            name,
            [-last[0], last[1]],
            stations[1..stations.len() - 1]
                .iter()
                .rev()
                .map(|p| [-p[0], p[1], 2.0])
                .collect(),
        )
    } else {
        edge_with_points(
            name,
            stations[0],
            stations[1..stations.len() - 1]
                .iter()
                .map(|p| [p[0], p[1], 2.0])
                .collect(),
        )
    }
}

fn armhole(name: &'static str, a: [f64; 2], b: [f64; 2], downward: bool) -> Edge {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = dx.hypot(dy).max(1e-6);
    let nx = -dy / len;
    let ny = dx / len;
    let toward_center = if (a[0] + b[0]) * nx <= 0.0 { 1.0 } else { -1.0 };
    let bulge = (22.0 + 0.12 * dy.abs()) * toward_center;
    let cp = (1..=3)
        .map(|i| {
            let t = f64::from(i) / 4.0;
            let bias = if downward { t } else { 1.0 - t };
            let deviation = bulge * (std::f64::consts::PI * t).sin() * (0.55 + 0.9 * bias);
            [
                a[0] + dx * t + nx * deviation,
                a[1] + dy * t + ny * deviation,
                2.0,
            ]
        })
        .collect();
    edge_with_points(name, a, cp)
}

fn neckline(kind: &str, front: bool, w: f64, hps: f64) -> Result<Edge, String> {
    let (depth, _, _) = neck_shape(kind, front)?;
    let y = hps - depth;
    let cp = match kind {
        "v" | "deep-v" | "halter" => vec![[0.0, y, 0.0]],
        "square" => vec![[-w, y, 0.0], [w, y, 0.0]],
        "sweetheart" => vec![
            [-w - 5.0, y + 45.0, 2.0],
            [-w * 0.5, y - 5.0, 2.0],
            [0.0, y + 35.0, 0.0],
            [w * 0.5, y - 5.0, 2.0],
            [w + 5.0, y + 45.0, 2.0],
        ],
        "open" if !front => vec![
            [-w * 0.85, hps - depth * 0.5, 2.0],
            [0.0, y, 2.0],
            [w * 0.85, hps - depth * 0.5, 2.0],
        ],
        _ if front => vec![
            [-w * 0.92, hps - depth * 0.45, 2.0],
            [-w * 0.6, hps - depth * 0.86, 2.0],
            [0.0, y, 2.0],
            [w * 0.6, hps - depth * 0.86, 2.0],
            [w * 0.92, hps - depth * 0.45, 2.0],
        ],
        _ => vec![
            [-w * 0.6, hps - depth * 0.85, 2.0],
            [0.0, y, 2.0],
            [w * 0.6, hps - depth * 0.85, 2.0],
        ],
    };
    Ok(edge_with_points("neckline", [-w, hps], cp))
}

fn curved_hem(hip_width: f64, hip_y: f64, hem_width: f64, hem_y: f64) -> (Edge, f64) {
    let length = hip_y - hem_y;
    let spread = hem_width - hip_width;
    let dy = (length * length - spread * spread)
        .max((length * 0.5).powi(2))
        .sqrt();
    let side = hip_y - (length - 0.7 * (length - dy));
    let samples = [0.25, 0.5, 0.75]
        .into_iter()
        .map(|t: f64| {
            let u = 2.0 * t - 1.0;
            [-hem_width * u, side + (hem_y - side) * (1.0 - u * u), 2.0]
        })
        .collect();
    (edge_with_points("hem", [hem_width, side], samples), side)
}

fn body_panel(
    m: &Measurements,
    spec: &Value,
    bottom: f64,
    hem_width: Option<f64>,
    front: bool,
) -> Result<Panel, String> {
    let kind = string(
        spec,
        if front { "neckline" } else { "back_neckline" },
        "round",
    );
    let (_, extra_width, _) = neck_shape(string(spec, "neckline", "round"), true)?;
    let neck = (m.neck + extra_width).min(m.shoulder - 85.0);
    let balance = if front { 8.0 } else { -8.0 };
    let bust = m.bust + balance;
    let waist = m.waist + balance * 0.5;
    let hip = m.hip + balance * 0.5;
    let underarm_y = m.chest_y
        + if spec["fit"] == "oversized" {
            -15.0
        } else {
            25.0
        };
    if bottom >= underarm_y {
        return Err("the hem must fall below the armhole".into());
    }
    let mut stations = vec![[bust, underarm_y]];
    let bottom_width = if bottom >= m.standard_waist_y - 5.0 {
        if bottom < m.underbust_y {
            stations.push([waist + (bust - waist) * 0.35, m.underbust_y]);
        }
        if bottom <= m.standard_waist_y + 5.0 {
            waist
        } else {
            waist + (bust - waist) * 0.5
        }
    } else {
        stations.push([waist, m.standard_waist_y]);
        if bottom < m.hip_y - 20.0 {
            stations.push([hip, m.hip_y]);
        }
        hem_width.unwrap_or(hip)
    };
    let (hem, side_y) = if bottom < m.hip_y - 60.0 && bottom_width > hip + 25.0 {
        curved_hem(hip, m.hip_y, bottom_width, bottom)
    } else {
        (edge("hem", bottom_width, bottom), bottom)
    };
    stations.push([bottom_width, side_y]);
    let (name, role, arrange, s1, a1, l1, l2, a2, s2) = if front {
        (
            "Front",
            "bodice_front",
            "Body_Front_Center_3",
            "shoulder_L",
            "armhole_L",
            "side_L",
            "side_R",
            "armhole_R",
            "shoulder_R",
        )
    } else {
        (
            "Back",
            "bodice_back",
            "Body_Back_Center_3",
            "shoulder_R",
            "armhole_R",
            "side_R",
            "side_L",
            "armhole_L",
            "shoulder_L",
        )
    };
    Ok(Panel {
        name,
        role,
        arrange,
        elastic: vec![],
        edges: vec![
            edge(s1, neck, m.top_y),
            armhole(a1, [m.shoulder, m.shoulder_y], [bust, underarm_y], true),
            side_edge(l1, &stations, false),
            hem,
            side_edge(l2, &stations, true),
            armhole(a2, [-bust, underarm_y], [-m.shoulder, m.shoulder_y], false),
            edge(s2, -m.shoulder, m.shoulder_y),
            neckline(kind, front, neck, m.top_y)?,
        ],
    })
}

fn skirt_panel(
    m: &Measurements,
    kind: &str,
    bottom: f64,
    front: bool,
    waist_half_width: Option<f64>,
    lower: Option<f64>,
) -> Result<Panel, String> {
    if bottom > m.waist_y - 30.0 {
        return Err("skirt hem must fall below the waist".into());
    }
    let length = m.waist_y - bottom;
    let w = waist_half_width.unwrap_or(m.waist);
    let flare = match kind {
        "straight" => 0.02,
        "pencil" => -0.05,
        "column" => 0.0,
        "a-line" => 0.32,
        "flare" | "flared" => 0.60,
        "trumpet" => 0.50,
        "mermaid" => 0.55,
        "tulip" => 0.15,
        "half-circle" => 1.50,
        "full-circle" => 2.25,
        _ => {
            return Err(format!(
                "Rust drafting for skirt shape {kind:?} is not implemented"
            ))
        }
    };
    let hem_width = lower.unwrap_or((m.hip + flare * length / 2.0).max(m.hip * 0.80));
    let curve = hem_width > m.hip + 25.0 && (lower.is_none() || bottom < m.hip_y - 60.0);
    let (hem, side) = if curve {
        let (radius, at) = if lower.is_some() {
            (m.hip, m.hip_y)
        } else {
            (w, m.waist_y)
        };
        curved_hem(radius, at, hem_width, bottom)
    } else {
        (edge("hem", hem_width, bottom), bottom)
    };
    let mut stations = vec![[w, m.waist_y]];
    if length > 200.0 && m.hip_y < m.waist_y - 40.0 && bottom < m.hip_y - 30.0 {
        stations.push([m.hip, m.hip_y]);
    }
    stations.push([hem_width, side]);
    let (name, role, arrange, l, r) = if front {
        (
            if lower.is_some() {
                "Lower_Front"
            } else {
                "Skirt_Front"
            },
            "skirt_front",
            "Leg_Skirt_Front",
            "side_L",
            "side_R",
        )
    } else {
        (
            if lower.is_some() {
                "Lower_Back"
            } else {
                "Skirt_Back"
            },
            "skirt_back",
            "Leg_Skirt_Back",
            "side_R",
            "side_L",
        )
    };
    Ok(Panel {
        name,
        role,
        arrange,
        elastic: vec![],
        edges: vec![
            edge("waist", -w, m.waist_y),
            side_edge(l, &stations, false),
            hem,
            side_edge(r, &stations, true),
        ],
    })
}

fn sleeve_cap(a: [f64; 2], b: [f64; 2], low: f64, high: f64) -> Vec<[f64; 3]> {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = dx.hypot(dy).max(1e-6);
    [0.25, 0.5, 0.75]
        .into_iter()
        .map(|t: f64| {
            let bump = if t < 0.5 { -low } else { high } * (std::f64::consts::PI * t).sin();
            [
                a[0] + t * dx - dy * bump / len,
                a[1] + t * dy + dx * bump / len,
                2.0,
            ]
        })
        .collect()
}

fn sleeve_panel(
    body: &Value,
    spec: &Value,
    armhole_length: f64,
    left: bool,
) -> Result<Panel, String> {
    let style = string(spec, "sleeve_style", "regular").to_lowercase();
    let key = string(spec, "sleeve", "short")
        .to_lowercase()
        .replace('_', "-");
    let arm_length = number(body, "arm_length", 589.0);
    let length = match key.as_str() {
        "cap" => 70.0,
        "short" => 220.0,
        "elbow" => 330.0,
        "three-quarter" => 440.0,
        "long" => arm_length + 15.0,
        _ => arm_length - 60.0,
    };
    let bicep = number(body, "bicep", 257.0);
    let wrist = number(body, "wrist", 141.0);
    let h = number(body, "h_shoulder", 1454.0) + 20.0;
    let mut ease = match style.as_str() {
        "fitted" => 30.0,
        "puff" => 170.0,
        "bishop" => 120.0,
        "flutter" | "bell" => 60.0,
        "balloon" => 200.0,
        _ => 55.0,
    };
    if ["loose", "oversized"].iter().any(|fit| spec["fit"] == *fit) {
        ease += 40.0;
    }
    let width = bicep + ease;
    if width < 60.0 {
        return Err("sleeve width is too small for the cap".into());
    }
    let w = width / 2.0;
    let top = [0.0, h];
    let target = match style.as_str() {
        "puff" | "balloon" => armhole_length * 1.45,
        "fitted" | "regular" => armhole_length + 12.0,
        _ => armhole_length + 25.0,
    };
    let caps = |height: f64| {
        let y = h - height;
        let start = [-w, y];
        let end = [w, y];
        let front = edge_with_points(
            "cap_front",
            start,
            sleeve_cap(start, top, -height * 0.16, height * 0.22),
        );
        let back = edge_with_points(
            "cap_back",
            top,
            sleeve_cap(top, end, height * 0.18, -height * 0.10),
        );
        let len = edge_length(&front, top) + edge_length(&back, end);
        (front, back, len, y)
    };
    let mut cap_height = (armhole_length * 0.30).max(80.0);
    for _ in 0..25 {
        let (_, _, len, _) = caps(cap_height);
        if (len - target).abs() < 2.0 {
            break;
        }
        cap_height = (cap_height * (target / len).powf(1.4)).clamp(50.0, width * 0.9);
    }
    let (front, back, _, y) = caps(cap_height);
    let length = length.max(cap_height + 55.0);
    let hem_y = h - length;
    let is_long = key == "long" || key == "bracelet";
    let cuff = match (is_long, style.as_str()) {
        (true, "fitted") => (wrist + 70.0) / 2.0,
        (true, _) => (wrist + 110.0) / 2.0,
        (_, "flutter" | "bell") => w + length * 0.35,
        (_, "puff" | "balloon" | "bishop") => w * 0.95,
        _ if length > 250.0 => w * 0.90,
        _ => w * 0.96,
    };
    let elastic = if ["puff", "balloon", "bishop"].contains(&style.as_str()) {
        vec![(
            3,
            if length < 300.0 {
                bicep + 40.0
            } else {
                wrist + 60.0
            },
        )]
    } else {
        vec![]
    };
    let (name, role, arrange, edges) = if left {
        (
            "Sleeve_L",
            "sleeve_L",
            "Arm_Outside_2_L",
            vec![
                front,
                back,
                edge("underarm_back", w, y),
                edge("hem", cuff, hem_y),
                edge("underarm_front", -cuff, hem_y),
            ],
        )
    } else {
        let mirrored = |edge: Edge, start: [f64; 2]| {
            edge_with_points(
                edge.name,
                start,
                edge.intermediate
                    .into_iter()
                    .rev()
                    .map(|p| [-p[0], p[1], 2.0])
                    .collect(),
            )
        };
        (
            "Sleeve_R",
            "sleeve_R",
            "Arm_Outside_2_R",
            vec![
                mirrored(back, [-w, y]),
                mirrored(front, top),
                edge("underarm_front", w, y),
                edge("hem", cuff, hem_y),
                edge("underarm_back", -cuff, hem_y),
            ],
        )
    };
    Ok(Panel {
        name,
        role,
        arrange,
        elastic,
        edges,
    })
}

fn seam(
    pieces: &[Value],
    a: usize,
    edge_a: &str,
    b: usize,
    edge_b: &str,
    reverse: bool,
) -> Result<Value, String> {
    let find = |panel: usize, name: &str| {
        array(&pieces[panel]["edges"])
            .iter()
            .find(|e| e["name"] == name)
            .ok_or_else(|| format!("unknown drafting edge {name}"))
    };
    let ea = find(a, edge_a)?;
    let eb = find(b, edge_b)?;
    Ok(
        json!({"piece_a":pieces[a]["name"],"line_a":ea["line"],"piece_b":pieces[b]["name"],"line_b":eb["line"],
        "dir_a":true,"dir_b":!reverse,"len_a":ea["length"],"len_b":eb["length"],"gather":false,"note":""}),
    )
}

fn plan(spec: &Value, body: &Value, source: &str) -> Result<Value, String> {
    let spec = normalize(spec)?;
    let mut m = measurements(body, &spec)?;
    let hem = hem_height(body, &spec)?;
    let garment = spec["garment"].as_str().unwrap();
    let has_bodice = garment != "skirt";
    let one_piece = boolean(&spec, "one_piece", true);
    let split = has_bodice && one_piece && hem < m.standard_waist_y - 150.0;
    let has_skirt = garment == "skirt" || (has_bodice && !one_piece);
    let sleeves = has_bodice && spec["sleeve"] != "none";
    let mut pieces = Vec::<Value>::new();
    let mut seams = Vec::<Value>::new();
    let mut merges = Vec::<Value>::new();
    let mut notes = Vec::<Value>::new();
    let mut armhole_length = 0.0;
    let draft_hem = if one_piece {
        if ["pencil", "bodycon"]
            .iter()
            .any(|s| spec["silhouette"] == *s)
            && hem < number(body, "h_knee", 483.0) + 30.0
        {
            Some(m.hip - 6.0)
        } else if spec["silhouette"] == "sheath" {
            Some(m.hip + 8.0)
        } else {
            Some(m.hip + number(&spec, "flare", 0.0) * (m.hip_y - hem).max(0.0) * 0.5)
        }
    } else {
        None
    };
    if has_bodice {
        let bodice_hem = if split {
            m.standard_waist_y
        } else if has_skirt {
            m.waist_y
        } else {
            hem
        };
        let front = body_panel(&m, &spec, bodice_hem, draft_hem, true)?;
        let back = body_panel(&m, &spec, bodice_hem, draft_hem, false)?;
        armhole_length = front.line_length(1) + back.line_length(5);
        pieces.extend([front.json(-520.0), back.json(520.0)]);
        if split {
            let width = |a: usize| number(&pieces[a]["edges"][3], "length", 0.0) / 2.0;
            m.waist_y = m.standard_waist_y;
            pieces.extend([
                skirt_panel(&m, "a-line", hem, true, Some(width(0)), draft_hem)?.json(-520.0),
                skirt_panel(&m, "a-line", hem, false, Some(width(1)), draft_hem)?.json(520.0),
            ]);
            for edge in ["side_L", "side_R"] {
                seams.push(seam(&pieces, 2, edge, 3, edge, true)?);
            }
            seams.push(seam(&pieces, 0, "hem", 2, "waist", true)?);
            seams.push(seam(&pieces, 1, "hem", 3, "waist", true)?);
            merges.extend([
                json!(["Front", "Lower_Front"]),
                json!(["Back", "Lower_Back"]),
            ]);
            notes.push(json!("one-piece dress built as bodice + lower piece for a stable drape; single-piece mesh joining after simulation is pending"));
        }
        for edge in ["side_L", "side_R", "shoulder_L", "shoulder_R"] {
            seams.push(seam(&pieces, 0, edge, 1, edge, true)?);
        }
    }
    if sleeves {
        let i = pieces.len();
        pieces.extend([
            sleeve_panel(body, &spec, armhole_length, true)?.json(-1300.0),
            sleeve_panel(body, &spec, armhole_length, false)?.json(1300.0),
        ]);
        let gather = ["puff", "balloon"]
            .iter()
            .any(|name| spec["sleeve_style"] == *name);
        for (index, arm) in ["armhole_L", "armhole_R"].into_iter().enumerate() {
            let sleeve = i + index;
            seams.push(seam(
                &pieces,
                sleeve,
                "underarm_back",
                sleeve,
                "underarm_front",
                true,
            )?);
            let mut front = seam(&pieces, sleeve, "cap_front", 0, arm, true)?;
            let mut back = seam(&pieces, sleeve, "cap_back", 1, arm, true)?;
            front["gather"] = json!(gather);
            back["gather"] = json!(gather);
            seams.extend([front, back]);
        }
    }
    if has_skirt {
        let i = pieces.len();
        let w = has_bodice.then(|| number(&pieces[0]["edges"][3], "length", 0.0) / 2.0);
        let style = string(&spec, "skirt", "a-line");
        pieces.extend([
            skirt_panel(&m, style, hem, true, w, None)?.json(-520.0),
            skirt_panel(&m, style, hem, false, w, None)?.json(520.0),
        ]);
        for edge in ["side_L", "side_R"] {
            seams.push(seam(&pieces, i, edge, i + 1, edge, true)?);
        }
        if has_bodice {
            seams.push(seam(&pieces, 0, "hem", i, "waist", true)?);
            seams.push(seam(&pieces, 1, "hem", i + 1, "waist", true)?);
        }
    }
    let warnings: Vec<_> = seams
        .iter()
        .filter_map(|s| {
            let a = number(s, "len_a", 0.0);
            let b = number(s, "len_b", 0.0);
            (!boolean(s, "gather", false) && a.min(b) > 0.0 && a.max(b) / a.min(b) > 1.08).then(
                || {
                    format!(
                        "seam {:?} to {:?} differs by {:.0}%",
                        s["piece_a"],
                        s["piece_b"],
                        100.0 * (a.max(b) / a.min(b) - 1.0)
                    )
                },
            )
        })
        .collect();
    let mut material = assets::selected_material(string(&spec, "fabric", ""))?;
    material["color"] = json!(assets::parse_color(&spec["color"])?);
    material["color_text"] = spec["color"].clone();
    material.as_object_mut().unwrap().remove("path");
    let steps =
        spec["simulate_steps"]
            .as_u64()
            .unwrap_or(if hem < number(body, "h_knee", 483.0) - 100.0 {
                160
            } else {
                120
            });
    let particle = number(&spec, "particle_distance", 20.0);
    Ok(
        json!({"pieces":pieces,"seams":seams,"fabric":material,"simulation":{"steps":steps,"particle_distance":particle},
        "warnings":warnings,"notes":notes,"merge_pairs":merges,
        "normalized_spec":spec,"spec":spec,"measurement_source":source}),
    )
}

pub fn preview(args: &Value, state: &Value) -> Result<Value, String> {
    let (body, source) = if boolean(args, "use_scene_measurements", false) {
        if state["avatar"].is_null() {
            return Err(
                "load an avatar first, or preview with use_scene_measurements=false".into(),
            );
        }
        assets::body_measurements(&state["avatar"], false)
    } else {
        (
            assets::default_body(),
            "standard default measurements; not fitted to a live avatar".to_owned(),
        )
    };
    let mut result = plan(&args["spec"], &body, &source)?;
    for panel in array_mut(&mut result, "pieces") {
        let obj = panel.as_object_mut().unwrap();
        obj.remove("points");
        obj.remove("draft_offset_x");
    }
    Ok(result)
}

pub fn last_spec(state: &Value) -> Value {
    json!({"spec":state["garment_spec"],"pieces":state["garment_piece_names"].as_array().cloned().unwrap_or_default()})
}

pub fn build(args: &Value, state: &mut Value, rebuild: bool) -> Result<Value, String> {
    let mut next = state.clone();
    let spec = if rebuild {
        let mut saved = next["garment_spec"]
            .as_object()
            .cloned()
            .ok_or("nothing built yet in this session; call garment_build")?;
        let changed = args["changes"]
            .as_object()
            .ok_or("changes must be an object")?;
        saved.extend(changed.iter().map(|(k, v)| (k.clone(), v.clone())));
        Value::Object(saved)
    } else {
        args["spec"].clone()
    };
    if next["avatar"].is_null() {
        assets::avatar_load(
            &json!({"name_or_path":string(&spec,"avatar","mia")}),
            &mut next,
        )?;
    }
    let (body, source) = assets::body_measurements(&next["avatar"], false);
    let draft = plan(&spec, &body, &source)?;
    let replace = rebuild || boolean(args, "replace_existing", true);
    let checkpoint = if replace && !array(&next["patterns"]).is_empty() {
        crate::compat_scene::dispatch("checkpoint_save", &json!({}), &mut next)
            .and_then(Result::ok)
            .map(|value| value["path"].clone())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    if replace {
        next["patterns"] = json!([]);
        next["seams"] = json!([]);
    }
    let rendered_panels = array(&draft["pieces"]);
    let indices = array(&next["patterns"]);
    let next_id = indices
        .iter()
        .filter_map(|p| p["id"].as_u64())
        .max()
        .map_or(0, |n| n + 1);
    let mut ids = Map::new();
    let mut summary = Map::new();
    let mut names = Vec::new();
    for (position, panel) in rendered_panels.iter().enumerate() {
        let name = panel["name"].as_str().unwrap().to_owned();
        let id = next_id + position as u64;
        ids.insert(name.clone(), json!(id));
        names.push(json!(name));
        let mut inserted = panel.clone();
        inserted["id"] = json!(id);
        array_mut(&mut next, "patterns").push(inserted);
        let lines: Vec<_> = array(&panel["edges"])
            .iter()
            .map(|e| json!([e["line"], e["name"], e["length"]]))
            .collect();
        summary.insert(name,json!({"index":array(&next["patterns"]).len()-1,"arrangement":panel["arrangement"],"lines":lines}));
    }
    for (i, seam) in array(&draft["seams"]).iter().enumerate() {
        let a = seam["piece_a"].as_str().unwrap();
        let b = seam["piece_b"].as_str().unwrap();
        let state_seams = array_mut(&mut next, "seams");
        let seam_id = state_seams.len();
        state_seams.push(json!({"id":seam_id,"name":format!("{a}/{b}/{}",i+1),"pattern_a":ids[a],"pattern_b":ids[b],
            "line_a":seam["line_a"],"line_b":seam["line_b"],"dir_a":seam["dir_a"],"dir_b":seam["dir_b"]}));
    }
    let fabric = assets::fabric_apply(
        &json!({"fabric":string(&draft["spec"],"fabric",""),"color":draft["fabric"]["color"],
        "patterns":ids.values().cloned().collect::<Vec<_>>()}),
        &mut next,
    )?;
    next["simulation"] = Value::Null;
    let steps = args["simulate_steps"]
        .as_i64()
        .or_else(|| draft["simulation"]["steps"].as_i64())
        .unwrap_or(120);
    let sim = if steps > 0 {
        solver::simulate(
            &json!({"steps":steps,"particle_distance":draft["simulation"]["particle_distance"]}),
            &mut next,
        )?
    } else {
        Value::Null
    };
    next["garment_spec"] = draft["spec"].clone();
    next["garment_piece_names"] = json!(names);
    let images = if boolean(args, "snapshots", true) {
        let views = if rebuild || args["views"].is_null() {
            json!(["front", "3/4-right", "back"])
        } else {
            args["views"].clone()
        };
        render::snapshot(&json!({"views":views,"label":"build"}), &next, false)?["images"].clone()
    } else {
        json!([])
    };
    let result = json!({"spec":draft["spec"],"avatar":next["avatar"]["name"],"measurements":source,
        "pieces":summary,"seams":draft["seams"],"fabric":{"file":fabric["file"],"matched":fabric["matched"],"color":draft["fabric"]["color"]},
        "arrangement":[],"warnings":draft["warnings"],"notes":draft["notes"],"simulation":sim,"checkpoint_before_build":checkpoint,
        "images":images});
    *state = next;
    Ok(result)
}
