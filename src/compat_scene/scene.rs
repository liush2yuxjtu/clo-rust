use super::{array, flag, formats, invalidate_geometry, patterns, string, vec_mut, Reply};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCENE_FORMAT: &str = "clo-rust.scene.v1";
const PATTERN_FORMAT: &str = "clo-rust.pattern.v1";

fn avatar_names(state: &Value) -> Vec<&str> {
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

pub(super) fn info(_: &Value, state: &mut Value) -> Reply {
    let pieces: Vec<_> = array(state, "patterns")
        .iter()
        .enumerate()
        .map(|(i, p)| patterns::summary(p, i))
        .collect();
    let fabrics: Vec<_> = array(state, "fabrics")
        .iter()
        .enumerate()
        .map(|(i, f)| json!({"index": f["index"].as_u64().unwrap_or(i as u64), "name": f["name"]}))
        .collect();
    Ok(json!({
        "project": state["project"].as_str().unwrap_or("Untitled"),
        "project_path": state["project_path"].as_str().unwrap_or(""),
        "clo_version": format!("clo-rust/{}", env!("CARGO_PKG_VERSION")),
        "pattern_count": pieces.len(), "patterns": pieces,
        "avatars": avatar_names(state), "fabric_count": fabrics.len(), "fabrics": fabrics,
        "colorways": state.get("colorways").cloned().unwrap_or_else(|| json!(["Colorway 1"])),
        "current_colorway": state["current_colorway"].as_u64().unwrap_or(0),
        "seam_groups": super::annotations::count(state),
        "temp_folder": env::temp_dir(),
        "asset_folder": env::var_os("CLO_RUST_ASSET_DIR").map(PathBuf::from)
    }))
}

fn clear_garment(state: &mut Value) {
    for field in ["patterns", "seams", "native_seam_groups", "imported_meshes"] {
        state[field] = json!([]);
    }
    state["garment_spec"] = Value::Null;
    state.as_object_mut().expect("scene object").remove(formats::native_save::SOURCE);
    invalidate_geometry(state);
}

pub(super) fn clear(args: &Value, state: &mut Value) -> Reply {
    let count = array(state, "patterns").len();
    clear_garment(state);
    if flag(args, "keep_avatar", true) {
        return Ok(json!({"deleted": count, "pattern_count": 0}));
    }
    state["avatar"] = Value::Null;
    state["additional_avatars"] = json!([]);
    state["fabrics"] = json!([]);
    state["colorways"] = json!(["Colorway 1"]);
    state["current_colorway"] = json!(0);
    state["project_path"] = json!("");
    state["project"] = json!("Untitled");
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
    let (groups, group_ids) = super::annotations::remap(state, incoming, remap_id)?;
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
    vec_mut(state, "native_seam_groups").extend(groups);
    vec_mut(state, "patterns").extend(additions);
    vec_mut(state, "seams").extend(joins);
    invalidate_geometry(state);
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

pub(super) fn open(args: &Value, state: &mut Value) -> Reply {
    let path = Path::new(string(args, "path")?);
    if !path.is_file() {
        return Err(format!("file not found: {}", path.display()));
    }
    let append = match args["mode"].as_str().unwrap_or("replace") {
        "replace" => false,
        "append" => true,
        mode => return Err(format!("unknown scene open mode {mode:?}")),
    };
    match extension(path).as_str() {
        "json" | "clorust" => {
            let content = formats::read_json(path)?;
            match content["format"].as_str() {
                Some(SCENE_FORMAT) => {
                    let loaded = recognized_state(content)?;
                    if append {
                        merge(state, &loaded, true)?;
                    } else {
                        replace_scene(state, loaded);
                        set_project(state, path);
                    }
                }
                Some(PATTERN_FORMAT) => merge(state, &content, append)?,
                _ if content.get("pieces").is_some() => {
                    let converted = formats::pattern_native(&content)?;
                    merge(state, &converted, append)?;
                }
                _ => return Err("unknown JSON scene or pattern format".into()),
            }
        }
        "dxf" => merge(state, &formats::read_dxf(path)?, append)?,
        ext @ ("obj" | "fbx") => {
            let mut imported = formats::read_mesh_scene(path, ext)?;
            let fabrics = prepare_archived_fabrics(state, &mut imported, true)?;
            if !append {
                clear_garment(state);
            }
            vec_mut(state, "fabrics").extend(fabrics);
            vec_mut(state, "imported_meshes").append(vec_mut(&mut imported, "imported_meshes"));
            let mut reply = open_reply(state, path);
            if !array(&imported, "warnings").is_empty() {
                reply["warnings"] = imported["warnings"].clone();
            }
            return Ok(reply);
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
            let mut loaded = formats::clo_native::read(path, is_project)?;
            let fabrics = prepare_archived_fabrics(state, &mut loaded, append)?;
            merge(state, &loaded, append)?;
            if append {
                vec_mut(state, "fabrics").extend(fabrics);
            } else {
                state["fabrics"] = json!(fabrics);
            }
            let mut avatar_error = None;
            let mut embedded_avatar = false;
            if is_project && !append {
                state["additional_avatars"] = json!([]);
                state["avatar"] = match crate::compat_fit::project_avatar(path) {
                    Ok(Some(avatar)) => {
                        embedded_avatar = true;
                        avatar
                    }
                    Ok(None) => Value::Null,
                    Err(error) => {
                        avatar_error = Some(error);
                        Value::Null
                    }
                };
                state["colorways"] = json!(["Colorway 1"]);
                state["current_colorway"] = json!(0);
                set_project(state, path);
            }
            let pose = array(&loaded, "saved_pose");
            if !append && !pose.is_empty() {
                state["simulation"] = json!({"source": "native_saved_pose", "meshes": pose});
            }
            if !append {
                formats::native_save::remember(state, path, is_project);
            }
            let mut reply = open_reply(state, path);
            reply["partial"] = json!(true);
            reply["native_import"] = loaded["native_import"].clone();
            reply["native_import"]["embedded_avatar_loaded"] = json!(embedded_avatar);
            let mut warnings = vec![
                "Loaded native panel outlines using saved mesh samples. Authored curve controls and internal lines were not recovered.".to_owned(),
                "Cloth calibration, panel arrangement and official solver continuation state were not imported.".to_owned(),
                "Full single outline-edge seams were restored where verified. Other recovered intervals remain recorded in seams_list.".to_owned(),
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
            if append && !pose.is_empty() {
                warnings.push("The saved 3D pose is loaded in replace mode only.".into());
            } else if pose.is_empty() {
                warnings.push("The saved 3D pose could not be validated for this file.".into());
            }
            warnings.extend(array(&loaded, "native_texture_warnings").iter().filter_map(Value::as_str).map(str::to_owned));
            reply["warnings"] = json!(warnings);
            return Ok(reply);
        }
        "avt" => {
            let saved_pose = (state["simulation"]["source"] == "native_saved_pose")
                .then(|| state["simulation"].clone());
            let loaded = crate::compat_fit::dispatch(
                "avatar_load",
                &json!({"name_or_path": path, "replace": false}),
                state,
            )
            .ok_or("Rust avatar loader unavailable")??;
            if let Some(pose) = saved_pose {
                state["simulation"] = pose;
            }
            let mut reply = open_reply(state, path);
            reply["partial"] = json!(true);
            reply["rest_geometry"] = loaded["rest_geometry"].clone();
            reply["warnings"] = loaded["warnings"].clone();
            return Ok(reply);
        }
        ext => return Err(format!("unsupported import extension {ext:?}")),
    }
    Ok(open_reply(state, path))
}

fn saved(path: &Path, state: &Value) -> Result<(), String> {
    let _ = validate_garment(state)?;
    let mut project = state.clone();
    if let Some(object) = project.as_object_mut() {
        object.remove("runtime");
        object.remove(formats::native_save::SOURCE);
    }
    formats::write_json(path, &json!({"format": SCENE_FORMAT, "scene": project}))
}

pub(super) fn save(args: &Value, state: &mut Value) -> Reply {
    let path = Path::new(string(args, "path")?);
    match extension(path).as_str() {
        "json" | "clorust" => saved(path, state)?,
        ext @ ("zprj" | "zpac") => {
            formats::native_save::save(path, state, ext == "zprj", flag(args, "thumbnail", true))?;
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
    if flag(args, "colorways", false)
        && !matches!(ext.as_str(), "zprj" | "zpac")
        && (!matches!(ext.as_str(), "obj" | "fbx" | "glb" | "gltf") || array(state, "colorways").len() > 1)
    {
        return Err("exporting colorways is supported for a single OBJ, FBX, GLB or glTF colorway".into());
    }
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
            let result = formats::write_mesh(path, &ext, state, flag(args, "include_avatar", false))?;
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

pub(super) fn checkpoint_save(args: &Value, state: &mut Value) -> Reply {
    let name = args["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let [year, month, day, hour, min, sec] = calendar(SystemTime::now());
            format!("cp_{year:04}{month:02}{day:02}_{hour:02}{min:02}{sec:02}")
        });
    let path = checkpoint_path(&name)?;
    saved(&path, state)?;
    Ok(json!({"name": name, "path": path}))
}

pub(super) fn checkpoint_restore(args: &Value, state: &mut Value) -> Reply {
    let name = string(args, "name")?;
    let path = checkpoint_path(name)?;
    if !path.is_file() {
        return Err(format!("no checkpoint {name}; see checkpoint_list"));
    }
    let restored = recognized_state(formats::read_json(&path)?)?;
    replace_scene(state, restored);
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
        if extension(&entry.path()) != "clorust"
            || !entry.file_type().map_err(|err| err.to_string())?.is_file()
        {
            continue;
        }
        let time = entry
            .metadata()
            .and_then(|m| m.modified())
            .map_err(|err| err.to_string())?;
        files.push((entry.path(), time));
    }
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let rows: Vec<_> = files.into_iter().map(|(path, time)| {
        let [year, month, day, hour, min, _] = calendar(time);
        json!({"name": path.file_stem().and_then(|name| name.to_str()), "path": path, "mtime": format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")})
    }).collect();
    Ok(json!({"checkpoints": rows}))
}
