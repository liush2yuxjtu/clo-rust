use super::{array, array_mut, avt, boolean, pattern_index, string, texture::BaseTexture};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    ops::Range,
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

// Keyword-to-file mappings exposed by the CLO MCP fabric tools.
#[rustfmt::skip]
const LIBRARY_KEYWORDS: &[(&[&str], &str)] = &[
    (&["satin", "silk", "charmeuse", "duchess"], "V2_Woven_Satin_1"),
    (&["chiffon"], "V2_Woven_Chiffon_1"),
    (&["georgette"], "V2_Woven_Georgette_1"),
    (&["crepe", "cdc", "crepe de chine"], "V2_Woven_Crepe_CDC_1"),
    (&["organza"], "V2_Woven_Organza_1"),
    (&["taffeta"], "V2_Woven_Taffeta_1"),
    (&["tulle", "mesh", "net"], "V2_Cut_Sew_Knit_Mesh_Tulle_1"),
    (&["lace"], "V2_Cut_Sew_Knit_Lace_1"),
    (&["velvet", "velveteen"], "V2_Woven_Velvet_Velveteen_1"),
    (&["velour"], "V2_Cut_Sew_Knit_Velour_1"),
    (&["jersey", "t-shirt", "tee", "knit", "stretch"], "V2_Cut_Sew_Knit_Jersey_1"),
    (&["ity", "matte jersey", "slinky"], "V2_Cut_Sew_Knit_ITY_MatteJersey_1"),
    (&["ponte", "double knit"], "V2_Cut_Sew_Knit_Ponte_1"),
    (&["scuba", "neoprene"], "V2_Cut_Sew_Knit_Neoprene_Scuba_1"),
    (&["rib", "ribbed"], "V2_Cut_Sew_Knit_Rib_1"),
    (&["crepe knit"], "V2_Cut_Sew_Knit_CrepeKnit_1"),
    (&["denim", "jean"], "V2_Woven_Denim_1"),
    (&["chambray", "oxford"], "V2_Woven_Chambray_Oxford_1"),
    (&["poplin", "cotton", "shirting"], "V2_Woven_Poplin_1"),
    (&["linen", "plain weave"], "V2_Woven_Plain_1"),
    (&["twill", "gabardine"], "V2_Woven_Twill_1"),
    (&["canvas", "duck"], "V2_Woven_Canvas_1"),
    (&["wool", "suiting", "tweed"], "V2_Woven_Tweed_1"),
    (&["melton", "boiled wool", "coating"], "V2_Woven_Melton_Boiled_1"),
    (&["flannel"], "V2_Woven_Flannel_1"),
    (&["voile", "lawn"], "V2_Woven_Voile_1"),
    (&["gauze", "double gauze", "muslin"], "V2_Woven_Gauze_DoubleGauze_1"),
    (&["challis", "rayon", "viscose"], "V2_Woven_Challis_1"),
    (&["seersucker"], "V2_Woven_Seersucker_1"),
    (&["corduroy", "cord"], "V2_Woven_Corduroy_1"),
    (&["jacquard", "brocade", "damask"], "V2_Woven_Jacquard_Brocade_1"),
    (&["boucle"], "V2_Woven_Boucle_1"),
    (&["plaid", "tartan", "check"], "V2_Woven_Plaid_1"),
    (&["eyelet", "broderie"], "V2_Woven_Eyelet_1"),
    (&["dobby"], "V2_Woven_Dobby_1"),
    (&["leather"], "V2_Leather_Full_Grain"),
    (&["vegan leather", "pleather", "pu"], "V2_Animal_Alternatives_VeganLeather_1"),
    (&["suede"], "V2_Animal_Alternatives_VeganSuede_1"),
    (&["fur"], "V2_Animal_Alternatives_VeganFur_1"),
    (&["fleece"], "V2_Cut_Sew_Knit_Fleece_1"),
    (&["french terry", "sweatshirt"], "V2_Cut_Sew_Knit_FrenchTerry_1"),
    (&["pique", "polo"], "V2_Cut_Sew_Knit_Pique_1"),
    (&["interlock"], "V2_Cut_Sew_Knit_DoubleKnit_Interlock_1"),
    (&["ripstop", "nylon"], "V2_Woven_Ripstop_1"),
    (&["crochet"], "V2_Cut_Sew_Knit_Crochet_1"),
    (&["pointelle"], "V2_Cut_Sew_Knit_Pointelle_1"),
    (&["waffle"], "V2_Cut_Sew_Knit_Waffle_1"),
    (&["sherpa"], "V2_Cut_Sew_Knit_Sherpa_1"),
    (&["ottoman"], "V2_Woven_Ottoman_1"),
    (&["dewspo", "sportswear"], "V2_Woven_Dewspo_1"),
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
    configured_avatar_paths()
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("avt"))
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

fn asset_subfolder(setting: &str, subfolder: &str) -> Option<PathBuf> {
    match std::env::var_os(setting) {
        Some(dir) => (!dir.is_empty()).then(|| PathBuf::from(dir)),
        None => std::env::var_os("CLO_MCP_ASSETS").map(|dir| PathBuf::from(dir).join(subfolder)),
    }
}

fn avatar_root() -> Option<PathBuf> {
    asset_subfolder("CLO_RUST_AVATAR_DIR", "Avatar")
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
    if let Some(dir) = avatar_root() {
        let mut remaining = MAX_ASSETS;
        visit(Path::new(&dir), 5, &mut remaining, &mut result);
    }
    result.sort();
    result
}

fn native_alias_path(alias: &str) -> &str {
    match alias {
        "mia" => "Female/FV2.1_Mia.avt",
        "luka" => "Male/MV2.1_Luka.avt",
        "female mannequin" => "Mannequin/FV2.1_Mannequin.avt",
        "male mannequin" => "Mannequin/MV2.1_Mannequin.avt",
        "melody" => "Kid/KV1.1G_Melody_(7-20).avt",
        "oliver" => "Kid/KV1.1B_Oliver_(7-20).avt",
        _ => alias,
    }
}

fn avatar_lookup(reference: &str, alias: Option<&str>) -> Option<PathBuf> {
    let direct = PathBuf::from(reference);
    if direct.is_file() {
        return Some(direct);
    }
    if let Some(path) = alias
        .and_then(|name| avatar_root().map(|root| root.join(native_alias_path(name))))
        .filter(|path| path.is_file())
    {
        return Some(path);
    }
    let reference = reference.trim();
    let paths = configured_avatar_paths();
    let native = alias.and_then(|name| {
        let expected = Path::new(native_alias_path(name)).file_stem()?;
        paths.iter().find(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("avt"))
                && path
                    .file_stem()
                    .is_some_and(|stem| stem.eq_ignore_ascii_case(expected))
        })
    });
    let exact = native.or_else(|| {
        paths.iter().find(|path| {
            path.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(reference))
                || path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|stem| {
                        stem.eq_ignore_ascii_case(reference)
                            || alias.is_some_and(|alias| stem.eq_ignore_ascii_case(alias))
                    })
        })
    });
    let lower = reference.to_ascii_lowercase();
    exact
        .or_else(|| {
            paths.iter().find(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("avt"))
                    && path.file_name().is_some_and(|name| {
                        name.to_string_lossy().to_ascii_lowercase().contains(&lower)
                    })
            })
        })
        .cloned()
}

fn fabric_paths(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, remaining: &mut usize, depth: usize, paths: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            if *remaining == 0 {
                break;
            }
            *remaining -= 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && depth > 0 {
                visit(&path, remaining, depth - 1, paths);
            } else if kind.is_file()
                && path.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("zfab") || ext.eq_ignore_ascii_case("json")
                })
            {
                paths.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    visit(root, &mut 20_000, 16, &mut paths);
    paths.sort();
    paths
}

fn fabric_root() -> Option<PathBuf> {
    asset_subfolder("CLO_RUST_FABRIC_DIR", "Fabric")
}

struct NativeAvatar {
    parts: usize,
    arrangements: Vec<Value>,
    body_faces: Option<Range<usize>>,
    regions: Vec<super::mesh_measure::RegionFace>,
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
    if extension == "avt" {
        let stamp = crate::compat_scene::native_avatar_stamp(&path)?;
        let mut rest = avt::read(&path)?;
        let scene_name = rest.scene_name.take();
        let mut avatar = avatar_from_avt(scene_name.as_deref().unwrap_or(name), Some(&path), rest)?;
        if crate::compat_scene::native_avatar_stamp(&path)? != stamp {
            return Err("native avatar changed during import".into());
        }
        avatar["native_archive"] = json!({"path": path, "stamp": stamp});
        return Ok(avatar);
    }
    let (points, faces) = match extension.as_str() {
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
    avatar_from_mesh(name, Some(&path), points, faces, None)
}

pub(crate) fn project_avatar(path: &Path) -> Result<Vec<Value>, String> {
    let stamp = crate::compat_scene::native_avatar_stamp(path)?;
    let records = avt::read_project(path)?;
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let source = path.canonicalize().map_err(|error| error.to_string())?;
    if crate::compat_scene::native_avatar_stamp(&source)? != stamp {
        return Err("native project changed during avatar import".into());
    }
    records.into_iter().map(|(name, member, rest)| {
        let mut avatar = avatar_from_avt(&name, None, rest)?;
        avatar["source_project"] = json!(path);
        avatar["native_archive"] = json!({"path": source, "stamp": stamp, "member": member});
        Ok(avatar)
    }).collect()
}

fn avatar_from_avt(name: &str, path: Option<&Path>, rest: avt::RestMesh) -> Result<Value, String> {
    avatar_from_mesh(
        name,
        path,
        rest.points,
        rest.triangles,
        Some(NativeAvatar {
            parts: rest.part_count,
            arrangements: rest.arrangements,
            body_faces: rest.body_faces,
            regions: rest.regions,
        }),
    ).map(|mut avatar| {
        if !rest.native_arrangements.is_empty() {
            avatar["native_arrangements"] = json!(rest.native_arrangements);
        }
        avatar
    })
}

fn avatar_from_mesh(
    name: &str,
    path: Option<&Path>,
    mut points: Vec<[f64; 3]>,
    faces: Vec<[usize; 3]>,
    rest_details: Option<NativeAvatar>,
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
    let mut floor = ymin;
    let mut center_x = (xmin + xmax) / 2.0;
    if rest_details.is_none() {
        for point in &mut points {
            point[0] -= (xmin + xmax) / 2.0;
            point[1] -= ymin;
            point[2] -= (zmin + zmax) / 2.0;
        }
        floor = 0.0;
        center_x = 0.0;
    }
    let mut measure_height = height;
    let mut measure_faces = faces.as_slice();
    if let Some(range) = rest_details
        .as_ref()
        .and_then(|details| details.body_faces.clone())
    {
        let body_faces = &faces[range];
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for &index in body_faces.iter().flatten() {
            for axis in 0..2 {
                lo[axis] = lo[axis].min(points[index][axis]);
                hi[axis] = hi[axis].max(points[index][axis]);
            }
        }
        let body_height = hi[1] - lo[1];
        if (1.0..=5000.0).contains(&body_height) {
            measure_height = body_height;
            floor = lo[1];
            center_x = (lo[0] + hi[0]) / 2.0;
            measure_faces = body_faces;
        }
    }
    let mut body = scaled_body(measure_height, measure_height / 1755.0, "rust_mesh_estimate");
    if let Some(measurements) = rest_details.as_ref().and_then(|details| {
        super::mesh_measure::measure(&points, &details.regions)
    }) {
        body = measurements;
    } else {
        for (measurement, height_key) in [
        ("bust", "h_apex"),
        ("underbust", "h_underbust"),
        ("waist", "h_waist"),
        ("high_hip", "h_high_hip"),
        ("hip", "h_hip"),
    ] {
            let level = body[height_key].as_f64().unwrap() + floor;
            if let Some(circumference) =
                mesh_section(&points, measure_faces, level, center_x, measure_height)
            {
                body[measurement] = json!(circumference);
            }
        }
    }
    let mut avatar = json!({
        "id":name,"name":name,"path":path,"source":"rust_mesh_estimate","body":body,
        "arrangement_points":POINTS,"mesh":{"positions":points,"triangles":faces}
    });
    if let Some(NativeAvatar { parts, arrangements, .. }) = rest_details {
        avatar["source"] = json!("clo_avt_rest");
        avatar["measurement_source"] = json!("rust_mesh_estimate");
        avatar["rest_geometry"] = json!({
            "parts":parts,"vertices":points.len(),"triangles":faces.len(),
            "bounds_mm":[[xmin,ymin,zmin],[xmax,ymax,zmax]]
        });
        let mut warnings=vec![
            "Loaded saved AVT rest geometry only. Native skinning, posing, and avatar textures are unavailable.",
            "Body measurements are estimated. The imported mesh represents the avatar rest pose.",
        ];
        if arrangements.is_empty() {
            warnings.push("Native arrangement names are unavailable. Using built-in names.");
        } else {
            avatar["arrangement_points"] = json!(arrangements
                .iter()
                .filter_map(|entry| entry["ArrangementName"].as_str())
                .collect::<Vec<_>>());
            avatar["arrangement_records"] = json!(arrangements);
        }
        avatar["warnings"] = json!(warnings);
    }
    Ok(avatar)
}

fn mesh_section(
    points: &[[f64; 3]],
    faces: &[[usize; 3]],
    y: f64,
    center_x: f64,
    height: f64,
) -> Option<f64> {
    // Weld the slice endpoints in a 0.05 mm grid. UV seams and separate
    // mesh parts can have coincident vertices with different indices.
    let key = |point: [f64; 2]| point.map(|v| (v * 20.0).round() as i64);
    let mut positions = HashMap::new();
    let mut edges: HashMap<_, HashSet<_>> = HashMap::new();
    for face in faces {
        let mut intersections = [[0.0; 2]; 2];
        let mut count = 0;
        for i in 0..3 {
            let a = points[face[i]];
            let b = points[face[(i + 1) % 3]];
            if (a[1] <= y && b[1] > y) || (b[1] <= y && a[1] > y) {
                let t = (y - a[1]) / (b[1] - a[1]);
                intersections[count] = [a[0] + t * (b[0] - a[0]), a[2] + t * (b[2] - a[2])];
                count += 1;
            }
        }
        if count == 2 {
            let [a, b] = intersections.map(key);
            if a == b {
                continue;
            }
            positions.entry(a).or_insert(intersections[0]);
            positions.entry(b).or_insert(intersections[1]);
            edges.entry(a).or_default().insert(b);
            edges.entry(b).or_default().insert(a);
        }
    }
    // Estimate the tape around the central loop only. A hull of the whole
    // slice can span unrelated arm, hand, or accessory loops.
    let mut seen = HashSet::new();
    let mut best = None;
    for &start in edges.keys() {
        if !seen.insert(start) {
            continue;
        }
        let mut pending = vec![start];
        let mut contour = Vec::new();
        let mut closed = true;
        while let Some(node) = pending.pop() {
            contour.push(positions[&node]);
            closed &= edges[&node].len() == 2;
            pending.extend(edges[&node].iter().copied().filter(|p| seen.insert(*p)));
        }
        let center = contour.iter().map(|p| p[0]).sum::<f64>() / contour.len() as f64;
        if closed && (center - center_x).abs() <= height * 0.09 {
            if let Some(value) = tape_perimeter(contour) {
                best = Some(best.map_or(value, |previous: f64| previous.max(value)));
            }
        }
    }
    best
}

pub(super) fn tape_perimeter(mut section: Vec<[f64; 2]>) -> Option<f64> {
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

#[cfg(test)]
#[test]
fn avatar_tape_excludes_disconnected_arms() {
    let mut points = Vec::new();
    let mut faces = Vec::new();
    for (x, half_width) in [(0.0, 100.0), (-360.0, 30.0), (360.0, 30.0)] {
        let base = points.len();
        for y in [0.0, 200.0] {
            for (dx, z) in [
                (-half_width, -half_width),
                (half_width, -half_width),
                (half_width, half_width),
                (-half_width, half_width),
            ] {
                points.push([x + dx, y, z]);
            }
        }
        for i in 0..4 {
            let a = base + i;
            let b = base + (i + 1) % 4;
            faces.extend([[a, b, b + 4], [a, b + 4, a + 4]]);
        }
    }
    assert_eq!(mesh_section(&points, &faces, 80.0, 0.0, 1750.0), Some(800.0));
    assert_eq!(mesh_section(&points, &faces, 220.0, 0.0, 1750.0), None);
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
    json!({"in_scene":crate::compat_scene::avatar_names(state),"library":avatar_library(),"aliases":AVATARS.iter().map(|(alias,_)|alias).collect::<Vec<_>>()})
}

pub fn avatar_load(args: &Value, state: &mut Value) -> Result<Value, String> {
    let reference = string(args, "name_or_path", "mia");
    let reference = if reference.trim().is_empty() {
        "mia"
    } else {
        reference
    };
    let alias = reference.trim().to_ascii_lowercase();
    let canonical = AVATARS
        .iter()
        .find(|(from, _)| *from == alias)
        .map(|(_, name)| *name);
    let avatar = if let Some(path) = avatar_lookup(reference, canonical) {
        avatar_from_path(&path)?
    } else if let Some(name) = canonical.filter(|_| avatar_root().is_none()) {
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
    // The native MCP uses default ImportAvatar options, which replace the
    // current avatar for AVT files even when the bridge skips its explicit delete.
    let native_avt = avatar["source"] == "clo_avt_rest";
    if boolean(args, "replace", true) || native_avt || state["avatar"].is_null() {
        state["avatar"] = avatar;
        state["additional_avatars"] = json!([]);
    } else {
        array_mut(state, "additional_avatars").push(avatar);
    }
    Ok(json!({"ok":true,"avatars":crate::compat_scene::avatar_names(state)}))
}

pub(super) fn body_measurements(avatar: &Value, refresh: bool) -> (Value, String) {
    if !refresh {
        if let Some(cached) = avatar["name"]
            .as_str()
            .and_then(super::measurement_cache::read)
        {
            return cached;
        }
    }
    let body = &avatar["body"];
    let source = if body["source"] == "obj" {
        "approximate measurements from the avatar mesh (open Avatar > Avatar Editor once in CLO and rerun avatar_measurements to get CLO's exact values)"
    } else {
        string(
            avatar,
            "measurement_source",
            string(avatar, "source", "rust_scene"),
        )
    };
    (body.clone(), source.to_owned())
}

pub fn avatar_measurements(args: &Value, state: &Value) -> Result<Value, String> {
    let avatar = &state["avatar"];
    if avatar.is_null() {
        return Err("no avatar in the scene; call avatar_load first".into());
    }
    let (body, source) = body_measurements(avatar, boolean(args, "refresh", false));
    Ok(json!({"avatar":avatar["name"],"source":source,"body":body}))
}

pub fn arrangement_points(state: &Value) -> Result<Value, String> {
    let points = array(&state["avatar"]["arrangement_points"]);
    Ok(json!({"count":points.len(),"names":points}))
}

fn library_keyword(reference: &str) -> (&'static str, &'static str) {
    let lower = reference.to_ascii_lowercase();
    let mut best = ("V2_Woven_Crepe_CDC_1", "default (crepe)");
    let mut length = 0;
    for (keys, stem) in LIBRARY_KEYWORDS {
        for key in *keys {
            if key.len() > length && lower.contains(key) {
                best = (stem, key);
                length = key.len();
            }
        }
    }
    best
}

fn native_fabric_path(reference: &str) -> Result<Option<(PathBuf, &'static str)>, String> {
    let path = Path::new(reference);
    let explicit = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zfab"));
    if explicit && path.is_file() {
        return Ok(Some((path.to_path_buf(), "path")));
    }
    if explicit && path.components().count() > 1 {
        return Err(format!(
            "no fabric file matching {reference:?}; see fabric_library"
        ));
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        return Ok(None);
    }
    let root = fabric_root();
    let mut files = root.as_deref().map(fabric_paths).unwrap_or_default();
    files.retain(|path| {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zfab"))
    });
    let find = |name: &str| {
        files
            .iter()
            .find(|path| {
                [path.file_name(), path.file_stem()]
                    .into_iter()
                    .flatten()
                    .any(|candidate| candidate.eq_ignore_ascii_case(name))
            })
            .cloned()
    };
    if explicit {
        return find(reference).map(|path| Some((path, "path"))).ok_or_else(|| {
            format!("no fabric file matching {reference:?}; see fabric_library")
        });
    }
    if files.is_empty() {
        return Ok(None);
    }
    let (stem, matched) = library_keyword(reference);
    let partial = |name: &str| {
        let name = name.to_ascii_lowercase();
        (!name.is_empty())
            .then(|| {
                files
                    .iter()
                    .find(|p| {
                        p.file_stem().is_some_and(|stem| {
                            stem.to_string_lossy().to_ascii_lowercase().contains(&name)
                        })
                    })
                    .cloned()
            })
            .flatten()
    };
    let found = root
        .map(|root| root.join(format!("{stem}.zfab")))
        .filter(|path| path.is_file())
        .or_else(|| find(stem))
        .or_else(|| partial(stem))
        .or_else(|| find(reference))
        .or_else(|| partial(reference))
        .map(|path| (path, matched));
    found.map(Some).ok_or_else(|| {
        format!("no fabric file for {reference:?} (matched {matched}); see fabric_library")
    })
}

pub(super) fn load_material(reference: &str) -> Result<(Value, String, Option<PathBuf>), String> {
    let reference = reference.trim();
    let path = Path::new(reference);
    if let Some((path, matched)) = native_fabric_path(reference)? {
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        let data = crate::compat_scene::read_native_fabric(&canonical)
            .map_err(|error| format!("cannot load fabric {}: {error}", path.display()))?;
        return Ok((data, matched.to_owned(), Some(path)));
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        let path = if path.is_file() {
            path.to_path_buf()
        } else {
            fabric_root()
                .map(|dir| dir.join(path))
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

pub(super) fn planned_material(reference: &Value, color: &Value) -> Result<Value, String> {
    let reference = match reference {
        Value::String(reference) => reference,
        Value::Null | Value::Bool(false) => "",
        Value::Number(value) if value.as_f64() == Some(0.0) => "",
        Value::Array(value) if value.is_empty() => "",
        Value::Object(value) if value.is_empty() => "",
        _ => return Err("garment fabric must be a string".into()),
    };
    let (stem, matched) = if reference.trim().to_ascii_lowercase().ends_with(".zfab") {
        (reference, "path")
    } else {
        library_keyword(reference)
    };
    let rgb = match color.as_array().filter(|channels| channels.len() >= 3) {
        Some(channels) => normalized_rgb(channels)?,
        None => parse_color_text(color.as_str().unwrap_or_default())?,
    };
    Ok(json!({"library_stem":stem,"matched":matched,"color":rgb,"color_text":color}))
}

pub fn fabric_library(args: &Value) -> Value {
    let query = string(args, "filter", "").to_lowercase();
    let root = fabric_root();
    let custom = root.as_deref().map(fabric_paths).unwrap_or_default();
    let native = custom.iter().any(|p| {
        p.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zfab"))
    });
    let mut fabric_names: Vec<_> = MATERIALS
        .iter()
        .filter(|p| {
            !native
                && (p.name.to_ascii_lowercase().contains(&query)
                    || p.aliases.iter().any(|s| s.contains(&query)))
        })
        .map(|preset| preset.name.to_owned())
        .collect();
    fabric_names.extend(
        custom
            .iter()
            .filter(|path| {
                !native || path.extension().is_some_and(|ext| ext == "zfab")
            })
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()))
            .filter(|s| s.to_lowercase().contains(&query))
            .map(str::to_owned),
    );
    if !native {
        fabric_names.sort();
    }
    let mut keywords: Vec<_> = LIBRARY_KEYWORDS
        .iter()
        .flat_map(|(keys, _)| *keys)
        .copied()
        .collect();
    keywords.sort_unstable();
    keywords.dedup();
    json!({"folder":root,"fabrics":fabric_names,"keywords":keywords})
}

pub fn fabric_list(state: &Value) -> Value {
    let fabrics: Vec<_> = crate::compat_scene::fabric_catalog(state)
        .take(crate::compat_scene::assigned_fabric_count(state))
        .enumerate()
        .map(|(index, value)| {
            let material = &value["info"];
            let catalog = &material["catalog"];
            let properties = &material["physical_properties"];
            let rgb = serde_json::from_value::<[f64; 3]>(value["color"].clone()).unwrap_or([1.0; 3]);
            let [r, g, b] = rgb.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            let hex = format!("{r:02X}{g:02X}{b:02X}");
            let texture = value["texture_maps"]["base"].as_str()
                .and_then(|path| Path::new(path).file_stem()?.to_str());
            let thickness = properties["thickness"].as_f64()
                .map(|mm| format!("{} mm", mm as f32)).unwrap_or_default();
            let weight = properties["density"].as_f64()
                .map(|density| format!("{:.0} g/m2", density * 1_000_000.0)).unwrap_or_default();
            json!({
                "index":index,"name":value["name"],"info":{
                    "Classification": string(catalog, "Classification", string(material, "fabric_type", "None")),
                    "Color": string(catalog, "Color", &hex),
                    "ColorCode": format!("#{}", hex.to_ascii_lowercase()),
                    "Content": string(catalog, "Content", string(material, "composition", "None")),
                    "Owner": string(catalog, "Owner", "None"),
                    "Print": string(catalog, "Print", ""),
                    "SupplierName": string(catalog, "SupplierName", "None"),
                    "Texture": string(catalog, "Texture", texture.unwrap_or("None")),
                    "Thickness": thickness, "Weight": weight
                }
            })
        })
        .collect();
    let current = crate::compat_scene::current_fabric_catalog_index(state);
    json!({"fabrics":fabrics,"current":current})
}

pub(crate) fn preserves_opacity_on_recolor(fabric: &Value) -> bool {
    let name = fabric["name"].as_str().unwrap_or_default().to_ascii_lowercase();
    [
        "chiffon", "organza", "tulle", "mesh", "lace", "voile", "georgette", "gauze", "eyelet",
        "crochet", "pointelle", "net",
    ]
    .iter()
    .any(|hint| name.contains(hint))
}

fn update_material_color(fabric: &mut Value, rgb: [f64; 3]) -> bool {
    let clear_opacity = !preserves_opacity_on_recolor(fabric);
    fabric["color"] = json!(rgb);
    let [r, g, b] = rgb;
    fabric["rgba"] = json!([r, g, b, 1.0]);
    if let Some(faces) = fabric
        .get_mut("info")
        .and_then(|info| info.get_mut("face_colors"))
        .and_then(Value::as_array_mut)
    {
        for face in faces {
            if matches!(face["type"].as_u64(), Some(0..=2)) {
                face["rgba"] = json!([r, g, b, 1.0]);
            }
        }
    }
    if fabric["texture_maps"]["base"].is_string() {
        fabric["base_texture_desaturated"] = json!(true);
    }
    if let Some(maps) = fabric["texture_maps"].as_object_mut() {
        if clear_opacity {
            maps.remove("opacity");
        }
    }
    if let Some(catalog) = fabric["info"]["catalog"].as_object_mut() {
        catalog.remove("Color");
    }
    clear_opacity
}

fn update_material_texture(fabric: &mut Value, kind: &str, path: &Path) {
    fabric["texture_maps"][kind] = json!(path);
    if kind == "base" {
        fabric["base_texture_desaturated"] = json!(false);
        if let Some(catalog) = fabric["info"]["catalog"].as_object_mut() {
            catalog.remove("Texture");
        }
    }
}

fn normalized_rgb(channels: &[Value]) -> Result<[f64; 3], String> {
    let mut rgb = [0.0; 3];
    for (channel, value) in rgb.iter_mut().zip(channels) {
        *channel = value.as_f64()
            .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
            .or_else(|| value.as_bool().map(|value| f64::from(u8::from(value))))
            .filter(|value| value.is_finite())
            .ok_or("RGB channels must be finite numbers")?;
    }
    let scale = if rgb.iter().any(|channel| *channel > 1.0) { 255.0 } else { 1.0 };
    Ok(rgb.map(|channel| channel / scale))
}

pub fn parse_color(input: &Value) -> Result<[f64; 3], String> {
    let Some(channels) = input.as_array() else {
        return parse_color_text(input.as_str().unwrap_or_default());
    };
    if channels.len() < 3 {
        return Err("RGB requires three channels".into());
    }
    let rgb = normalized_rgb(channels)?;
    if rgb.iter().any(|v| !(0.0..=1.0).contains(v)) {
        return Err("RGB channels are out of range".into());
    }
    Ok(rgb)
}

fn parse_color_text(input: &str) -> Result<[f64; 3], String> {
    let value = input.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#').filter(|hex| matches!(hex.len(), 3 | 6)) {
        if !hex.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err(format!("invalid RGB color {value:?}"));
        }
        let stride = hex.len() / 3;
        return Ok(std::array::from_fn(|i| {
            let code = &hex[stride * i..stride * (i + 1)];
            f64::from(u8::from_str_radix(code, 16).expect("ASCII hex"))
                / if stride == 1 { 15.0 } else { 255.0 }
        }));
    }
    let colors = [
        ("black", [0.05, 0.05, 0.05]),
        ("white", [0.96, 0.96, 0.96]),
        ("ivory", [0.95, 0.92, 0.84]),
        ("cream", [0.96, 0.93, 0.86]),
        ("red", [0.78, 0.08, 0.12]),
        ("crimson", [0.7, 0.05, 0.15]),
        ("burgundy", [0.45, 0.06, 0.14]),
        ("wine", [0.4, 0.08, 0.16]),
        ("pink", [0.95, 0.6, 0.72]),
        ("blush", [0.93, 0.76, 0.75]),
        ("hot pink", [0.92, 0.2, 0.55]),
        ("fuchsia", [0.85, 0.1, 0.5]),
        ("coral", [0.98, 0.5, 0.45]),
        ("orange", [0.95, 0.5, 0.12]),
        ("peach", [0.98, 0.8, 0.65]),
        ("yellow", [0.98, 0.85, 0.2]),
        ("mustard", [0.85, 0.65, 0.13]),
        ("gold", [0.83, 0.68, 0.22]),
        ("green", [0.15, 0.5, 0.25]),
        ("emerald", [0.02, 0.55, 0.35]),
        ("sage", [0.6, 0.7, 0.55]),
        ("olive", [0.45, 0.48, 0.2]),
        ("mint", [0.6, 0.9, 0.75]),
        ("forest", [0.05, 0.3, 0.15]),
        ("teal", [0.05, 0.45, 0.5]),
        ("turquoise", [0.2, 0.75, 0.75]),
        ("blue", [0.15, 0.3, 0.75]),
        ("navy", [0.1, 0.16, 0.42]),
        ("royal blue", [0.15, 0.25, 0.8]),
        ("sky blue", [0.55, 0.75, 0.95]),
        ("baby blue", [0.65, 0.8, 0.95]),
        ("cobalt", [0.0, 0.28, 0.67]),
        ("purple", [0.45, 0.2, 0.6]),
        ("lavender", [0.75, 0.68, 0.9]),
        ("lilac", [0.78, 0.65, 0.85]),
        ("violet", [0.5, 0.25, 0.75]),
        ("plum", [0.5, 0.15, 0.35]),
        ("brown", [0.4, 0.25, 0.15]),
        ("chocolate", [0.3, 0.17, 0.1]),
        ("tan", [0.8, 0.65, 0.45]),
        ("beige", [0.85, 0.78, 0.65]),
        ("camel", [0.76, 0.6, 0.42]),
        ("khaki", [0.7, 0.65, 0.45]),
        ("grey", [0.5, 0.5, 0.5]),
        ("gray", [0.5, 0.5, 0.5]),
        ("charcoal", [0.2, 0.2, 0.22]),
        ("silver", [0.75, 0.75, 0.78]),
        ("nude", [0.87, 0.72, 0.6]),
        ("champagne", [0.95, 0.87, 0.72]),
        ("rose", [0.85, 0.45, 0.55]),
        ("magenta", [0.8, 0.1, 0.6]),
        ("cyan", [0.1, 0.7, 0.8]),
    ];
    Ok(colors
        .iter()
        .rev()
        .filter(|(name, _)| value.contains(name))
        .max_by_key(|(name, _)| name.len())
        .map_or([0.85; 3], |(_, rgb)| *rgb))
}

fn validate_texture_map(kind: &str) -> Result<(), String> {
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
    Ok(())
}

fn texture(path: &str, kind: &str) -> Result<PathBuf, String> {
    validate_texture_map(kind)?;
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
            return Err(
                "the native base-color renderer requires a static PNG or JPEG texture".into(),
            );
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
    apply_material(args, state, load_material(string(args, "fabric", ""))?)
}

pub(super) fn fabric_add(args: &Value, state: &mut Value) -> Result<Value, String> {
    let (mut material, _, file) = load_material(string(args, "fabric", ""))?;
    let native_name = file.as_deref().filter(|path| {
        path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("zfab"))
    }).and_then(|path| path.file_stem()?.to_str());
    if let Some(name) = native_name {
        material["name"] = json!(name);
    }
    let preserve_default = array(&state["patterns"]).iter().any(|pattern| {
        pattern["fabric_index"].as_u64().unwrap_or(0) == 0
    });
    Ok(json!(register_material(state, material, preserve_default)?))
}

pub(super) fn fabric_assign(args: &Value, state: &mut Value) -> Result<Value, String> {
    let index = args["fabric_index"]
        .as_u64()
        .ok_or("fabric_index must be a nonnegative integer")?;
    find_fabric_mut(state, index)?;
    let pattern = pattern_index(array(&state["patterns"]), &args["pattern"])?;
    assign_material(state, &[pattern], index);
    Ok(json!(true))
}

pub(super) fn apply_material(
    args: &Value,
    state: &mut Value,
    (material, matched, file): (Value, String, Option<PathBuf>),
) -> Result<Value, String> {
    let patterns = array(&state["patterns"]);
    let pattern_refs = array(&args["patterns"]);
    let panel_count = patterns.len();
    let selection: Result<Vec<i64>, _> = if pattern_refs.is_empty() {
        (0..panel_count).map(|index| i64::try_from(index).map_err(|_| "pattern index is too large".into())).collect()
    } else {
        pattern_refs
            .iter()
            .map(|r| crate::compat_scene::bridge_pattern_index(patterns, r))
            .collect()
    };
    // Resolve every name before editing. Numeric references instead reach
    // the native assignment one by one, where an invalid value can fail late.
    let selected = selection.as_deref().unwrap_or_default();
    let preserve_default = patterns.iter().enumerate().any(|(i, pattern)| {
        pattern["fabric_index"].as_u64().unwrap_or(0) == 0 && !selected.contains(&(i as i64))
    });
    let name = material["name"].clone();
    let index = register_material(state, material, preserve_default)?;
    let selected = selection?;
    let mut assigned = Vec::with_capacity(selected.len());
    for &reference in &selected {
        let panel = u32::try_from(reference).map_err(|_| "pattern index must fit in an unsigned 32-bit integer")?;
        let panel = panel as usize;
        let exists = panel < panel_count;
        if exists {
            assign_material(state, &[panel], index);
        }
        assigned.push(exists);
    }
    let mut result = json!({"fabric":{"index":index,"name":name},"matched":matched,"file":file,
        "assigned":{"assigned":assigned,"patterns":selected}});
    if let Some(color) = args
        .get("color")
        .filter(|v| !v.is_null() && v.as_str() != Some(""))
    {
        let rgb = parse_color(color)?;
        update_material_color(find_fabric_mut(state, index)?, rgb);
        result["rgb"] = json!(rgb);
        result["color"] = json!({"ok":[true,true,true]});
    }
    if let Some(path) = args["texture_image"].as_str().filter(|s| !s.is_empty()) {
        result["texture"] = fabric_set_texture(&json!({"fabric_index":index,"image_path":path}), state)?;
    }
    crate::compat_scene::refresh_colorway_exports(state);
    Ok(result)
}

fn register_material(state: &mut Value, mut material: Value, preserve_default: bool) -> Result<u64, String> {
    let fabrics = array_mut(state, "fabrics");
    if fabrics.is_empty() && preserve_default {
        fabrics.push(crate::compat_scene::default_fabric());
    }
    let index = fabrics
        .iter()
        .filter_map(|f| f["index"].as_u64())
        .max()
        .map_or(Some(0), |i| i.checked_add(1))
        .ok_or("fabric index space exhausted")?;
    material["index"] = json!(index);
    fabrics.push(material);
    state["current_fabric"] = json!(index);
    state["runtime"]["last_added_fabric"] = json!(index);
    Ok(index)
}

fn assign_material(state: &mut Value, selected: &[usize], index: u64) {
    let patterns = array_mut(state, "patterns");
    let mut ids = std::collections::HashSet::new();
    for i in selected {
        patterns[*i]["fabric_index"] = json!(index);
        ids.insert(patterns[*i]["id"].clone());
    }
    if let Some(meshes) = state["simulation"].get_mut("meshes").and_then(Value::as_array_mut) {
        for mesh in meshes.iter_mut().filter(|mesh| ids.contains(&mesh["id"])) {
            mesh["fabric_index"] = json!(index);
        }
    }
}

fn find_fabric_mut(state: &mut Value, index: u64) -> Result<&mut Value, String> {
    state
        .get_mut("fabrics")
        .and_then(Value::as_array_mut)
        .and_then(|fabrics| fabrics.iter_mut().find(|f| f["index"] == index))
        .ok_or_else(|| format!("no fabric at index {index}"))
}

pub fn fabric_set_color(args: &Value, state: &mut Value) -> Result<Value, String> {
    let rgb = parse_color(&args["color"])?;
    let index = args["fabric_index"]
        .as_u64()
        .filter(|&value| u32::try_from(value).is_ok())
        .ok_or("fabric_index must be an unsigned 32-bit integer")?;
    if let Ok(fabric) = find_fabric_mut(state, index) {
        let clear_opacity = update_material_color(fabric, rgb);
        let selected = if clear_opacity { index } else { state["current_fabric"].as_u64().unwrap_or(0) };
        crate::compat_scene::mark_native_fabric_color(state, index, clear_opacity);
        finish_fabric_edit(state, selected);
        return Ok(json!({"rgb":rgb,"result":{"ok":[true,true,true]}}));
    }
    // The face setters return false, but the bridge's cleanup still uses the
    // current fabric when selecting a nonexistent fabric has no effect.
    let current = state["current_fabric"].as_u64().unwrap_or(0);
    for kind in ["base", "opacity"] {
        clear_fabric_texture(state, current, kind);
    }
    finish_fabric_edit(state, current);
    Ok(json!({"rgb":rgb,"result":{"ok":[false,false,false]}}))
}

fn finish_fabric_edit(state: &mut Value, selected: u64) {
    // CLO’s colorway update reselects the latest fabric added to the live scene.
    let current = state["runtime"]["last_added_fabric"].as_u64().unwrap_or(selected);
    state["current_fabric"] = json!(current);
    crate::compat_scene::refresh_colorway_exports(state);
}

fn clear_fabric_texture(state: &mut Value, index: u64, kind: &str) {
    if let Ok(fabric) = find_fabric_mut(state, index) {
        if let Some(maps) = fabric["texture_maps"].as_object_mut() {
            maps.remove(kind);
        }
        if kind == "base" {
            fabric["base_texture_desaturated"] = json!(false);
            if let Some(catalog) = fabric["info"]["catalog"].as_object_mut() {
                catalog.remove("Texture");
            }
        }
        crate::compat_scene::mark_native_fabric_texture(state, index, kind, true);
    }
}

pub fn fabric_set_texture(args: &Value, state: &mut Value) -> Result<Value, String> {
    let requested = args["fabric_index"]
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .ok_or("fabric_index must be a signed 32-bit integer")?;
    let current = state["current_fabric"].as_u64().unwrap_or(0);
    let index = u64::try_from(requested)
        .ok()
        .filter(|&index| array(&state["fabrics"]).iter().any(|fabric| fabric["index"] == index))
        .unwrap_or(current);
    let kind = string(args, "map", "base");
    let input = string(args, "image_path", "");
    if let Err(error) = validate_texture_map(kind) {
        state["current_fabric"] = json!(index);
        return Err(error);
    }
    if input.is_empty() {
        clear_fabric_texture(state, index, kind);
    } else if let Ok(path) = texture(input, kind) {
        if let Ok(fabric) = find_fabric_mut(state, index) {
            update_material_texture(fabric, kind, &path);
            crate::compat_scene::mark_native_fabric_texture(state, index, kind, false);
        }
    }
    finish_fabric_edit(state, index);
    Ok(json!({"ok":true}))
}

pub(super) fn assigned_material<'a>(state: &'a Value, id: &Value) -> &'a Value {
    array(&state["fabrics"])
        .iter()
        .find(|m| !id.is_null() && m["index"] == *id)
        .unwrap_or(&Value::Null)
}
