use super::{array, flag, formats, invalidate_geometry, patterns, string, use_partial_pose, vec_mut, Reply};
use formats::clo_native::SceneGarment;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCENE_FORMAT: &str = "clo-rust.scene.v1";
const PATTERN_FORMAT: &str = "clo-rust.pattern.v1";
const DEFAULT_COLORWAY: &str = "Default Colorway";

pub(crate) fn default_fabric() -> Value {
    json!({
        "index": 0, "name": "FABRIC 1", "stiffness": 0.9,
        "color": [1.0, 1.0, 1.0], "texture_maps": {},
        "info": {
            "source": "rust_default",
            "physical_properties": {"thickness": 0.5, "density": 0.0003}
        }
    })
}

pub(crate) fn empty_project() -> Value {
    json!({
        "patterns": [], "seams": [], "native_seam_groups": [],
        "avatar": null, "additional_avatars": [], "fabrics": [default_fabric()], "imported_meshes": [],
        "simulation": null, "garment_spec": null, "garment_piece_names": [],
        "colorways": [DEFAULT_COLORWAY], "current_colorway": 0
    })
}

pub(crate) fn avatar_names(state: &Value) -> Vec<&str> {
    let avatar = &state["avatar"];
    avatar
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(avatar))
        .iter()
        .chain(array(state, "additional_avatars"))
        .filter_map(|a| a["name"].as_str())
        .collect()
}

fn open_reply(state: &Value, path: &Path) -> Value {
    json!({"ok": true, "path": path, "pattern_count": array(state, "patterns").len(), "avatars": avatar_names(state)})
}

pub(crate) fn assigned_fabrics(state: &Value) -> impl Iterator<Item = &Value> {
    let assignments = array(state, "patterns")
        .iter()
        .map(|pattern| pattern["fabric_index"].as_u64().unwrap_or(0))
        .collect::<HashSet<_>>();
    array(state, "fabrics")
        .iter()
        .enumerate()
        .filter_map(move |(position, fabric)| {
            assignments.contains(&fabric["index"].as_u64().unwrap_or(position as u64)).then_some(fabric)
        })
}

pub(crate) fn assigned_fabric_count(state: &Value) -> usize {
    assigned_fabrics(state).count()
}

pub(super) fn info(_: &Value, state: &mut Value) -> Reply {
    let pieces: Vec<_> = array(state, "patterns")
        .iter()
        .enumerate()
        .map(|(i, pattern)| patterns::summary(pattern, i))
        .collect();
    // The bridge enumerates the first N fabric indices, where N is the used count.
    let fabrics: Vec<_> = super::fabric_catalog(state)
        .take(assigned_fabric_count(state))
        .enumerate()
        .map(|(i, f)| json!({"index": i, "name": f["name"]}))
        .collect();
    Ok(json!({
        "project": state["project"].as_str().unwrap_or("NULL"),
        "project_path": state["project_path"].as_str().unwrap_or("NULL"),
        "clo_version": format!("clo-rust/{}", env!("CARGO_PKG_VERSION")),
        "pattern_count": pieces.len(), "patterns": pieces,
        "avatars": avatar_names(state), "fabric_count": fabrics.len(), "fabrics": fabrics,
        "colorways": state.get("colorways").cloned().unwrap_or_else(|| json!([DEFAULT_COLORWAY])),
        "current_colorway": state["current_colorway"].as_u64().unwrap_or(0),
        "seam_groups": super::annotations::count(state),
        "temp_folder": env::temp_dir(),
        "asset_folder": env::var_os("CLO_RUST_ASSET_DIR").or_else(|| env::var_os("CLO_MCP_ASSETS")).map(PathBuf::from)
    }))
}

pub(crate) fn clear_patterns(state: &mut Value) {
    for field in ["patterns", "seams", "native_seam_groups"] {
        state[field] = json!([]);
    }
    state["garment_spec"] = Value::Null;
    state["garment_piece_names"] = json!([]);
    state.as_object_mut().expect("scene object").remove(formats::native_save::SOURCE);
    invalidate_geometry(state);
}

fn clear_garment(state: &mut Value) {
    clear_patterns(state);
    state["imported_meshes"] = json!([]);
}

pub(super) fn clear(args: &Value, state: &mut Value) -> Reply {
    let keep_avatar = flag(args, "keep_avatar", true);
    crate::compat_runtime::clear_scene_ui(state, keep_avatar);
    if keep_avatar {
        let count = array(state, "patterns").len();
        clear_garment(state);
        return Ok(json!({"deleted": count, "pattern_count": 0}));
    }
    replace_scene(state, empty_project());
    Ok(json!({"pattern_count": 0, "avatars": []}))
}

fn set_project(state: &mut Value, path: &Path) {
    state["project"] = json!(path
        .file_stem()
        .and_then(|p| p.to_str())
        .unwrap_or("Untitled"));
    state["project_path"] = json!(path);
}

fn recognized_state(content: Value) -> Result<Value, String> {
    if content["format"] != SCENE_FORMAT {
        return Err(format!("expected a {SCENE_FORMAT} JSON project"));
    }
    let state = content
        .get("scene")
        .filter(|state| state.is_object())
        .ok_or("expected a scene object")?;
    let _ = validate_garment(state)?;
    let mut scene = state.clone();
    if let Some(object) = scene.as_object_mut() {
        object.remove("runtime");
        object.remove(formats::native_save::SOURCE);
    }
    Ok(scene)
}

fn replace_scene(state: &mut Value, mut scene: Value) {
    if let Some(runtime) = state.get("runtime") {
        scene["runtime"] = runtime.clone();
        if let Some(runtime) = scene["runtime"].as_object_mut() {
            runtime.remove("last_added_fabric");
        }
    }
    *state = scene;
}

fn id_key(id: &Value) -> Result<String, String> {
    if !(id.is_u64() || id.as_str().is_some_and(|s| !s.is_empty())) {
        return Err("pattern ID must be an unsigned integer or nonempty string".into());
    }
    Ok(id.to_string())
}

fn validate_garment(content: &Value) -> Result<(&[Value], &[Value]), String> {
    let pieces = content["patterns"]
        .as_array()
        .ok_or("patterns must be an array")?;
    let seams = content["seams"]
        .as_array()
        .ok_or("seams must be an array")?;
    let mut edges = HashMap::new();
    for piece in pieces {
        let key = id_key(&piece["id"])?;
        string(piece, "name")?;
        let corners = patterns::points(piece)?
            .iter()
            .filter(|x| x.kind == 0)
            .count();
        if edges.insert(key, corners).is_some() {
            return Err("duplicate pattern ID".into());
        }
    }
    for seam in seams {
        for (p, l) in [("pattern_a", "line_a"), ("pattern_b", "line_b")] {
            let key = id_key(&seam[p])?;
            let count = edges.get(&key).ok_or("seam refers to an unknown pattern")?;
            if super::index(seam, l)? >= *count {
                return Err("seam line index is out of range".into());
            }
        }
    }
    super::annotations::validate(content, pieces, seams)?;
    Ok((pieces, seams))
}

fn merge(state: &mut Value, incoming: &Value, append: bool) -> Result<(), String> {
    let (pieces, seams) = validate_garment(incoming)?;
    if !append {
        clear_garment(state);
        state["patterns"] = Value::Array(pieces.to_vec());
        state["seams"] = Value::Array(seams.to_vec());
        state["native_seam_groups"] = json!(array(incoming, "native_seam_groups"));
        return Ok(());
    }
    let next = super::next_id(state)?;
    let mut ids = HashMap::new();
    let mut additions = Vec::with_capacity(pieces.len());
    for (i, p) in pieces.iter().enumerate() {
        let new_id = next
            .checked_add(i as u64)
            .ok_or("pattern ID space exhausted")?;
        ids.insert(id_key(&p["id"])?, json!(new_id));
        let mut new_pattern = p.clone();
        new_pattern["id"] = json!(new_id);
        additions.push(new_pattern);
    }
    let remap_id = |id: &Value| -> Result<Value, String> {
        ids.get(&id_key(id)?)
            .cloned()
            .ok_or_else(|| "seam refers to an unknown panel".to_string())
    };
    let groups = array(incoming, "native_seam_groups").iter();
    let (groups, group_ids) = super::annotations::remap(state, groups, remap_id)?;
    let mut joins = Vec::with_capacity(seams.len());
    for seam in seams {
        let mut new_seam = seam.clone();
        for key in ["pattern_a", "pattern_b"] {
            new_seam[key] = remap_id(&seam[key])?;
        }
        if let Some(id) = seam["native_group"].as_u64() {
            new_seam["native_group"] = json!(group_ids
                .get(&id)
                .ok_or("missing appended native seam group")?);
        }
        joins.push(new_seam);
    }
    if !additions.is_empty() {
        use_partial_pose(state);
    }
    vec_mut(state, "native_seam_groups").extend(groups);
    vec_mut(state, "patterns").extend(additions);
    vec_mut(state, "seams").extend(joins);
    Ok(())
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn prepare_archived_fabrics(
    state: &Value,
    loaded: &mut Value,
    append: bool,
) -> Result<Vec<Value>, String> {
    let mut fabrics = array(loaded, "fabrics").to_vec();
    if fabrics.is_empty() {
        return Ok(fabrics);
    }
    if append {
        let offset = array(state, "fabrics")
            .iter()
            .enumerate()
            .map(|(at, fabric)| fabric["index"].as_u64().unwrap_or(at as u64))
            .max()
            .map(|id| {
                id.checked_add(1)
                    .ok_or("native material-index limit exceeded")
            })
            .transpose()?
            .unwrap_or(0);
        for (index, fabric) in fabrics.iter_mut().enumerate() {
            fabric["index"] = json!(offset
                .checked_add(index as u64)
                .ok_or("native material-index limit exceeded")?);
            super::remap_appended_colorways(fabric, state, loaded);
            if loaded["colorways"].is_array() {
                fabric["appended_catalog"] = json!(true);
            }
        }
        for field in ["patterns", "imported_meshes"] {
            for piece in vec_mut(loaded, field) {
                if piece["fabric_index"].is_null() && field == "imported_meshes" {
                    continue;
                }
                let index = super::index(piece, "fabric_index")?;
                let index = offset
                    .checked_add(index as u64)
                    .ok_or("native material-index limit exceeded")?;
                piece["fabric_index"] = json!(index);
            }
        }
    }
    Ok(fabrics)
}

fn append_native_pose(
    existing: Option<Value>,
    prior_count: usize,
    panels: &[Value],
    imported: &Value,
) -> Option<Value> {
    let pose = array(imported, "saved_pose");
    let added = &panels[prior_count..];
    let original = array(imported, "patterns");
    if !pose.is_empty() && (pose.len() != added.len() || pose.len() != original.len()) {
        return None;
    }
    let mut previous = existing.unwrap_or(Value::Null);
    let complete = (prior_count == 0 || previous["source"] == "native_saved_pose")
        && array(&previous, "meshes").len() == prior_count
        && array(&previous, "meshes").iter().zip(panels).all(|(mesh, panel)| mesh["id"] == panel["id"])
        && pose.len() == added.len();
    let previous_native = previous["source"] == "native_saved_pose"
        || previous["texcoords_units"] == "mm";
    let mut meshes = match previous.get_mut("meshes").map(Value::take) {
        Some(Value::Array(meshes)) => meshes,
        _ => Vec::new(),
    };
    meshes.retain(|mesh| panels[..prior_count].iter().any(|panel| panel["id"] == mesh["id"]));
    if !complete && previous_native {
        for mesh in &mut meshes {
            mesh["texcoords_units"] = json!("mm");
        }
    }
    for ((mesh, old), new) in pose.iter().zip(original).zip(added) {
        if mesh["id"] != old["id"] {
            return None;
        }
        let mut mesh = mesh.clone();
        mesh["id"] = new["id"].clone();
        if !complete {
            mesh["texcoords_units"] = json!("mm");
        }
        meshes.push(mesh);
    }
    if meshes.is_empty() {
        return None;
    }
    if complete {
        return Some(json!({"source": "native_saved_pose", "meshes": meshes}));
    }
    let mut mixed = json!({"source": "mixed_saved_pose", "meshes": meshes});
    for field in ["steps", "quality", "particle_distance", "mesh_refinement"] {
        if let Some(value) = previous.get_mut(field) {
            mixed[field] = value.take();
        }
    }
    Some(mixed)
}

fn append_portable(state: &mut Value, mut loaded: Value) -> Result<(), String> {
    let mut next = state.clone();
    let mut material_ids = HashMap::new();
    let max_material_id = array(state, "fabrics")
        .iter()
        .enumerate()
        .map(|(i, material)| material["index"].as_u64().unwrap_or(i as u64))
        .chain(array(state, "patterns").iter().map(|panel| panel["fabric_index"].as_u64().unwrap_or(0)))
        .chain(array(state, "imported_meshes").iter().filter_map(|mesh| mesh["fabric_index"].as_u64()))
        .max();
    let mut last = max_material_id;
    let mut remap_material = |old| -> Result<Value, String> {
        if let Some(id) = material_ids.get(&old) {
            return Ok(json!(id));
        }
        let id = last.map_or(Some(0), |id| id.checked_add(1))
            .ok_or("material index space exhausted")?;
        last = Some(id);
        material_ids.insert(old, id);
        Ok(json!(id))
    };
    for (i, material) in vec_mut(&mut loaded, "fabrics").iter_mut().enumerate() {
        material["index"] = remap_material(material["index"].as_u64().unwrap_or(i as u64))?;
    }
    for field in ["patterns", "imported_meshes"] {
        for piece in vec_mut(&mut loaded, field) {
            if let Some(id) = piece["fabric_index"].as_u64().or_else(|| (field == "patterns").then_some(0)) {
                piece["fabric_index"] = remap_material(id)?;
            }
        }
    }
    let prior_count = array(state, "patterns").len();
    merge(&mut next, &loaded, true)?;
    let mut meshes = portable_pose_meshes(state, &array(state, "patterns")[..prior_count])?;
    let keep_previous_clock = !meshes.is_empty();
    meshes.extend(portable_pose_meshes(&loaded, &array(&next, "patterns")[prior_count..])?);
    if !meshes.is_empty() {
        let clock = if keep_previous_clock { &state["simulation"] } else { &loaded["simulation"] };
        let mut pose = json!({"source": "mixed_saved_pose", "meshes": meshes});
        for field in ["steps", "quality", "particle_distance", "mesh_refinement"] {
            if let Some(value) = clock.get(field) {
                pose[field] = value.clone();
            }
        }
        next["simulation"] = pose;
    }
    for field in ["fabrics", "imported_meshes"] {
        vec_mut(&mut next, field).append(vec_mut(&mut loaded, field));
    }
    let avatar = loaded["avatar"].take();
    if !avatar.is_null() {
        if next["avatar"].is_null() {
            next["avatar"] = avatar;
        } else {
            vec_mut(&mut next, "additional_avatars").push(avatar);
        }
    }
    vec_mut(&mut next, "additional_avatars").append(vec_mut(&mut loaded, "additional_avatars"));
    *state = next;
    Ok(())
}

fn portable_pose_meshes(source: &Value, mapped: &[Value]) -> Result<Vec<Value>, String> {
    let mut by_id = HashMap::new();
    for mesh in array(&source["simulation"], "meshes") {
        if by_id.insert(&mesh["id"], mesh).is_some() {
            return Err("duplicate saved pose mesh for a panel".into());
        }
    }
    let mut meshes = Vec::new();
    for (old, new) in array(source, "patterns").iter().zip(mapped) {
        let Some(saved) = by_id.remove(&old["id"]) else { continue };
        let mut mesh = saved.clone();
        mesh["id"] = new["id"].clone();
        mesh["fabric_index"] = new["fabric_index"].clone();
        if crate::compat_fit::mesh_uses_rest_mm(source, saved) {
            mesh["texcoords_units"] = json!("mm");
        }
        meshes.push(mesh);
    }
    if !by_id.is_empty() {
        return Err("saved pose mesh refers to an unknown panel".into());
    }
    Ok(meshes)
}

pub(super) fn open(args: &Value, state: &mut Value) -> Reply {
    open_with_components(args, state, true, true)
}

pub(super) fn import_project(args: &Value, state: &mut Value) -> Reply {
    open_with_components(
        args, state, flag(args, "load_garment", true), flag(args, "load_avatar", true),
    )
}

fn open_with_components(args: &Value, state: &mut Value, load_garment: bool, load_avatar: bool) -> Reply {
    let path = Path::new(string(args, "path")?);
    if !path.is_file() {
        return Err(format!("file not found: {}", path.display()));
    }
    let append = args["mode"].as_str() == Some("append");
    let ext = extension(path);
    if (!load_garment || !load_avatar) && ext != "zprj" {
        return Err("selective project import requires a .zprj file".into());
    }
    if !load_garment {
        let avatar = crate::compat_fit::project_avatar(path)?;
        if load_avatar {
            import_project_avatar(state, avatar, append);
            crate::compat_runtime::invalidate_scene_ui_snapshot(state);
        }
        return Ok(open_reply(state, path));
    }
    let (reply, appended) = if matches!(ext.as_str(), "json" | "clorust") {
        let appended = open_json(path, append, state)?;
        (open_reply(state, path), appended)
    } else {
        // Standalone avatar and generic mesh imports keep the rest of the scene.
        let append = append || matches!(ext.as_str(), "obj" | "fbx" | "avt");
        let Some(reply) = open_path(path, &ext, append, load_avatar, state)? else {
            return Ok(open_reply(state, path));
        };
        (reply, append)
    };
    super::refresh_colorway_exports(state);
    if appended {
        crate::compat_runtime::invalidate_scene_ui_snapshot(state);
    } else {
        crate::compat_runtime::clear_scene_ui(state, true);
    }
    Ok(reply)
}

fn import_project_avatar(state: &mut Value, avatars: Vec<Value>, append: bool) {
    if !append {
        state["additional_avatars"] = json!([]);
        state["avatar"] = Value::Null;
    }
    for avatar in avatars {
        if state["avatar"].is_null() {
            state["avatar"] = avatar;
        } else {
            vec_mut(state, "additional_avatars").push(avatar);
        }
    }
}

fn open_json(path: &Path, append: bool, state: &mut Value) -> Result<bool, String> {
    let content = formats::read_json(path)?;
    match content["format"].as_str() {
        Some(SCENE_FORMAT) => {
            let loaded = recognized_state(content)?;
            if append {
                append_portable(state, loaded)?;
            } else {
                replace_scene(state, loaded);
                set_project(state, path);
            }
            return Ok(append);
        }
        Some(PATTERN_FORMAT) => merge(state, &content, true)?,
        _ if content.get("pieces").is_some() => {
            let converted = formats::pattern_native(&content)?;
            merge(state, &converted, true)?;
        }
        _ => return Err("unknown JSON scene or pattern format".into()),
    }
    Ok(true)
}

fn open_path(path: &Path, ext: &str, append: bool, load_avatar: bool, state: &mut Value) -> Result<Option<Value>, String> {
    match ext {
        "dxf" => merge(state, &formats::read_dxf(path)?, append)?,
        ext @ ("obj" | "fbx") => {
            let mut imported = formats::read_mesh_scene(path, ext)?;
            let fabrics = prepare_archived_fabrics(state, &mut imported, true)?;
            vec_mut(state, "fabrics").extend(fabrics);
            vec_mut(state, "imported_meshes").append(vec_mut(&mut imported, "imported_meshes"));
            let mut reply = open_reply(state, path);
            if !array(&imported, "warnings").is_empty() {
                reply["warnings"] = imported["warnings"].clone();
            }
            return Ok(Some(reply));
        }
        "glb" | "gltf" => {
            let meshes = formats::read_mesh(path)?;
            if !append {
                clear_garment(state);
            }
            vec_mut(state, "imported_meshes").extend(meshes);
        }
        ext @ ("zprj" | "zpac") => {
            let is_project = ext == "zprj";
            let mut intact = true;
            let loaded = if is_project {
                if load_avatar {
                    formats::clo_native::read_scene_project(path)?
                } else {
                    Some(formats::clo_native::read_project_garment(path)?)
                }
            } else {
                match formats::clo_native::read_scene_garment(path)? {
                    SceneGarment::UnreadableArchive => return Ok(None),
                    SceneGarment::Empty if append => return Ok(None),
                    SceneGarment::Empty => {
                        let mut empty = empty_project();
                        empty["avatar"] = state["avatar"].take();
                        empty["additional_avatars"] = state["additional_avatars"].take();
                        empty["project"] = json!("NULL");
                        empty["project_path"] = json!("NULL");
                        replace_scene(state, empty);
                        return Ok(Some(open_reply(state, path)));
                    }
                    SceneGarment::Loaded { garment, intact: source_intact } => {
                        intact = source_intact;
                        Some(garment)
                    }
                }
            };
            let Some(mut loaded) = loaded else {
                // CLO can still import independently stored avatars when the
                // garment envelope is missing or unreadable.
                let avatars = crate::compat_fit::project_avatar(path).unwrap_or_default();
                if !append {
                    replace_scene(state, empty_project());
                    set_project(state, path);
                }
                import_project_avatar(state, avatars, append);
                return Ok(Some(open_reply(state, path)));
            };
            let prior_count = array(state, "patterns").len();
            let composition = if intact && append && prior_count != 0 {
                formats::native_save::append_sources(state, path, is_project)
            } else {
                None
            };
            let composition = composition.filter(|_| !array(state, "colorways").is_empty()
                && !array(&loaded, "colorways").is_empty());
            let empty_receiver = intact && append
                && prior_count == 0
                && array(state, "imported_meshes").is_empty()
                && state["colorways"] == json!([DEFAULT_COLORWAY])
                && !array(&loaded, "fabrics").is_empty();
            let fabrics = prepare_archived_fabrics(state, &mut loaded, append)?;
            let old_pose = append.then(|| state["simulation"].clone());
            merge(state, &loaded, append)?;
            if append {
                vec_mut(state, "fabrics").extend(fabrics);
                state["current_fabric"] = json!(0);
            } else {
                state["fabrics"] = json!(fabrics);
                state["current_fabric"] = json!(0);
                if let Some(runtime) = state["runtime"].as_object_mut() {
                    runtime.remove("last_added_fabric");
                }
                state["colorways"] = loaded["colorways"]
                    .as_array()
                    .map_or_else(|| json!([DEFAULT_COLORWAY]), |names| json!(names));
                state["current_colorway"] = json!(loaded["current_colorway"].as_u64().unwrap_or(0));
            }
            let mut avatar_error = None;
            let mut embedded_avatar = false;
            if is_project && load_avatar {
                match crate::compat_fit::project_avatar(path) {
                    Ok(avatar) => {
                        embedded_avatar = !avatar.is_empty();
                        import_project_avatar(state, avatar, append);
                    }
                    Err(error) => {
                        import_project_avatar(state, Vec::new(), append);
                        avatar_error = Some(error);
                    }
                }
                if !append {
                    set_project(state, path);
                }
            } else if !is_project && !append {
                state["project"] = json!("NULL");
                state["project_path"] = json!("NULL");
            }
            let pose = array(&loaded, "saved_pose");
            if append {
                if let Some(pose) =
                    append_native_pose(old_pose, prior_count, array(state, "patterns"), &loaded)
                {
                    state["simulation"] = pose;
                }
            } else if !pose.is_empty() {
                state["simulation"] = json!({"source": "native_saved_pose", "meshes": pose});
            }
            if intact && load_avatar && !append {
                formats::native_save::remember(state, path, is_project);
            } else if empty_receiver && !array(state, "patterns").is_empty() {
                formats::native_save::remember_empty_append(state, path, is_project);
            } else if let Some(parts) = composition {
                formats::native_save::remember_composition(state, parts);
            }
            let mut reply = open_reply(state, path);
            reply["partial"] = json!(true);
            reply["native_import"] = loaded["native_import"].clone();
            reply["native_import"]["embedded_avatar_loaded"] = json!(embedded_avatar);
            reply["native_import"]["saved_pose_loaded"] = json!(!pose.is_empty() && !state["simulation"].is_null());
            let mut warnings = vec![
                "Loaded native outlines from saved mesh samples. Verified curve metrics, child shapes and internal seam paths are available for supported records.".to_owned(),
                "Saved arrangement settings may be present. Official placement calibration and solver continuation state are unavailable.".to_owned(),
                "Full single outline-edge seams were restored where verified. Other recovered intervals remain in the portable project data.".to_owned(),
            ];
            if embedded_avatar {
                warnings.push("The embedded avatar was loaded as static rest geometry. Native skinning, pose and textures were not recovered.".into());
            }
            if let Some(error) = avatar_error {
                warnings.push(format!("The embedded avatar could not be imported: {error}"));
            }
            if array(&loaded, "fabrics").is_empty() {
                warnings.push("Archived fabric assignments and front colors could not be validated for this file.".into());
            }
            if append && !pose.is_empty() && state["simulation"]["source"] == "mixed_saved_pose" {
                warnings.push("The combined scene retains saved poses. Panels without a pose are initialized from the pattern outlines.".into());
            } else if pose.is_empty() {
                warnings.push("The saved 3D pose could not be validated for this file.".into());
            }
            warnings.extend(array(&loaded, "native_texture_warnings").iter().filter_map(Value::as_str).map(str::to_owned));
            reply["warnings"] = json!(warnings);
            return Ok(Some(reply));
        }
        "avt" => {
            crate::compat_fit::dispatch(
                "avatar_load",
                &json!({"name_or_path": path, "replace": false}),
                state,
            )
            .ok_or("Rust avatar loader unavailable")??;
            let mut reply = open_reply(state, path);
            reply["partial"] = json!(true);
            reply["rest_geometry"] = state["avatar"]["rest_geometry"].clone();
            reply["warnings"] = state["avatar"]["warnings"].clone();
            return Ok(Some(reply));
        }
        ext => return Err(format!("unsupported import extension {ext:?}")),
    }
    Ok(Some(open_reply(state, path)))
}

fn scene_document(state: &Value) -> Result<Value, String> {
    let _ = validate_garment(state)?;
    let mut project = state.clone();
    if let Some(object) = project.as_object_mut() {
        object.remove("runtime");
        object.remove(formats::native_save::SOURCE);
    }
    Ok(json!({"format": SCENE_FORMAT, "scene": project}))
}

fn saved(path: &Path, state: &Value) -> Result<(), String> {
    formats::write_json(path, &scene_document(state)?)
}

pub(super) fn save(args: &Value, state: &mut Value) -> Reply {
    let path = Path::new(string(args, "path")?);
    match extension(path).as_str() {
        "json" | "clorust" => saved(path, state)?,
        ext @ ("zprj" | "zpac") => {
            formats::native_save::save(path, state, ext == "zprj", flag(args, "thumbnail", true))?;
            state["runtime"]["gltf_export_active_only"] = json!(true);
            if ext == "zpac" {
                return Ok(json!({"path": path}));
            }
        }
        ext => return Err(format!("unsupported project extension {ext:?}")),
    }
    set_project(state, path);
    Ok(json!({"path": path}))
}

pub(super) fn export(args: &Value, state: &mut Value) -> Reply {
    let path = Path::new(string(args, "path")?);
    let ext = extension(path);
    match ext.as_str() {
        "clorust" => return save(args, state),
        "zprj" | "zpac" => return save(&json!({"path": path, "thumbnail": false}), state),
        "json" => {
            let (patterns, seams) = validate_garment(state)?;
            formats::write_json(
                path,
                &json!({"format": PATTERN_FORMAT, "units": "mm", "patterns": patterns, "seams": seams, "native_seam_groups": array(state, "native_seam_groups")}),
            )?;
        }
        "dxf" => formats::write_dxf(path, array(state, "patterns"))?,
        "obj" | "fbx" | "glb" | "gltf" => {
            let result = formats::write_mesh(
                path, &ext, state, flag(args, "include_avatar", false), flag(args, "colorways", false),
            )?;
            return Ok(json!({"path": path, "result": result}));
        }
        _ => return Err(format!("unsupported export extension {ext:?}")),
    }
    let result = match ext.as_str() {
        "json" => json!(true),
        _ => json!(path),
    };
    Ok(json!({"path": path, "result": result}))
}

fn checkpoint_dir() -> PathBuf {
    if let Some(path) = env::var_os("CLO_RUST_CHECKPOINT_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("CLO_MCP_STATE") {
        return PathBuf::from(path).join("checkpoints");
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("clo-rust").join("checkpoints");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join(".clo-rust")
        .join("checkpoints")
}

fn checkpoint_path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.len() > 180
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err("invalid checkpoint name; use a filename without directories".into());
    }
    Ok(checkpoint_dir().join(format!("{name}.clorust")))
}

fn calendar(time: SystemTime) -> [u64; 6] {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    #[cfg(any(unix, windows))]
    if let Some(local) = local_calendar(seconds) {
        return local;
    }
    let day = seconds / 86400 + 719468;
    let era = day / 146097;
    let doe = day % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    [
        yoe + era * 400 + u64::from(month <= 2),
        month,
        doy - (153 * mp + 2) / 5 + 1,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    ]
}

#[cfg(any(unix, windows))]
fn local_calendar(seconds: u64) -> Option<[u64; 6]> {
    let time = libc::time_t::try_from(seconds).ok()?;
    let mut result = std::mem::MaybeUninit::<libc::tm>::uninit();
    // Both system calls fill the caller-owned buffer when they succeed.
    #[cfg(unix)]
    let ok = unsafe { !libc::localtime_r(&time, result.as_mut_ptr()).is_null() };
    #[cfg(windows)]
    let ok = unsafe { libc::localtime_s(result.as_mut_ptr(), &time) == 0 };
    if !ok {
        return None;
    }
    let result = unsafe { result.assume_init() };
    let fields = [
        result.tm_year.checked_add(1900)?,
        result.tm_mon + 1,
        result.tm_mday,
        result.tm_hour,
        result.tm_min,
        result.tm_sec,
    ];
    fields.iter().all(|&value| value >= 0).then(|| fields.map(|value| value as u64))
}

pub(super) fn checkpoint_save(args: &Value, state: &mut Value) -> Reply {
    let result = save_checkpoint(state, args["name"].as_str().filter(|name| !name.is_empty()), "cp")?;
    let path = Path::new(string(&result, "path")?);
    if extension(path) == "zprj" {
        formats::native_save::remember_project_save(state, path);
        state["runtime"]["gltf_export_active_only"] = json!(true);
    }
    set_project(state, path);
    Ok(result)
}

pub(crate) fn checkpoint_before_build(state: &mut Value) -> Reply {
    let result = save_checkpoint(state, None, "auto_before_build")?;
    set_project(state, Path::new(string(&result, "path")?));
    Ok(result)
}

fn save_checkpoint(state: &Value, name: Option<&str>, prefix: &str) -> Reply {
    let started = SystemTime::now();
    let named = name.map(checkpoint_path).transpose()?;
    let shared = env::var_os("CLO_MCP_STATE").is_some()
        && env::var_os("CLO_RUST_CHECKPOINT_DIR").is_none();
    // Portable snapshots cover scenes that cannot be written as a native project.
    let native = if shared
        && named.as_ref().is_none_or(|path| !path.exists())
    {
        formats::native_save::encode(state, true, false).ok()
    } else {
        None
    };
    if let (Some(name), Some(portable), Some(bytes)) = (name, named.as_ref(), native.as_ref()) {
        let path = portable.with_extension("zprj");
        formats::write(&path, bytes)?;
        return Ok(json!({"name": name, "path": path}));
    }
    let (bytes, extension) = if let (None, Some(bytes)) = (name, native) {
        (bytes, "zprj")
    } else {
        let mut document = scene_document(state)?;
        if let Some(source) = state.get(formats::native_save::SOURCE).filter(|v| v.is_object()) {
            // Preserve the source and edit records so an undo can still be exported as native.
            document[formats::native_save::SOURCE] = source.clone();
        }
        if let (Some(name), Some(path)) = (name, named) {
            formats::write_json(&path, &document)?;
            return Ok(json!({"name": name, "path": path}));
        }
        (serde_json::to_vec_pretty(&document).map_err(|err| err.to_string())?, "clorust")
    };
    let [year, month, day, hour, min, sec] = calendar(started);
    let base = format!("{prefix}_{year:04}{month:02}{day:02}_{hour:02}{min:02}{sec:02}");
    for suffix in 0u64.. {
        let name = if suffix == 0 { base.clone() } else { format!("{base}_{suffix}") };
        let portable = checkpoint_path(&name)?;
        let native = portable.with_extension("zprj");
        if portable.exists() || native.exists() {
            continue;
        }
        let path = if extension == "zprj" { native } else { portable };
        if formats::write_new(&path, &bytes)? {
            return Ok(json!({"name": name, "path": path}));
        }
    }
    unreachable!("checkpoint suffix range exhausted")
}

pub(super) fn checkpoint_restore(args: &Value, state: &mut Value) -> Reply {
    let name = string(args, "name")?;
    let path = checkpoint_path(name)?;
    if !path.is_file() {
        let native = path.with_extension("zprj");
        if native.is_file() {
            open(&json!({"path": native, "mode": "replace"}), state)?;
            return Ok(open_reply(state, &native));
        }
        return Err(format!("no checkpoint {name}; see checkpoint_list"));
    }
    let document = formats::read_json(&path)?;
    let source = document.get(formats::native_save::SOURCE).filter(|v| v.is_object()).cloned();
    let mut restored = recognized_state(document)?;
    if let Some(source) = source {
        restored[formats::native_save::SOURCE] = source;
    }
    set_project(&mut restored, &path);
    restored["current_fabric"] = json!(0);
    replace_scene(state, restored);
    crate::compat_runtime::clear_scene_ui(state, true);
    Ok(open_reply(state, &path))
}

pub(super) fn checkpoint_list(_: &Value, _: &mut Value) -> Reply {
    let mut files = Vec::new();
    let entries = match fs::read_dir(checkpoint_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"checkpoints": []}))
        }
        Err(error) => return Err(format!("read checkpoint directory: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        let native = path.extension().and_then(|ext| ext.to_str()) == Some("zprj");
        if !native && extension(&path) != "clorust" {
            continue;
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() || (native && path.with_extension("clorust").is_file()) {
            continue;
        }
        let time = metadata.modified().map_err(|err| err.to_string())?;
        files.push((path, time));
    }
    // The reference sorts by floating-point mtime and retains the directory order for ties.
    let seconds = |time: SystemTime| match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64(),
        Err(error) => -error.duration().as_secs_f64(),
    };
    files.sort_by(|a, b| seconds(b.1).total_cmp(&seconds(a.1)));
    let rows: Vec<_> = files.into_iter().map(|(path, time)| {
        let [year, month, day, hour, min, _] = calendar(time);
        json!({"name": path.file_stem().and_then(|name| name.to_str()), "path": path, "mtime": format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")})
    }).collect();
    Ok(json!({"checkpoints": rows}))
}
