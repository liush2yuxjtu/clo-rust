use super::{array, is_running, text, DomainCall, ToolResult};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const WIDTH: usize = 1024;
const HEIGHT: usize = 720;
const CANVAS_X: f64 = 264.0;
const CANVAS_Y: f64 = 110.0;
const ROW_Y: f64 = 151.0;
const ROW_HEIGHT: f64 = 32.0;
const BUTTONS: [(&str, &str, usize, usize); 4] = [
    ("add-pattern", "Add rectangle", 20, 126),
    ("delete-pattern", "Delete selected", 155, 133),
    ("simulate", "Simulate", 298, 106),
    ("reset", "Reset simulation", 414, 142),
];
const PATTERN_BASE: usize = 10;

fn clock() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis())
}

fn validate_window(state: &Value) -> Result<(), String> {
    if is_running(state) {
        Ok(())
    } else {
        Err("The CLO Rust workbench is not running".into())
    }
}

struct Node<'a> {
    id: usize,
    parent: Option<usize>,
    role: &'a str,
    title: &'a str,
    identifier: &'a str,
    enabled: bool,
    actions: &'a [&'a str],
}

impl Node<'_> {
    fn value(&self) -> Value {
        json!({"id":self.id,"parent":self.parent,"AXRole":self.role,"AXTitle":self.title,
            "AXIdentifier":self.identifier,"AXEnabled":self.enabled,"actions":self.actions})
    }
}

fn scene_patterns(state: &Value) -> &[Value] {
    array(&state["patterns"])
}

fn selection(state: &Value) -> Option<usize> {
    let selected = state["runtime"]["ui"]["selected"].as_u64()?;
    usize::try_from(selected)
        .ok()
        .filter(|index| *index < scene_patterns(state).len())
}

fn current_name(state: &Value) -> &str {
    selection(state)
        .and_then(|index| state["patterns"][index]["name"].as_str())
        .unwrap_or_default()
}

fn nodes(state: &Value, max_depth: u64) -> Vec<Value> {
    let mut nodes = vec![Node {
        id: 0,
        parent: None,
        role: "AXApplication",
        title: "CLO Rust",
        identifier: "workbench",
        enabled: true,
        actions: &[],
    }
    .value()];
    nodes.push(
        Node {
            id: 1,
            parent: Some(0),
            role: "AXWindow",
            title: "Pattern workbench",
            identifier: "main-window",
            enabled: true,
            actions: &[],
        }
        .value(),
    );
    if max_depth < 2 {
        return nodes;
    }
    for (index, (identifier, title, _, _)) in BUTTONS.iter().enumerate() {
        let enabled = if index == 1 {
            selection(state).is_some()
        } else {
            true
        };
        nodes.push(
            Node {
                id: index + 2,
                parent: Some(1),
                role: "AXButton",
                title,
                identifier,
                enabled,
                actions: &["AXPress"],
            }
            .value(),
        );
    }
    nodes.push(
        Node {
            id: 6,
            parent: Some(1),
            role: "AXOutline",
            title: "Patterns",
            identifier: "pattern-list",
            enabled: true,
            actions: &[],
        }
        .value(),
    );
    nodes.push(
        Node {
            id: 7,
            parent: Some(1),
            role: "AXCanvas",
            title: "Drafting area",
            identifier: "drafting-area",
            enabled: true,
            actions: &[],
        }
        .value(),
    );
    let mut name = Node {
        id: 8,
        parent: Some(1),
        role: "AXTextField",
        title: "Pattern name",
        identifier: "pattern-name",
        enabled: selection(state).is_some(),
        actions: &[],
    }
    .value();
    name["AXValue"] = json!(state["runtime"]["ui"]["name"]
        .as_str()
        .unwrap_or_else(|| current_name(state)));
    nodes.push(name);
    if state["runtime"]["ui"]["menu_x"].is_number() {
        nodes.push(
            Node {
                id: 9,
                parent: Some(1),
                role: "AXMenuItem",
                title: "Delete selected pattern",
                identifier: "context-delete",
                enabled: selection(state).is_some(),
                actions: &["AXPress"],
            }
            .value(),
        );
    }
    if max_depth >= 3 {
        for (index, pattern) in scene_patterns(state).iter().enumerate() {
            let name = pattern["name"].as_str().unwrap_or("Unnamed pattern");
            let mut row = Node {
                id: PATTERN_BASE + index,
                parent: Some(6),
                role: "AXRow",
                title: name,
                identifier: "pattern",
                enabled: true,
                actions: &["AXPress"],
            }
            .value();
            row["AXValue"] = json!(selection(state) == Some(index));
            nodes.push(row);
        }
    }
    nodes
}

pub(super) fn state(args: &Value, state: &mut Value) -> ToolResult {
    validate_window(state)?;
    let max_nodes = args["max_nodes"].as_u64().unwrap_or(1500);
    let max_depth = args["max_depth"].as_u64().unwrap_or(16);
    if !(1..=5000).contains(&max_nodes) || !(1..=30).contains(&max_depth) {
        return Err("max_nodes 1..5000 and max_depth 1..30 required".into());
    }
    let mut nodes = nodes(state, max_depth);
    let truncated = nodes.len() > max_nodes as usize
        || max_depth < 2
        || (max_depth < 3 && !scene_patterns(state).is_empty());
    nodes.truncate(max_nodes as usize);
    let sequence = state["runtime"]["ui"]["sequence"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    let now = clock();
    let token = format!("{:08x}-{now:x}-{sequence:x}", std::process::id());
    state["runtime"]["ui"]["sequence"] = json!(sequence);
    state["runtime"]["ui"]["observed"] = json!({"token":token,"milliseconds":now as u64,"nodes":nodes.iter().map(|node| node["id"].clone()).collect::<Vec<_>>()});
    Ok(
        json!({"snapshot":token,"pid":std::process::id(),"nodes":nodes,"truncated":truncated,
        "coordinate_system":"workbench viewport points, origin top-left, 1 point per screenshot pixel",
        "windows":[{"id":1,"name":"Pattern workbench","bounds":{"X":0,"Y":0,"Width":WIDTH,"Height":HEIGHT}}]}),
    )
}

fn select(state: &mut Value, index: Option<usize>) {
    state["runtime"]["ui"]["selected"] = json!(index);
    if let Some(name) = index.and_then(|index| state["patterns"][index]["name"].as_str()) {
        state["runtime"]["ui"]["name"] = json!(name);
    } else {
        state["runtime"]["ui"]["name"] = Value::Null;
    }
    state["runtime"]["ui"]["focus"] = Value::Null;
}

fn hide_menu(state: &mut Value) {
    state["runtime"]["ui"]["menu_x"] = Value::Null;
    state["runtime"]["ui"]["menu_y"] = Value::Null;
}

fn new_pattern(state: &mut Value, domain: DomainCall) -> Result<(), String> {
    let index = scene_patterns(state).len();
    let position = index as f64 * 45.0;
    let result = domain(
        "pattern_create",
        &json!({"name":format!("Rectangle {}",index+1),
        "points":[[position,0.0],[position+120.0,0.0],[position+120.0,180.0],[position,180.0]]}),
        state,
    )?;
    let index = result["index"]
        .as_u64()
        .map(|value| value as usize)
        .unwrap_or(index);
    select(state, Some(index));
    Ok(())
}

fn choose_button(id: usize, state: &mut Value, domain: DomainCall) -> Result<(), String> {
    match id {
        2 => new_pattern(state, domain),
        3 | 9 => {
            let index = selection(state).ok_or("Select a pattern before deleting it")?;
            domain(
                "pattern_delete",
                &json!({"patterns":[index.to_string()]}),
                state,
            )?;
            select(state, None);
            Ok(())
        }
        4 => {
            domain("simulate", &json!({"steps":100}), state)?;
            Ok(())
        }
        5 => {
            domain("simulation_reset", &json!({}), state)?;
            Ok(())
        }
        id if id >= PATTERN_BASE && id - PATTERN_BASE < scene_patterns(state).len() => {
            select(state, Some(id - PATTERN_BASE));
            Ok(())
        }
        _ => Err("Action not exposed by element".into()),
    }
}

fn rename(name: &str, state: &mut Value, domain: DomainCall) -> Result<(), String> {
    let index = selection(state).ok_or("Select a pattern before changing its name")?;
    if name.trim().is_empty() || name.chars().count() > 256 {
        return Err("Pattern name must contain 1..256 characters".into());
    }
    domain(
        "pattern_ops",
        &json!({"pattern":index.to_string(),"op":"rename","args":{"name":name}}),
        state,
    )?;
    state["runtime"]["ui"]["name"] = json!(name);
    Ok(())
}

fn point_valid(x: f64, y: f64) -> bool {
    x >= 0.0 && x < WIDTH as f64 && y >= 0.0 && y < HEIGHT as f64
}

fn click(
    x: f64,
    y: f64,
    double: bool,
    state: &mut Value,
    domain: DomainCall,
) -> Result<(), String> {
    if let (Some(menu_x), Some(menu_y)) = (
        state["runtime"]["ui"]["menu_x"].as_f64(),
        state["runtime"]["ui"]["menu_y"].as_f64(),
    ) {
        hide_menu(state);
        if (menu_x..menu_x + 180.0).contains(&x) && (menu_y..menu_y + 34.0).contains(&y) {
            return choose_button(9, state, domain);
        }
    }
    if (54.0..90.0).contains(&y) {
        for (index, (_, _, left, width)) in BUTTONS.iter().enumerate() {
            if (*left as f64..(*left + *width) as f64).contains(&x) {
                return choose_button(index + 2, state, domain);
            }
        }
    }
    if (ROW_Y..HEIGHT as f64).contains(&y) && (12.0..249.0).contains(&x) {
        let index = ((y - ROW_Y) / ROW_HEIGHT).floor() as usize;
        if index < scene_patterns(state).len() {
            select(state, Some(index));
            if double {
                state["runtime"]["ui"]["focus"] = json!("name");
            }
        }
        return Ok(());
    }
    if (574.0..800.0).contains(&x) && (54.0..90.0).contains(&y) && selection(state).is_some() {
        state["runtime"]["ui"]["focus"] = json!("name");
        return Ok(());
    }
    if x >= CANVAS_X && y >= CANVAS_Y {
        let selected = pattern_at(state, x, y, domain)?;
        select(state, selected);
    }
    Ok(())
}

pub(super) fn action(args: &Value, state_ref: &mut Value, domain: DomainCall) -> ToolResult {
    validate_window(state_ref)?;
    let token = text(args, "snapshot")?;
    let observed = &state_ref["runtime"]["ui"]["observed"];
    let observation_time = observed["milliseconds"].as_u64().unwrap_or(0);
    if observed["token"].as_str() != Some(token)
        || clock().saturating_sub(u128::from(observation_time)) > 60_000
    {
        return Err("Stale UI snapshot; call clo_ui_state again".into());
    }
    let action = text(args, "action")?;
    if ![
        "press",
        "set_value",
        "ax_action",
        "click",
        "right_click",
        "double_click",
        "drag",
        "key",
        "text",
        "scroll",
    ]
    .contains(&action)
    {
        return Err("Unknown UI action".into());
    }
    let value = args["value"].as_str().unwrap_or_default();
    let modifiers = array(&args["modifiers"]);
    if modifiers.iter().any(|modifier| {
        !matches!(
            modifier.as_str(),
            Some("command" | "shift" | "option" | "control")
        )
    }) {
        return Err("Unknown modifier".into());
    }
    let keycode = if args["keycode"].is_null() {
        0
    } else {
        args["keycode"]
            .as_u64()
            .filter(|value| *value <= 127)
            .ok_or("Invalid keycode")?
    };
    let x = args["x"].as_f64().unwrap_or(0.0);
    let y = args["y"].as_f64().unwrap_or(0.0);
    let end_x = args["end_x"].as_f64().unwrap_or(0.0);
    let end_y = args["end_y"].as_f64().unwrap_or(0.0);
    let element = args["element"]
        .as_u64()
        .filter(|id| array(&observed["nodes"]).contains(&json!(id)))
        .map(|id| id as usize);
    state_ref["runtime"]["ui"]["observed"] = Value::Null;
    match action {
        "press" | "ax_action" => {
            if action == "ax_action" && value != "AXPress" {
                return Err("Action not exposed by element".into());
            }
            choose_button(
                element.ok_or("Unknown element; inspect UI state first")?,
                state_ref,
                domain,
            )?;
            hide_menu(state_ref);
        }
        "set_value" => {
            if element != Some(8) {
                return Err("Element does not expose an editable value".into());
            }
            rename(value, state_ref, domain)?;
        }
        "click" | "double_click" | "right_click" | "drag" => {
            if !point_valid(x, y) || (action == "drag" && !point_valid(end_x, end_y)) {
                return Err(
                    "Pointer coordinates must be inside the observed workbench window".into(),
                );
            }
            match action {
                "click" | "double_click" => {
                    click(x, y, action == "double_click", state_ref, domain)?
                }
                "right_click" => {
                    if x >= CANVAS_X && y >= CANVAS_Y {
                        let selected = pattern_at(state_ref, x, y, domain)?;
                        select(state_ref, selected);
                    }
                    if selection(state_ref).is_some() {
                        state_ref["runtime"]["ui"]["menu_x"] = json!(x.min((WIDTH - 180) as f64));
                        state_ref["runtime"]["ui"]["menu_y"] = json!(y.min((HEIGHT - 34) as f64));
                    }
                }
                _ => {
                    if x < CANVAS_X || y < CANVAS_Y {
                        return Err("Drag starts inside the drafting area".into());
                    }
                    let outlines = outlines(state_ref, domain)?;
                    let transform = transform(state_ref, &outlines);
                    if let Some(index) =
                        hit_pattern(&outlines, transform, x - CANVAS_X, y - CANVAS_Y)
                    {
                        domain(
                            "pattern_ops",
                            &json!({"pattern":index.to_string(),"op":"move_2d","args":{"dx":(end_x-x)/transform.scale,"dy":(y-end_y)/transform.scale}}),
                            state_ref,
                        )?;
                        state_ref["runtime"]["ui"]["camera"] = json!({"fit_scale":transform.fit_scale,"center_x":transform.center_x,"center_y":transform.center_y});
                        select(state_ref, Some(index));
                    } else {
                        let pan_x = state_ref["runtime"]["ui"]["pan_x"].as_f64().unwrap_or(0.0);
                        let pan_y = state_ref["runtime"]["ui"]["pan_y"].as_f64().unwrap_or(0.0);
                        state_ref["runtime"]["ui"]["pan_x"] = json!(pan_x + end_x - x);
                        state_ref["runtime"]["ui"]["pan_y"] = json!(pan_y + end_y - y);
                    }
                }
            }
        }
        "text" => {
            if state_ref["runtime"]["ui"]["focus"] != json!("name") {
                return Err("Focus the pattern name field before entering text".into());
            }
            let next = format!(
                "{}{value}",
                state_ref["runtime"]["ui"]["name"]
                    .as_str()
                    .unwrap_or_default()
            );
            rename(&next, state_ref, domain)?;
        }
        "key" => match keycode {
            36 => {
                state_ref["runtime"]["ui"]["focus"] = Value::Null;
            }
            53 => {
                hide_menu(state_ref);
                select(state_ref, None);
            }
            51 | 117 => {
                if state_ref["runtime"]["ui"]["focus"] == json!("name") {
                    let mut next = state_ref["runtime"]["ui"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    next.pop();
                    if !next.is_empty() {
                        rename(&next, state_ref, domain)?;
                    }
                } else if selection(state_ref).is_some() {
                    choose_button(3, state_ref, domain)?;
                }
            }
            45 if modifiers.contains(&json!("command")) => {
                new_pattern(state_ref, domain)?;
            }
            49 => {
                choose_button(4, state_ref, domain)?;
            }
            _ => return Err("Key has no binding in the CLO Rust workbench".into()),
        },
        "scroll" => {
            if y.abs() > 100.0 {
                return Err("Scroll delta must be -100..100".into());
            }
            let zoom = state_ref["runtime"]["ui"]["zoom"].as_f64().unwrap_or(1.0);
            state_ref["runtime"]["ui"]["zoom"] = json!((zoom * (y / 16.0).exp()).clamp(0.2, 6.0));
        }
        _ => unreachable!(),
    }
    Ok(
        json!({"input_delivered":true,"verification_required":true,"state":state(&json!({}),state_ref)?}),
    )
}

#[derive(Clone, Copy)]
struct Transform {
    scale: f64,
    fit_scale: f64,
    center_x: f64,
    center_y: f64,
    tx: f64,
    ty: f64,
}

impl Transform {
    fn point(self, (x, y): (f64, f64)) -> (f64, f64) {
        (x * self.scale + self.tx, self.ty - y * self.scale)
    }
}

type Outlines = Vec<Vec<(f64, f64)>>;

fn outlines(state: &mut Value, domain: DomainCall) -> Result<Outlines, String> {
    let count = scene_patterns(state).len();
    (0..count)
        .map(|index| {
            let shape = domain(
                "__clo_pattern_outline",
                &json!({"pattern":index.to_string()}),
                state,
            )?;
            array(&shape["points"])
                .iter()
                .map(|point| {
                    let x = point[0].as_f64().ok_or("Invalid outline x")?;
                    let y = point[1].as_f64().ok_or("Invalid outline y")?;
                    Ok((x, y))
                })
                .collect()
        })
        .collect()
}

fn transform(state: &Value, outlines: &Outlines) -> Transform {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for point in outlines.iter().flatten() {
        bounds = Some(match bounds {
            Some((x0, y0, x1, y1)) => (
                x0.min(point.0),
                y0.min(point.1),
                x1.max(point.0),
                y1.max(point.1),
            ),
            None => (point.0, point.1, point.0, point.1),
        });
    }
    let (x0, y0, x1, y1) = bounds.unwrap_or((0.0, 0.0, 200.0, 200.0));
    let camera = &state["runtime"]["ui"]["camera"];
    let center_x = camera["center_x"].as_f64().unwrap_or((x0 + x1) / 2.0);
    let center_y = camera["center_y"].as_f64().unwrap_or((y0 + y1) / 2.0);
    let viewport_w = WIDTH as f64 - CANVAS_X;
    let viewport_h = HEIGHT as f64 - CANVAS_Y;
    let zoom = state["runtime"]["ui"]["zoom"].as_f64().unwrap_or(1.0);
    let fit_scale = camera["fit_scale"].as_f64().unwrap_or_else(|| {
        ((viewport_w - 100.0) / (x1 - x0).max(1.0))
            .min((viewport_h - 100.0) / (y1 - y0).max(1.0))
            .min(2.5)
    });
    let scale = fit_scale * zoom;
    let pan_x = state["runtime"]["ui"]["pan_x"].as_f64().unwrap_or(0.0);
    let pan_y = state["runtime"]["ui"]["pan_y"].as_f64().unwrap_or(0.0);
    Transform {
        scale,
        fit_scale,
        center_x,
        center_y,
        tx: viewport_w / 2.0 - center_x * scale + pan_x,
        ty: viewport_h / 2.0 + center_y * scale + pan_y,
    }
}

pub(super) fn pattern_at(
    state: &mut Value,
    x: f64,
    y: f64,
    domain: DomainCall,
) -> Result<Option<usize>, String> {
    if x < CANVAS_X || y < CANVAS_Y || !point_valid(x, y) {
        return Ok(None);
    }
    let outlines = outlines(state, domain)?;
    Ok(hit_pattern(
        &outlines,
        transform(state, &outlines),
        x - CANVAS_X,
        y - CANVAS_Y,
    ))
}

fn hit_pattern(outlines: &Outlines, transform: Transform, x: f64, y: f64) -> Option<usize> {
    outlines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, outline)| {
            let points: Vec<_> = outline
                .iter()
                .copied()
                .map(|point| transform.point(point))
                .collect();
            let mut inside = false;
            for ((x1, y1), (x2, y2)) in points
                .iter()
                .copied()
                .zip(points.iter().copied().cycle().skip(1))
            {
                if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
                    inside = !inside;
                }
            }
            inside.then_some(index)
        })
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn html(state: &Value, outlines: &Outlines) -> String {
    let mut buttons = String::new();
    for (index, (_, title, left, width)) in BUTTONS.iter().enumerate() {
        let disabled = if index == 1 && selection(state).is_none() {
            " disabled"
        } else {
            ""
        };
        buttons.push_str(&format!(
            "<button style=\"left:{left}px;width:{width}px\"{disabled}>{title}</button>"
        ));
    }
    let field = state["runtime"]["ui"]["name"]
        .as_str()
        .unwrap_or_else(|| current_name(state));
    let mut rows = String::new();
    let mut shapes = String::new();
    let transform = transform(state, outlines);
    let mut colors = BTreeMap::new();
    for (index, fabric) in array(&state["fabrics"]).iter().enumerate() {
        if let Some(color) = fabric["color"].as_str().filter(|color| {
            color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            colors.insert(index, color);
        }
    }
    for (index, pattern) in scene_patterns(state).iter().enumerate() {
        let name = escape(pattern["name"].as_str().unwrap_or("Unnamed pattern"));
        let selected = if selection(state) == Some(index) {
            " selected"
        } else {
            ""
        };
        let color = pattern["fabric_index"]
            .as_u64()
            .and_then(|index| colors.get(&(index as usize)))
            .copied()
            .unwrap_or("#b5d6ed");
        rows.push_str(&format!(
            "<li class=\"{selected}\"><i style=\"background:{color}\"></i>{name}</li>"
        ));
        let vertices: Vec<_> = outlines[index]
            .iter()
            .copied()
            .map(|point| transform.point(point))
            .collect();
        let path = vertices
            .iter()
            .map(|(x, y)| format!("{x:.3},{y:.3}"))
            .collect::<Vec<_>>()
            .join(" ");
        shapes.push_str(&format!(
            "<polygon class=\"{selected}\" fill=\"{color}\" points=\"{path}\"/>"
        ));
        if !vertices.is_empty() {
            let (sum_x, sum_y) = vertices
                .iter()
                .fold((0.0, 0.0), |(x, y), point| (x + point.0, y + point.1));
            let n = vertices.len() as f64;
            shapes.push_str(&format!(
                "<text x=\"{:.2}\" y=\"{:.2}\">{name}</text>",
                sum_x / n,
                sum_y / n
            ));
        }
    }
    let menu = match (state["runtime"]["ui"]["menu_x"].as_f64(),state["runtime"]["ui"]["menu_y"].as_f64()) {
        (Some(x),Some(y)) => format!("<button class=\"context\" style=\"left:{x}px;top:{y}px\">Delete selected pattern</button>"),
        _ => String::new(),
    };
    let fragments = [
        buttons,
        escape(field),
        if selection(state).is_none() {
            "disabled".into()
        } else {
            String::new()
        },
        rows,
        shapes,
        menu,
    ];
    let pieces = include_str!("workbench.html").split("<!--WORKBENCH-->");
    let mut page = String::new();
    for (index, piece) in pieces.enumerate() {
        page.push_str(piece);
        if let Some(fragment) = fragments.get(index) {
            page.push_str(fragment);
        }
    }
    page
}

fn browser() -> Result<PathBuf, String> {
    if let Some(executable) = std::env::var_os("CLO_RUST_BROWSER") {
        return Ok(PathBuf::from(executable));
    }
    let mac = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    if mac.is_file() {
        return Ok(mac.into());
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&search) {
        for binary in ["google-chrome", "chromium", "chromium-browser"] {
            let candidate = directory.join(binary);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err("Install Chromium or set CLO_RUST_BROWSER to render the CLO Rust workbench".into())
}

fn capture(browser: &Path, doc: &Path, output: &Path, profile: &Path) -> Result<(), String> {
    let mut child = Command::new(browser)
        .args([
            "--headless",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--disable-background-networking",
        ])
        .arg(format!("--window-size={WIDTH},{HEIGHT}"))
        .arg("--force-device-scale-factor=1")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--screenshot={}", output.display()))
        .arg(doc)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not launch the workbench renderer: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() && output.is_file() => return Ok(()),
            Ok(Some(status)) => return Err(format!("Workbench screenshot failed: {status}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => error.to_string(),
                    _ => "Workbench screenshot timed out".into(),
                });
            }
        }
    }
}

pub(super) fn screenshot(state: &mut Value, domain: DomainCall) -> ToolResult {
    validate_window(state)?;
    let scene_outlines = outlines(state, domain)?;
    let executable = browser()?;
    let root = std::env::var_os("CLO_RUST_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let directory = root.join(format!("clo-workbench-{}-{}", std::process::id(), clock()));
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    let doc = directory.join("workbench.html");
    let output = directory.join("workbench.png");
    let profile = directory.join("browser-profile");
    let result = fs::write(&doc, html(state, &scene_outlines))
        .map_err(|error| error.to_string())
        .and_then(|_| capture(&executable, &doc, &output, &profile));
    let _ = fs::remove_dir_all(&profile);
    let _ = fs::remove_file(&doc);
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&directory);
        return Err(error);
    }
    Ok(json!({"images":[{"path":output,"mime_type":"image/png"}]}))
}
