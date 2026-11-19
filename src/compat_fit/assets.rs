use super::{array, array_mut, avt, boolean, number, pattern_index, string, texture::BaseTexture};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const AVATARS: &[(&str, &str)] = &[
    ("boy", "oliver"),
    ("female", "mia"),
    ("female mannequin", "female mannequin"),
    ("girl", "melody"),
    ("luka", "luka"),
    ("male", "luka"),
    ("male mannequin", "male mannequin"),
    ("man", "luka"),
    ("melody", "melody"),
    ("mia", "mia"),
    ("oliver", "oliver"),
    ("woman", "mia"),
];

const POINTS: &[&str] = &[
    "Front",
    "Back",
    "Front Torso",
    "Back Torso",
    "Left",
    "Right",
    "Left Arm",
    "Right Arm",
    "Left Leg",
    "Right Leg",
    "Front Waist",
    "Back Waist",
    "Front Hip",
    "Back Hip",
    "Body_Front_Center_3",
    "Body_Back_Center_3",
    "Leg_Skirt_Front",
    "Leg_Skirt_Back",
    "Arm_Outside_2_L",
    "Arm_Outside_2_R",
];

struct Preset {
    name: &'static str,
    aliases: &'static [&'static str],
    stiffness: f64,
}

const MATERIALS: &[Preset] = &[
    Preset {
        name: "Native Woven Crepe",
        aliases: &["crepe", "crepe de chine", "cdc"],
        stiffness: 0.84,
    },
    Preset {
        name: "Native Woven Satin",
        aliases: &["satin", "silk", "charmeuse", "duchess"],
        stiffness: 0.82,
    },
    Preset {
        name: "Native Woven Cotton",
        aliases: &["cotton", "poplin", "muslin", "broadcloth"],
        stiffness: 0.92,
    },
    Preset {
        name: "Native Woven Denim",
        aliases: &["denim", "jean", "canvas", "twill"],
        stiffness: 0.97,
    },
    Preset {
        name: "Native Knit Jersey",
        aliases: &["jersey", "t-shirt", "tee", "knit", "stretch"],
        stiffness: 0.66,
    },
    Preset {
        name: "Native Chiffon",
        aliases: &["chiffon", "georgette", "organza"],
        stiffness: 0.72,
    },
    Preset {
        name: "Native Knit Mesh",
        aliases: &["tulle", "mesh", "net", "lace"],
        stiffness: 0.74,
    },
    Preset {
        name: "Native Woven Linen",
        aliases: &["linen", "hemp"],
        stiffness: 0.94,
    },
    Preset {
        name: "Native Woven Wool",
        aliases: &["wool", "flannel", "felt", "tweed"],
        stiffness: 0.95,
    },
    Preset {
        name: "Native Velvet",
        aliases: &["velvet", "velveteen", "velour"],
        stiffness: 0.87,
    },
    Preset {
        name: "Native Leather",
        aliases: &["leather", "pleather", "suede"],
        stiffness: 0.99,
    },
];

pub fn default_body() -> Value {
    json!({
        "height":1755.0,"bust":814.0,"underbust":662.0,"waist":608.0,"high_hip":805.0,
        "hip":953.0,"neck":355.0,"armhole":384.0,"bicep":257.0,"elbow":218.0,
        "wrist":141.0,"thigh":553.0,"knee_circ":343.0,"across_shoulder":377.0,
        "across_front":324.0,"across_back":319.0,"apex_width":162.0,"h_hps":1500.0,
        "h_shoulder":1454.0,"h_apex":1290.0,"h_underbust":1235.0,"h_waist":1141.0,
        "h_high_hip":1011.0,"h_hip":879.0,"h_crotch":841.0,"h_knee":483.0,
        "h_calf":333.0,"h_ankle":86.0,"cf_neck_waist":338.0,"cb_neck_waist":380.0,
        "arm_length":589.0,"shoulder_drop":46.0,"source":"default_female"
    })
}

fn scaled_body(height: f64, girth: f64, source: &str) -> Value {
    let mut body = default_body();
    for (name, value) in body.as_object_mut().unwrap() {
        if let Some(n) = value.as_f64() {
            let scale = if name.starts_with("h_")
                || name == "height"
                || name.ends_with("length")
                || name.ends_with("_waist")
            {
                height / 1755.0
            } else {
                girth
            };
            *value = json!(n * scale);
        }
    }
    body["source"] = json!(source);
    body
}

fn avatar_library() -> Vec<String> {
    let mut result = vec![
        "female mannequin".into(),
        "luka".into(),
        "male mannequin".into(),
        "melody".into(),
        "mia".into(),
        "oliver".into(),
    ];
    result.extend(
        configured_avatar_paths()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    result.sort();
    result
}

fn configured_avatar_paths() -> Vec<PathBuf> {
    const MAX_ASSETS: usize = 2048;
    fn visit(directory: &Path, depth: usize, remaining: &mut usize, paths: &mut Vec<PathBuf>) {
        if depth == 0 || *remaining == 0 {
            return;
        }
        for entry in fs::read_dir(directory)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if *remaining == 0 {
                break;
            }
            *remaining -= 1;
            let path = entry.path();
            if kind.is_dir() {
                visit(&path, depth - 1, remaining, paths);
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| {
                        ["obj", "glb", "avt"]
                            .iter()
                            .any(|s| s.eq_ignore_ascii_case(ext))
                    })
            {
                paths.push(path);
            }
        }
    }
    let mut result = Vec::new();
    if let Some(dir) = std::env::var_os("CLO_RUST_AVATAR_DIR") {
        let mut remaining = MAX_ASSETS;
        visit(Path::new(&dir), 5, &mut remaining, &mut result);
    }
    result.sort();
    result
}

fn native_alias_stem(alias: &str) -> &str {
    match alias {
        "mia" => "FV2.1_Mia",
        "luka" => "MV2.1_Luka",
        "female mannequin" => "FV2.1_Mannequin",
        "male mannequin" => "MV2.1_Mannequin",
        "melody" => "KV1.1G_Melody_(7-20)",
        "oliver" => "KV1.1B_Oliver_(7-20)",
        _ => alias,
    }
}

fn avatar_lookup(reference: &str, alias: Option<&str>) -> Option<PathBuf> {
    let direct = PathBuf::from(reference);
    if direct.is_file() {
        return Some(direct);
    }
    configured_avatar_paths().into_iter().find(|path| {
        path.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case(reference))
            || path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| {
                    stem.eq_ignore_ascii_case(reference)
                        || alias.is_some_and(|alias| {
                            stem.eq_ignore_ascii_case(alias)
                                || stem.eq_ignore_ascii_case(native_alias_stem(alias))
                        })
                })
    })
}

fn files(root: &str, types: &[&str]) -> Vec<PathBuf> {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| types.iter().any(|kind| kind.eq_ignore_ascii_case(ext)))
        })
        .collect()
}

pub fn avatar_from_path(path: &Path) -> Result<Value, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("cannot open avatar {}: {e}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("avatar");
    let mut rest_details = None;
    let (points, faces) = match extension.as_str() {
        "avt" => {
            let rest=avt::read(&path)?;
            rest_details=Some((rest.part_count,rest.arrangements));
            (rest.points,rest.triangles)
        }
        "obj" => avatar_obj(&path)?,
        "glb" => {
            let model = gltf::Gltf::open(&path).map_err(|e| format!("invalid avatar GLB: {e}"))?;
            let names: Vec<_> = model.nodes().filter_map(|node| {
                let mesh = node.mesh()?;
                node.name().or_else(|| mesh.name())
            }).collect();
            let mesh = crate::avatar::load_selected(path.to_str().ok_or("avatar path is not UTF-8")?, &names, "m")?;
            (mesh.points, mesh.faces)
        }
        _ => return Err(format!("avatar format .{extension} is unavailable on the Rust backend; use a supported static AVT, an OBJ in millimetres, or a named-mesh GLB in metres")),
    };
    avatar_from_mesh(name, Some(&path), points, faces, rest_details)
}

pub(crate) fn project_avatar(path: &Path) -> Result<Option<Value>, String> {
    let Some((name, rest)) = avt::read_project(path)? else {
        return Ok(None);
    };
    let mut avatar = avatar_from_mesh(
        &name,
        None,
        rest.points,
        rest.triangles,
        Some((rest.part_count, rest.arrangements)),
    )?;
    avatar["source_project"] = json!(path);
    Ok(Some(avatar))
}

fn avatar_from_mesh(
    name: &str,
    path: Option<&Path>,
    mut points: Vec<[f64; 3]>,
    faces: Vec<[usize; 3]>,
    rest_details: Option<(usize, Vec<String>)>,
) -> Result<Value, String> {
    let ymin = points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min);
    let ymax = points
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let xmin = points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
    let xmax = points
        .iter()
        .map(|p| p[0])
        .fold(f64::NEG_INFINITY, f64::max);
    let zmin = points.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
    let zmax = points
        .iter()
        .map(|p| p[2])
        .fold(f64::NEG_INFINITY, f64::max);
    let height = ymax - ymin;
    if !(1.0..=5000.0).contains(&height) {
        return Err("avatar height is out of the supported millimetre range".into());
    }
    if rest_details.is_none() {
        for point in &mut points {
            point[0] -= (xmin + xmax) / 2.0;
            point[1] -= ymin;
            point[2] -= (zmin + zmax) / 2.0;
        }
    }
    let mut body = scaled_body(height, height / 1755.0, "rust_mesh_estimate");
    for (measurement, height_key) in [
        ("bust", "h_apex"),
        ("underbust", "h_underbust"),
        ("waist", "h_waist"),
        ("high_hip", "h_high_hip"),
        ("hip", "h_hip"),
    ] {
        let floor = if rest_details.is_some() { ymin } else { 0.0 };
        let level = body[height_key].as_f64().unwrap() + floor;
        if let Some(circumference) = mesh_section(&points, &faces, level) {
            body[measurement] = json!(circumference);
        }
    }
    let mut avatar = json!({
        "id":name,"name":name,"path":path,"source":"rust_mesh_estimate","body":body,
        "arrangement_points":POINTS,"mesh":{"positions":points,"triangles":faces}
    });
    if let Some((parts, names)) = rest_details {
        avatar["source"] = json!("clo_avt_rest");
        avatar["measurement_source"] = json!("rust_mesh_estimate");
        avatar["rest_geometry"] = json!({
            "parts":parts,"vertices":points.len(),"triangles":faces.len(),
            "bounds_mm":[[xmin,ymin,zmin],[xmax,ymax,zmax]]
        });
        let mut warnings=vec![
            "Loaded saved AVT rest geometry only. Native skinning, posing, and avatar textures are unavailable.",
            "Body measurements are estimated. The CPU cloth solver uses an analytic collision proxy.",
        ];
        if names.is_empty() {
            warnings.push("Native arrangement names are unavailable. Using built-in names.");
        } else {
            avatar["arrangement_points"] = json!(names);
        }
        avatar["warnings"] = json!(warnings);
    }
    Ok(avatar)
}

fn mesh_section(points: &[[f64; 3]], faces: &[[usize; 3]], y: f64) -> Option<f64> {
    // Estimate a girth from the convex hull of the horizontal mesh intersection.
    let mut section = Vec::<[f64; 2]>::new();
    for face in faces {
        for i in 0..3 {
            let a = points[face[i]];
            let b = points[face[(i + 1) % 3]];
            if (a[1] <= y && b[1] > y) || (b[1] <= y && a[1] > y) {
                let t = (y - a[1]) / (b[1] - a[1]);
                section.push([a[0] + t * (b[0] - a[0]), a[2] + t * (b[2] - a[2])]);
            }
        }
    }
    if section.len() < 3 {
        return None;
    }
    section.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let hull = |values: &mut dyn Iterator<Item = [f64; 2]>| {
        let mut result = Vec::new();
        for point in values {
            while result.len() >= 2
                && cross(result[result.len() - 2], result[result.len() - 1], point) <= 0.0
            {
                result.pop();
            }
            result.push(point);
        }
        result.pop();
        result
    };
    let mut circumference = hull(&mut section.iter().copied());
    circumference.extend(hull(&mut section.iter().rev().copied()));
    let length = circumference
        .iter()
        .zip(circumference.iter().cycle().skip(1))
        .map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]))
        .sum::<f64>();
    (length > 1.0).then_some(length)
}

fn avatar_obj(path: &Path) -> Result<(Vec<[f64; 3]>, Vec<[usize; 3]>), String> {
    let mut points = Vec::new();
    let mut faces = Vec::new();
    for line in fs::read_to_string(path).map_err(|e| e.to_string())?.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("v") => {
                let xyz: Vec<_> = fields
                    .take(3)
                    .map(str::parse::<f64>)
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?;
                if xyz.len() != 3 || !xyz.iter().all(|v| v.is_finite()) {
                    return Err("invalid OBJ vertex".into());
                }
                points.push([xyz[0], xyz[1], xyz[2]]);
            }
            Some("f") => {
                let corners: Vec<_> = fields
                    .take_while(|f| !f.starts_with('#'))
                    .map(|s| {
                        let i = s
                            .split('/')
                            .next()
                            .unwrap_or("")
                            .parse::<isize>()
                            .map_err(|e| e.to_string())?;
                        let at = if i > 0 {
                            i - 1
                        } else if i < 0 {
                            points.len() as isize + i
                        } else {
                            -1
                        };
                        if at < 0 || at as usize >= points.len() {
                            return Err("invalid OBJ face index".into());
                        }
                        Ok(at as usize)
                    })
                    .collect::<Result<_, String>>()?;
                if corners.len() < 3 {
                    return Err("invalid OBJ face".into());
                }
                for i in 1..corners.len() - 1 {
                    faces.push([corners[0], corners[i], corners[i + 1]]);
                }
            }
            _ => (),
        }
    }
    if faces.is_empty() {
        return Err("avatar OBJ contains no triangles".into());
    }
    Ok((points, faces))
}

pub fn avatar_list(state: &Value) -> Value {
    let in_scene: Vec<_> = std::iter::once(&state["avatar"])
        .chain(array(&state["additional_avatars"]))
        .filter_map(|a| a["name"].as_str())
        .collect();
    json!({"in_scene":in_scene,"library":avatar_library(),"aliases":AVATARS.iter().map(|(alias,_)|alias).collect::<Vec<_>>()})
}

pub fn avatar_load(args: &Value, state: &mut Value) -> Result<Value, String> {
    let reference = string(args, "name_or_path", "mia");
    let alias = reference.trim().to_ascii_lowercase();
    let canonical = AVATARS
        .iter()
        .find(|(from, _)| *from == alias)
        .map(|(_, name)| *name);
    let avatar = if let Some(path) = avatar_lookup(reference, canonical) {
        avatar_from_path(&path)?
    } else if let Some(name) = canonical {
        let (height, girth) = match name {
            "luka" | "male mannequin" => (1820.0, 1.17),
            "melody" => (1370.0, 0.74),
            "oliver" => (1400.0, 0.77),
            _ => (1755.0, 1.0),
        };
        json!({"id":name,"name":name,"source":"rust_procedural_proxy","body":scaled_body(height,girth,"rust_procedural_proxy"),"arrangement_points":POINTS})
    } else {
        return Err(format!("no avatar matching {reference:?}"));
    };
    let mut result =
        json!({"ok":true,"avatar":avatar["name"],"path":avatar["path"],"source":avatar["source"]});
    if avatar["source"] == "clo_avt_rest" {
        result["rest_geometry"] = avatar["rest_geometry"].clone();
        result["warnings"] = avatar["warnings"].clone();
    }
    if boolean(args, "replace", true) || state["avatar"].is_null() {
        state["avatar"] = avatar;
        state["additional_avatars"] = json!([]);
    } else {
        array_mut(state, "additional_avatars").push(avatar);
    }
    state["simulation"] = Value::Null;
    Ok(result)
}

pub(super) fn measurement_source(avatar: &Value) -> &str {
    string(
        avatar,
        "measurement_source",
        string(avatar, "source", "rust_scene"),
    )
}

pub fn avatar_measurements(args: &Value, state: &mut Value) -> Result<Value, String> {
    if boolean(args, "refresh", false) {
        if let Some(path) = state["avatar"]["path"].as_str() {
            state["avatar"] = avatar_from_path(Path::new(path))?;
            state["simulation"] = Value::Null;
        }
    }
    let avatar = &state["avatar"];
    if avatar.is_null() {
        return Err("no avatar in the scene; call avatar_load first".into());
    }
    Ok(json!({"avatar":avatar["name"],"source":measurement_source(avatar),"body":avatar["body"]}))
}

pub fn arrangement_points(state: &Value) -> Result<Value, String> {
    let points = array(&state["avatar"]["arrangement_points"]);
    Ok(json!({"count":points.len(),"names":points}))
}

fn material(reference: &str) -> Result<(Value, String, Option<PathBuf>), String> {
    let reference = reference.trim();
    let path = Path::new(reference);
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zfab"))
    {
        return Err(
            "native Rust does not decode CLO .zfab files; supply a Rust material JSON preset"
                .into(),
        );
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        let path = if path.is_file() {
            path.to_path_buf()
        } else {
            std::env::var("CLO_RUST_FABRIC_DIR")
                .ok()
                .map(|dir| PathBuf::from(dir).join(path))
                .filter(|path| path.is_file())
                .ok_or_else(|| format!("no material file matching {reference:?}"))?
        };
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let data: Value = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let name = data["name"].as_str().ok_or("material JSON needs name")?;
        let stiffness = data["stiffness"]
            .as_f64()
            .filter(|v| (0.0..=1.0).contains(v))
            .ok_or("material JSON needs stiffness in 0..=1")?;
        let rgb = parse_color(&data["color"])?;
        return Ok((
            json!({"name":name,"stiffness":stiffness,"color":rgb,"texture_maps":{},"info":data}),
            "path".into(),
            Some(path),
        ));
    }
    let lower = reference.to_ascii_lowercase();
    let mat = MATERIALS
        .iter()
        .flat_map(|preset| preset.aliases.iter().map(move |alias| (preset, *alias)))
        .filter(|(preset, alias)| {
            lower.contains(alias) || lower == preset.name.to_ascii_lowercase()
        })
        .max_by_key(|(_, alias)| alias.len());
    let (preset, alias) = mat.unwrap_or((&MATERIALS[0], "default (crepe)"));
    Ok((
        json!({"name":preset.name,"stiffness":preset.stiffness,"color":[0.85,0.85,0.85],"texture_maps":{},"info":{"source":"rust_builtin","stiffness":preset.stiffness}}),
        alias.into(),
        None,
    ))
}

pub fn selected_material(reference: &str) -> Result<Value, String> {
    let (material, matched, file) = material(reference)?;
    let library_stem = match matched.as_str() {
        "default (crepe)" | "crepe" | "crepe de chine" | "cdc" => "V2_Woven_Crepe_CDC_1",
        "cotton" | "poplin" | "muslin" | "broadcloth" => "V2_Woven_Cotton_Poplin_1",
        "satin" | "silk" | "charmeuse" | "duchess" => "V2_Woven_Satin_1",
        "denim" | "jean" => "V2_Woven_Denim_Jean_1",
        _ => material["name"].as_str().unwrap_or("Native Fabric"),
    };
    Ok(json!({"library_stem":library_stem,"matched":matched,"path":file}))
}

pub fn fabric_library(args: &Value) -> Value {
    let query = string(args, "filter", "").to_ascii_lowercase();
    let root = std::env::var("CLO_RUST_FABRIC_DIR").ok();
    let custom = root
        .as_deref()
        .map(|p| files(p, &["json"]))
        .unwrap_or_default();
    let mut fabric_names: Vec<_> = MATERIALS
        .iter()
        .filter(|p| {
            p.name.to_ascii_lowercase().contains(&query)
                || p.aliases.iter().any(|s| s.contains(&query))
        })
        .map(|preset| preset.name.to_owned())
        .collect();
    fabric_names.extend(
        custom
            .iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
            .filter(|s| s.to_ascii_lowercase().contains(&query))
            .map(str::to_owned),
    );
    fabric_names.sort();
    let mut keywords: Vec<_> = MATERIALS.iter().flat_map(|p| p.aliases).copied().collect();
    keywords.sort_unstable();
    json!({"folder":root,"fabrics":fabric_names,"keywords":keywords})
}

pub fn fabric_list(state: &Value) -> Value {
    let fabrics: Vec<_> = array(&state["fabrics"]).iter().map(|value| json!({
        "index":value["index"],"name":value["name"],"info":{
            "material":value["info"],"color":value["color"],"texture_maps":value["texture_maps"]
        }
    })).collect();
    json!({"fabrics":fabrics,"current":state["current_fabric"].as_i64().unwrap_or(if fabrics.is_empty(){-1}else{0})})
}

pub fn parse_color(input: &Value) -> Result<[f64; 3], String> {
    if let Some(list) = input.as_array() {
        if list.len() < 3 {
            return Err("RGB requires three channels".into());
        }
        let nums = [list[0].as_f64(), list[1].as_f64(), list[2].as_f64()];
        if let [Some(r), Some(g), Some(b)] = nums {
            let scale = if r.max(g).max(b) > 1.0 { 255.0 } else { 1.0 };
            if [r, g, b].iter().all(|v| (0.0..=scale).contains(v)) {
                return Ok([r / scale, g / scale, b / scale]);
            }
        }
        return Err("RGB channels are out of range".into());
    }
    let value = input.as_str().unwrap_or("").trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        if !hex.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err(format!("invalid RGB color {value:?}"));
        }
        if hex.len() == 3 || hex.len() == 6 {
            let stride = hex.len() / 3;
            return Ok(std::array::from_fn(|i| {
                let code = &hex[stride * i..stride * (i + 1)];
                f64::from(u8::from_str_radix(code, 16).expect("ASCII hex"))
                    / if stride == 1 { 15.0 } else { 255.0 }
            }));
        }
    }
    let colors = [
        ("black", [0.05, 0.05, 0.05]),
        ("white", [0.96, 0.96, 0.96]),
        ("ivory", [0.95, 0.92, 0.84]),
        ("cream", [0.96, 0.93, 0.86]),
        ("red", [0.78, 0.08, 0.12]),
        ("crimson", [0.7, 0.05, 0.15]),
        ("burgundy", [0.45, 0.06, 0.14]),
        ("pink", [0.95, 0.6, 0.72]),
        ("hot pink", [0.92, 0.2, 0.55]),
        ("orange", [0.95, 0.5, 0.12]),
        ("yellow", [0.98, 0.85, 0.2]),
        ("green", [0.15, 0.5, 0.25]),
        ("olive", [0.45, 0.48, 0.2]),
        ("teal", [0.05, 0.45, 0.5]),
        ("blue", [0.15, 0.3, 0.75]),
        ("navy", [0.1, 0.16, 0.42]),
        ("royal blue", [0.15, 0.25, 0.8]),
        ("sky blue", [0.55, 0.75, 0.95]),
        ("purple", [0.45, 0.2, 0.6]),
        ("lavender", [0.75, 0.68, 0.9]),
        ("brown", [0.4, 0.25, 0.15]),
        ("beige", [0.85, 0.78, 0.65]),
        ("grey", [0.5, 0.5, 0.5]),
        ("gray", [0.5, 0.5, 0.5]),
        ("charcoal", [0.2, 0.2, 0.22]),
        ("silver", [0.75, 0.75, 0.78]),
    ];
    Ok(colors
        .iter()
        .filter(|(name, _)| value.contains(name))
        .max_by_key(|(name, _)| name.len())
        .map_or([0.85; 3], |(_, rgb)| *rgb))
}

fn texture(path: &str, kind: &str) -> Result<PathBuf, String> {
    if ![
        "base",
        "normal",
        "roughness",
        "opacity",
        "displacement",
        "metalness",
    ]
    .contains(&kind)
    {
        return Err(format!("unknown map type {kind:?}"));
    }
    let path = Path::new(path)
        .canonicalize()
        .map_err(|e| format!("cannot open fabric texture {path}: {e}"))?;
    let mut file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let mut header = [0; 12];
    use std::io::Read;
    let count = file.read(&mut header).map_err(|e| e.to_string())?;
    let png = count >= 8 && &header[..8] == b"\x89PNG\r\n\x1a\n";
    let jpeg = count >= 3 && header[..3] == [0xff, 0xd8, 0xff];
    let webp = count >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP";
    if kind == "base" {
        if !png && !jpeg {
            return Err("the native base-color renderer requires a static PNG or JPEG texture".into());
        }
        BaseTexture::load(&path)
            .map_err(|error| format!("invalid base texture {}: {error}", path.display()))?;
    }
    if !png && !jpeg && !webp {
        return Err("texture must be PNG, JPEG, or WebP".into());
    }
    Ok(path)
}

pub fn fabric_apply(args: &Value, state: &mut Value) -> Result<Value, String> {
    let (mut material, matched, file) = material(string(args, "fabric", ""))?;
    let patterns = array(&state["patterns"]);
    let pattern_refs = array(&args["patterns"]);
    let selected: Vec<_> = if pattern_refs.is_empty() {
        (0..patterns.len()).collect()
    } else {
        pattern_refs
            .iter()
            .map(|r| pattern_index(patterns, r))
            .collect::<Result<_, _>>()?
    };
    let color = args
        .get("color")
        .filter(|v| !v.is_null() && v.as_str() != Some(""));
    let rgb = color.map(parse_color).transpose()?;
    let base_texture = args["texture_image"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|p| texture(p, "base"))
        .transpose()?;
    if let Some(rgb) = rgb {
        material["color"] = json!(rgb);
    }
    if let Some(path) = &base_texture {
        material["texture_maps"]["base"] = json!(path);
    }
    let fabrics = array_mut(state, "fabrics");
    let index = fabrics
        .iter()
        .filter_map(|f| f["index"].as_u64())
        .max()
        .map_or(0, |i| i + 1);
    material["index"] = json!(index);
    let selected_material = json!({"index":index,"name":material["name"]});
    fabrics.push(material);
    let patterns = array_mut(state, "patterns");
    for i in &selected {
        patterns[*i]["fabric_index"] = json!(index);
    }
    state["current_fabric"] = json!(index);
    let mut result = json!({"fabric":selected_material,"matched":matched,"file":file,
        "assigned":{"assigned":vec![true;selected.len()],"patterns":selected}});
    if let Some(rgb) = rgb {
        result["rgb"] = json!(rgb);
        result["color"] = json!({"ok":[true,true,true]});
    }
    if base_texture.is_some() {
        result["texture"] = json!({"ok":true});
    }
    Ok(result)
}

fn find_fabric_mut(state: &mut Value, index: u64) -> Result<&mut Value, String> {
    state
        .get_mut("fabrics")
        .and_then(Value::as_array_mut)
        .and_then(|fabrics| fabrics.iter_mut().find(|f| f["index"] == index))
        .ok_or_else(|| format!("no fabric at index {index}"))
}

pub fn fabric_set_color(args: &Value, state: &mut Value) -> Result<Value, String> {
    let index = args["fabric_index"]
        .as_u64()
        .ok_or("fabric_index must be a nonnegative integer")?;
    let rgb = parse_color(&args["color"])?;
    let fabric = find_fabric_mut(state, index)?;
    fabric["color"] = json!(rgb);
    let [r, g, b] = rgb;
    fabric["rgba"] = json!([r, g, b, 1.0]);
    if let Some(maps) = fabric["texture_maps"].as_object_mut() {
        maps.remove("base");
    }
    crate::compat_scene::mark_native_fabric_color(state, index);
    Ok(json!({"rgb":rgb,"result":{"ok":[true,true,true]}}))
}

pub fn fabric_set_texture(args: &Value, state: &mut Value) -> Result<Value, String> {
    let index = args["fabric_index"]
        .as_u64()
        .ok_or("fabric_index must be a nonnegative integer")?;
    let kind = string(args, "map", "base");
    let path = texture(string(args, "image_path", ""), kind)?;
    find_fabric_mut(state, index)?["texture_maps"][kind] = json!(path);
    crate::compat_scene::mark_native_fabric_texture(state, index, kind);
    state["current_fabric"] = json!(index);
    Ok(json!({"ok":true}))
}

pub fn material_stiffness(state: &Value, id: &Value) -> f64 {
    array(&state["fabrics"])
        .iter()
        .find(|m| !id.is_null() && m["index"] == *id)
        .map_or(0.9, |m| number(m, "stiffness", 0.9))
}
