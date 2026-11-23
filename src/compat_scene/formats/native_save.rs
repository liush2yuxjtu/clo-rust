//! Preserve proprietary scene payloads while updating verified editable fields.

use super::{
    clo_native::{avatar_members, avatar_payload, bounded_file, member, open_archive, optional_member, restore_zpac_footer, ARCHIVE_LIMIT, AVT, ZPAC, ZPRJ},
    native_pose::{encode_map, encoded_list, map_list, replace_positions, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE},
    native_avatar_ids::AvatarIdentities, native_author, native_compose, native_elastic, native_materials, native_recolor, native_textures, write,
};
use serde_json::{json, Value};
use native_textures::{ColorwayTextureEdit, TextureAssets};
use std::borrow::Cow;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

pub(in crate::compat_scene) const SOURCE: &str = "native_archive_source";

pub(in crate::compat_scene) fn stamp(path: &Path) -> Result<Value, String> {
    let meta = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !meta.is_file() || meta.len() > ARCHIVE_LIMIT {
        return Err("native source must be a file no larger than 512 MiB".into());
    }
    let time = meta
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?;
    Ok(json!([meta.len(), time.as_secs(), time.subsec_nanos()]))
}

fn hash_except(value: &Value, omitted: &[&str], hash: &mut DefaultHasher) {
    if let Some(fields) = value.as_object() {
        for (key, value) in fields
            .iter()
            .filter(|(key, _)| !omitted.contains(&key.as_str()))
        {
            key.hash(hash);
            value.hash(hash);
        }
    } else {
        value.hash(hash);
    }
}

fn hash_garment_layout(state: &Value, hash: &mut DefaultHasher, ignore_elastic: bool) {
    for pattern in super::super::array(state, "patterns") {
        if !ignore_elastic {
            hash_except(pattern, &["name"], hash);
            continue;
        }
        hash_except(pattern, &["name", "line_properties"], hash);
        for (index, record) in native_elastic::non_elastic(&pattern["line_properties"]) {
            index.hash(hash);
            hash_except(record, &["elastic"], hash);
        }
    }
    for key in [
        "seams",
        "native_seam_groups",
        "garment_spec",
        "colorways",
        "imported_meshes",
    ] {
        state[key].hash(hash);
    }
}

fn layout_fingerprint(state: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    hash_garment_layout(state, &mut hash, false);
    hash.finish()
}

fn garment_fingerprint_inner(state: &Value, ignore_elastic: bool, pose: bool) -> u64 {
    let mut hash = DefaultHasher::new();
    hash_garment_layout(state, &mut hash, ignore_elastic);
    if pose {
        let simulation = &state["simulation"];
        hash_except(simulation, &["meshes"], &mut hash);
        for mesh in super::super::array(simulation, "meshes") {
            hash_except(mesh, &["name"], &mut hash);
        }
    }
    hash.finish()
}

fn garment_fingerprint(state: &Value) -> u64 {
    garment_fingerprint_inner(state, false, true)
}

fn fabric_fingerprint(state: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    state["current_colorway"].hash(&mut hash);
    for fabric in super::super::array(state, "fabrics") {
        hash_editable_material(fabric, false, true, &mut hash);
    }
    hash.finish()
}

fn assigned_fabric_fingerprint(state: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    state["current_colorway"].hash(&mut hash);
    for fabric in super::super::assigned_fabrics(state) {
        fabric.hash(&mut hash);
    }
    hash.finish()
}

fn has_material_edits(source: &Value) -> bool {
    ["recolored", "retextured", "colorway_recolors", "colorway_textures"]
        .iter()
        .any(|key| !super::super::array(source, key).is_empty())
}

fn hash_editable_material(
    fabric: &Value, colorway: bool, texture_edit: bool, hash: &mut DefaultHasher,
) {
    let mut omitted = vec!["color", "rgba", "texture_maps", "base_texture_desaturated", "info"];
    if colorway {
        omitted.push("native_colorway_materials");
    }
    if !texture_edit {
        let opacity = !crate::compat_fit::preserves_opacity_on_recolor(fabric);
        let clearable_maps: &[&str] = if opacity { &["opacity"] } else { &[] };
        hash_except(&fabric["texture_maps"], clearable_maps, hash);
    }
    hash_except(fabric, &omitted, hash);
    hash_except(&fabric["info"], &["catalog", "face_colors"], hash);
    for face in super::super::array(&fabric["info"], "face_colors") {
        if matches!(face["type"].as_u64(), Some(0..=2)) {
            hash_except(face, &["rgba"], hash);
        } else {
            face.hash(hash);
        }
    }
    let editable_catalog: &[&str] = if texture_edit { &["Color", "Texture"] } else { &["Color"] };
    hash_except(&fabric["info"]["catalog"], editable_catalog, hash);
}

fn colorway_fingerprints(state: &Value) -> Option<Vec<Vec<Value>>> {
    let names = state["colorways"].as_array()?;
    let active = usize::try_from(state["current_colorway"].as_u64()?).ok()?;
    if names.len() < 2 || active >= names.len() {
        return None;
    }
    super::super::array(state, "fabrics").iter().map(|fabric| {
        let variants = &fabric["native_colorway_materials"];
        let values = variants["values"].as_array()?;
        if variants["names"] != state["colorways"] || values.len() != names.len() {
            return None;
        }
        Some(values.iter().enumerate().map(|(index, material)| {
            // The current material carries live edits. Its archived slot may be stale.
            let material = if index == active { fabric } else { material };
            let mut exact = DefaultHasher::new();
            hash_except(material, &["native_colorway_materials"], &mut exact);
            let mut recolorable = DefaultHasher::new();
            hash_editable_material(material, true, false, &mut recolorable);
            let mut texturable = DefaultHasher::new();
            hash_except(material, &["native_colorway_materials", "texture_maps",
                "base_texture_desaturated", "info"], &mut texturable);
            hash_except(&material["info"], &["catalog"], &mut texturable);
            hash_except(&material["info"]["catalog"], &["Texture"], &mut texturable);
            let mut both = DefaultHasher::new();
            hash_editable_material(material, true, true, &mut both);
            json!({
                "hash": [exact.finish(), recolorable.finish(), texturable.finish(), both.finish()],
                "textures": material["texture_maps"],
                "sheer": crate::compat_fit::preserves_opacity_on_recolor(material),
                "linked_colorway": super::super::canonical_colorway(fabric, index),
            })
        }).collect())
    }).collect()
}

fn verify_colorway_materials(
    state: &Value, source: &Value, recolors: &HashSet<[usize; 2]>,
    texture_edits: &[ColorwayTextureEdit],
) -> bool {
    let Some(observed) = colorway_fingerprints(state) else { return false };
    let Some(expected) = source["colorways"].as_array() else { return false };
    if observed.len() != expected.len() {
        return false;
    }
    let mut texture_maps: HashMap<[usize; 2], Vec<&str>> = HashMap::new();
    for edit in texture_edits {
        texture_maps.entry([edit.colorway, edit.fabric]).or_default().push(&edit.map);
    }
    observed.iter().enumerate().all(|(fabric, variants)| {
        variants.iter().enumerate().all(|(colorway, material)| {
            let before = &expected[fabric][colorway];
            if before["linked_colorway"] != material["linked_colorway"] {
                return false;
            }
            let target = [material["linked_colorway"].as_u64().unwrap_or(colorway as u64) as usize, fabric];
            let recolored = recolors.contains(&target);
            let maps = texture_maps.get(&target);
            let column = usize::from(recolored) + 2 * usize::from(maps.is_some());
            if before["hash"][column] != material["hash"][column] {
                return false;
            }
            maps.is_none_or(|maps| {
                let mut ignored = maps.clone();
                if recolored && before["sheer"] == false {
                    ignored.push("opacity");
                }
                let hash_maps = |value: &Value| {
                    let mut hash = DefaultHasher::new();
                    hash_except(&value["textures"], &ignored, &mut hash);
                    hash.finish()
                };
                hash_maps(before) == hash_maps(material)
            })
        })
    })
}

fn avatar_fingerprint(state: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    state["avatar"].hash(&mut hash);
    state["additional_avatars"].hash(&mut hash);
    hash.finish()
}

pub(in crate::compat_scene) fn remember(state: &mut Value, path: &Path, project: bool, recovered_zpac: bool) {
    let value = path
        .canonicalize()
        .ok()
        .and_then(|path| {
            Some(json!({
                "stamp": stamp(&path).ok()?,
                "path": path.to_str()?,
                "project": project,
                "garment": garment_fingerprint(state),
                "layout": layout_fingerprint(state),
                "garment_before_elastic": garment_fingerprint_inner(state, true, true),
                "layout_before_elastic": garment_fingerprint_inner(state, true, false),
                "fabrics": fabric_fingerprint(state),
                "assigned_fabrics": assigned_fabric_fingerprint(state),
                "colorways": colorway_fingerprints(state),
                "colorway_recolors": [],
                "colorway_textures": [],
                "current_colorway": state["current_colorway"],
                "avatars": avatar_fingerprint(state),
                "textures": super::super::array(state, "fabrics").iter().map(|fabric| &fabric["texture_maps"]).collect::<Vec<_>>(),
            }))
        })
        .unwrap_or(Value::Null);
    state[SOURCE] = value;
    if recovered_zpac && state[SOURCE].is_object() {
        state[SOURCE]["recovered_zpac"] = json!(true);
    }
}

pub(in crate::compat_scene) fn remember_empty_append(state: &mut Value, path: &Path, project: bool, recovered_zpac: bool) {
    remember(state, path, project, recovered_zpac);
    if state[SOURCE].is_object() {
        state[SOURCE]["empty_append"] = json!(true);
    }
}

pub(in crate::compat_scene) fn retained_after_deletion(state: &Value, deleted: &[usize]) -> Option<Vec<usize>> {
    let source = &state[SOURCE];
    if deleted.is_empty() || author_scene(source) || source["empty_append"] == true
        || !super::super::array(source, "appended").is_empty()
        || source["garment"].as_u64() != Some(garment_fingerprint(state))
    {
        return None;
    }
    let count = super::super::array(state, "patterns").len();
    let retained = if let Some(saved) = source.get("retained_panels") {
        serde_json::from_value::<Vec<usize>>(saved.clone()).ok()?
    } else {
        (0..count).collect()
    };
    if retained.len() != count {
        return None;
    }
    Some(retained.into_iter().enumerate()
        .filter_map(|(index, slot)| deleted.binary_search(&index).is_err().then_some(slot))
        .collect())
}

pub(in crate::compat_scene) fn remember_panel_deletion(state: &mut Value, retained: Vec<usize>) {
    state[SOURCE]["garment"] = json!(garment_fingerprint(state));
    state[SOURCE]["layout"] = json!(layout_fingerprint(state));
    state[SOURCE]["garment_before_elastic"] = json!(garment_fingerprint_inner(state, true, true));
    state[SOURCE]["layout_before_elastic"] = json!(garment_fingerprint_inner(state, true, false));
    state[SOURCE]["assigned_fabrics"] = json!(assigned_fabric_fingerprint(state));
    state[SOURCE]["retained_panels"] = json!(retained);
}

fn drafted_append(state: &Value) -> Result<Option<usize>, String> {
    let Some(draft) = state[SOURCE].get("draft_append") else { return Ok(None) };
    let start = draft["start"].as_u64().and_then(|v| usize::try_from(v).ok())
        .filter(|&start| start > 0 && start < super::super::array(state, "patterns").len());
    if draft["layout"].as_u64() != Some(layout_fingerprint(state))
        || draft["fabrics"].as_u64() != Some(fabric_fingerprint(state))
        || draft["avatars"].as_u64() != Some(avatar_fingerprint(state))
    {
        return Err("native save requires the unchanged appended draft".into());
    }
    start.map(Some).ok_or_else(|| "invalid native draft append start".into())
}

pub(crate) fn draft_append_start(state: &Value) -> Option<usize> {
    let source = &state[SOURCE];
    if !source.is_object() || author_scene(source) || source["empty_append"] == true {
        return None;
    }
    if let Some(start) = drafted_append(state).ok()? {
        return Some(start);
    }
    if source["garment"].as_u64() != Some(garment_fingerprint(state))
        || source["fabrics"].as_u64() != Some(fabric_fingerprint(state))
        || has_material_edits(source)
    {
        return None;
    }
    let count = super::super::array(state, "patterns").len();
    (count > 0).then_some(count)
}

pub(crate) fn remember_draft_append(state: &mut Value, start: usize) {
    let has_pose = state[SOURCE]["draft_append"]["has_pose"] == true
        || !super::super::array(&state["simulation"], "meshes").is_empty();
    state[SOURCE]["draft_append"] = json!({"start": start, "layout": layout_fingerprint(state),
        "fabrics": fabric_fingerprint(state), "avatars": avatar_fingerprint(state), "has_pose": has_pose});
}

pub(in crate::compat_scene) fn append_sources(state: &Value, path: &Path, project: bool, recovered_zpac: bool) -> Option<Vec<Value>> {
    let source = &state[SOURCE];
    if source["garment"].as_u64()? != garment_fingerprint(state)
        || (source["fabrics"].as_u64()? != fabric_fingerprint(state)
            && source["assigned_fabrics"].as_u64() != Some(assigned_fabric_fingerprint(state)))
        || has_material_edits(source)
    {
        return None;
    }
    let path = path.canonicalize().ok()?;
    let mut parts = super::super::array(source, "appended").to_vec();
    if parts.len() >= 16 {
        return None;
    }
    parts.push(json!({"path": path.to_str()?, "stamp": stamp(&path).ok()?, "project": project,
                      "recovered_zpac": recovered_zpac}));
    Some(parts)
}

pub(in crate::compat_scene) fn remember_composition(state: &mut Value, parts: Vec<Value>) {
    let Some(incoming) = parts.last() else {
        return;
    };
    let stable = incoming["path"].as_str().map(Path::new)
        .and_then(|path| stamp(path).ok()).is_some_and(|value| value == incoming["stamp"]);
    if !stable {
        return;
    }
    let source = &state[SOURCE];
    let path = source["path"].as_str().map(std::path::PathBuf::from);
    let project = source["project"] == true;
    let old_stamp = source["stamp"].clone();
    let recovered_zpac = source["recovered_zpac"] == true;
    let retained_panels = source.get("retained_panels").cloned();
    let archived_avatars = source["avatars"].clone();
    let empty_append = source["empty_append"] == true;
    let compacted_fabrics = source["compacted_fabrics"] == true;
    if let Some(path) = path {
        remember(state, &path, project, recovered_zpac);
        if state[SOURCE].is_object() {
            state[SOURCE]["stamp"] = old_stamp;
            state[SOURCE]["avatars"] = archived_avatars;
            state[SOURCE]["appended"] = json!(parts);
            state[SOURCE]["empty_append"] = json!(empty_append);
            state[SOURCE]["compacted_fabrics"] = json!(compacted_fabrics);
            if let Some(retained) = retained_panels {
                state[SOURCE]["retained_panels"] = retained;
            }
        }
    }
}

fn current_material_colorway(state: &Value, index: u64) -> usize {
    let colorway = state["current_colorway"].as_u64().unwrap_or(0) as usize;
    super::super::array(state, "fabrics").iter().find(|fabric| fabric["index"] == index)
        .map_or(colorway, |fabric| super::super::canonical_colorway(fabric, colorway))
}

pub(in crate::compat_scene) fn mark_color(state: &mut Value, index: u64, clear_opacity: bool) {
    let colorway = current_material_colorway(state, index);
    let multiple = super::super::array(state, "colorways").len() > 1;
    if let Some(source) = state.get_mut(SOURCE).filter(|source| source.is_object()) {
        if multiple {
            let targets = super::super::vec_mut(source, "colorway_recolors");
            let target = json!([colorway, index]);
            if !targets.contains(&target) {
                targets.push(target);
            }
            if clear_opacity {
                if let Some(edits) = source["colorway_textures"].as_array_mut() {
                    edits.retain(|edit| edit["colorway"] != colorway
                        || edit["fabric"] != index || edit["map"] != "opacity");
                }
            }
            return;
        }
        let indices = super::super::vec_mut(source, "recolored");
        let index = json!(index);
        if !indices.contains(&index) {
            indices.push(index.clone());
        }
        if clear_opacity {
            if let Some(edits) = source.get_mut("retextured").and_then(Value::as_array_mut) {
                edits.retain(|edit| edit["fabric"] != index || edit["map"] != "opacity");
            }
        }
    }
}

pub(in crate::compat_scene) fn mark_texture(state: &mut Value, index: u64, map: &str, clear: bool) {
    let multiple = super::super::array(state, "colorways").len() > 1;
    let colorway = current_material_colorway(state, index);
    if let Some(source) = state.get_mut(SOURCE).filter(|source| source.is_object()) {
        if multiple {
            let edits = super::super::vec_mut(source, "colorway_textures");
            edits.retain(|edit| edit["colorway"] != colorway
                || edit["fabric"] != index || edit["map"] != map);
            edits.push(json!({"fabric": index, "colorway": colorway, "map": map, "clear": clear}));
            return;
        }
        let edits = super::super::vec_mut(source, "retextured");
        edits.retain(|edit| edit["fabric"] != index || edit["map"] != map);
        edits.push(json!({"fabric": index, "map": map, "clear": clear}));
    }
}

fn legacy_panel_name(name: &str) -> Vec<u8> {
    // The Windows CLO reference writes a Windows-1252 mirror of qsNameUTF8.
    const EXTENDED: &str = "€\0‚ƒ„…†‡ˆ‰Š‹Œ\0Ž\0\0‘’“”•–—˜™š›œ\0žŸ";
    name.chars().map(|ch| match ch as u32 {
        byte @ (0..=0x7f | 0xa0..=0xff) => byte as u8,
        _ => EXTENDED.chars().position(|candidate| candidate == ch)
            .map_or(b'?', |i| 0x80 + i as u8),
    }).collect()
}

fn rename_fields(record: &Fields<'_>, utf8: &[u8], legacy: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut updated = match record.get(utf8, BYTE_TYPE) {
        Ok(_) => record.replace(utf8, BYTE_TYPE, name.as_bytes())?,
        Err(_) => record.encoded().to_vec(),
    };
    if record.get(legacy, BYTE_TYPE).is_ok() {
        updated = Fields::read(&updated)?.replace(legacy, BYTE_TYPE, &legacy_panel_name(name))?;
    }
    Ok(updated)
}

fn update_panels(pac: &[u8], state: &Value, pose_count: usize, write_distance: bool) -> Result<Option<Vec<u8>>, String> {
    let root = Fields::read(pac)?;
    let editor = root.map(b"mapPatternEditor")?;
    let patterns = editor.map(b"mapPatternList")?;
    let records = patterns.get(b"listPattern", LIST_TYPE)?;
    let panels = super::super::array(state, "patterns");
    let meshes = super::super::array(&state["simulation"], "meshes");
    if pose_count > 0 && (meshes.len() < pose_count
        || (pose_count == panels.len() && meshes.len() != pose_count)
        || panels.iter().zip(meshes).take(pose_count).any(|(panel, mesh)| {
        panel["id"].is_null() || panel["id"] != mesh["id"] || !crate::compat_fit::mesh_uses_rest_mm(state, mesh)
    })) {
        return Err("native save requires a continued native pose for every original panel".into());
    }
    let mut current = 0;
    let mut edited = false;
    let records = map_list(records, 4096)?;
    let mut bytes = Vec::with_capacity(records.len());
    for record in records {
        if record.get(b"uiPatternType", 5)? != 0u32.to_le_bytes() {
            bytes.push(Cow::Borrowed(record.encoded()));
            continue;
        }
        let panel = panels.get(current).ok_or("native write requires the original editable panel count")?;
        let name = panel["name"].as_str()
            .ok_or("native write requires the original editable panel count")?;
        let mut posed = if current < pose_count { replace_positions(&record, &meshes[current])? } else { None };
        if write_distance {
            let distance = panel["particle_distance"].as_f64().map(|d| d as f32)
                .filter(|d| d.is_finite() && *d > 0.).ok_or("invalid appended draft particle distance")?;
            let current = Fields::read(posed.as_deref().unwrap_or_else(|| record.encoded()))?;
            let bytes = distance.to_le_bytes();
            if current.get(b"fEdgeLength", 6)? != bytes {
                posed = Some(current.replace(b"fEdgeLength", 6, &bytes)?);
            }
        }
        let current_record = Fields::read(posed.as_deref().unwrap_or_else(|| record.encoded()))?;
        if let Some(updated) = native_elastic::update_panel(&current_record, panel)? {
            posed = Some(updated);
        }
        current += 1;
        let element = record.map(b"mapElement")?;
        if element.get(b"qsNameUTF8", BYTE_TYPE)? == name.as_bytes() {
            if let Some(posed) = posed {
                bytes.push(Cow::Owned(posed));
                edited = true;
            } else {
                bytes.push(Cow::Borrowed(record.encoded()));
            }
            continue;
        }
        let record = match &posed {
            Some(bytes) => Fields::read(bytes)?,
            None => record,
        };
        if name.is_empty()
            || name.len() > 255
            || name.chars().any(char::is_control)
            || ["qsNameUTF8", "qsCategoryForDXFUTF8", "mapPatternDynamics"]
                .iter()
                .any(|marker| name.contains(marker))
        {
            return Err("native panel names must be 1 to 255 UTF-8 bytes without control characters or PAC field markers".into());
        }
        let element = rename_fields(&element, b"qsNameUTF8", b"qsName", name)?;
        let mut updated = record.replace(b"mapElement", MAP_TYPE, &element)?;
        if let Ok(shape) = record.map(b"mapFabricShape2D") {
            if let Ok(element) = shape.map(b"mapElement") {
                let element = rename_fields(&element, b"qsNameUTF8", b"qsName", name)?;
                let shape = shape.replace(b"mapElement", MAP_TYPE, &element)?;
                updated = Fields::read(&updated)?.replace(b"mapFabricShape2D", MAP_TYPE, &shape)?;
            }
        }
        if let Ok(annotation) = record.map(b"mapAnnoForPatternName") {
            let annotation = rename_fields(&annotation, b"qsTextUTF8", b"qsText", name)?;
            updated = Fields::read(&updated)?.replace(b"mapAnnoForPatternName", MAP_TYPE, &annotation)?;
        }
        bytes.push(Cow::Owned(updated));
        edited = true;
    }
    if current != panels.len() {
        return Err("native write requires the original editable panel count".into());
    }
    if !edited {
        return Ok(None);
    }
    let patterns = patterns.replace(b"listPattern", LIST_TYPE, &encoded_list(&bytes)?)?;
    let editor = editor.replace(b"mapPatternList", MAP_TYPE, &patterns)?;
    Ok(Some(root.replace(
        b"mapPatternEditor",
        MAP_TYPE,
        &editor,
    )?))
}

fn split_archive(input: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let start = header_uint(input, 32)?;
    let size = header_uint(input, 36)?;
    if start < 40
        || start.checked_add(size) != Some(input.len())
        || !input[start..].starts_with(b"PK\x03\x04")
    {
        return Err("native file ZIP boundaries differ from the verified header format".into());
    }
    Ok(input.split_at(start))
}

fn header_uint(input: &[u8], position: usize) -> Result<usize, String> {
    let bytes = input
        .get(position..position + 4)
        .ok_or("truncated native header")?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap_or_default()) as usize)
}

fn join_archive(mut prefix: Vec<u8>, archive: &[u8]) -> Result<Vec<u8>, String> {
    let start = u32::try_from(prefix.len()).map_err(|_| "native header too large")?;
    let size = u32::try_from(archive.len()).map_err(|_| "native archive too large")?;
    prefix[32..36].copy_from_slice(&start.to_le_bytes());
    prefix[36..40].copy_from_slice(&size.to_le_bytes());
    prefix.extend_from_slice(archive);
    Ok(prefix)
}

fn rewrite_archive(
    input: &[u8],
    header: &[u8],
    replacement: Option<(&[u8], &[u8])>,
    mut assets: TextureAssets,
) -> Result<Vec<u8>, String> {
    let (prefix, _) = split_archive(input)?;
    let mut source = open_archive(Cursor::new(input), header)?;
    if source.has_duplicate_names() {
        return Err("native archive has duplicate member names".into());
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.set_raw_comment(source.comment().into())
        .map_err(|error| error.to_string())?;
    let added_names: HashSet<_> = assets.added.keys().map(|name| name.to_ascii_lowercase()).collect();
    let mut replaced = false;
    for index in 0..source.len() {
        let mut file = source.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name_raw();
        if std::str::from_utf8(name).is_ok_and(|name| assets.removed.contains(name)) {
            continue;
        }
        if !added_names.is_empty()
            && added_names.contains(&file.name().map_err(|error| error.to_string())?.to_ascii_lowercase())
        {
            return Err("native texture asset name already exists in the archive".into());
        }
        let asset = std::str::from_utf8(name)
            .ok()
            .and_then(|name| assets.replaced.remove(name));
        let manifest = if name == b"clofiles.json" && (!assets.added.is_empty() || !assets.removed.is_empty()) {
            let mut data = Vec::new();
            (&mut file).take(4 * 1024 * 1024 + 1).read_to_end(&mut data)
                .map_err(|error| error.to_string())?;
            if data.len() > 4 * 1024 * 1024 {
                return Err("native file manifest exceeds 4 MiB".into());
            }
            let mut entries: serde_json::Map<String, Value> = serde_json::from_slice(&data)
                .map_err(|error| format!("invalid native file manifest: {error}"))?;
            for name in &assets.removed {
                entries.remove(name);
            }
            for name in assets.added.keys() {
                entries.insert(name.clone(), json!(name));
            }
            Some(serde_json::to_vec_pretty(&entries).map_err(|error| error.to_string())?)
        } else {
            None
        };
        let name = file.name_raw();
        let body = if let Some((_, bytes)) = replacement.filter(|(suffix, _)| {
            name.len() >= suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        }) {
            if replaced {
                return Err("multiple native archive members match the replacement".into());
            }
            replaced = true;
            Some(bytes)
        } else {
            asset.as_deref().or(manifest.as_deref())
        };
        if let Some(bytes) = body {
            let name = file.name().map_err(|error| error.to_string())?;
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file(name, options)
                .map_err(|error| error.to_string())?;
            zip.write_all(bytes).map_err(|error| error.to_string())?;
        } else {
            zip.raw_copy_file(file).map_err(|error| error.to_string())?;
        }
    }
    if (replacement.is_some() && !replaced) || !assets.replaced.is_empty() {
        return Err("native archive is missing a member being replaced".into());
    }
    for (name, data) in assets.added {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file(name, options).map_err(|error| error.to_string())?;
        zip.write_all(&data).map_err(|error| error.to_string())?;
    }
    let output = zip
        .finish()
        .map_err(|error| error.to_string())?
        .into_inner();
    join_archive(prefix.to_vec(), &output)
}

fn png_end(input: &[u8]) -> Result<usize, String> {
    let mut at = 8usize;
    loop {
        let end = at.checked_add(8).ok_or("invalid native thumbnail offset")?;
        let tag = input.get(at..end).ok_or("truncated native thumbnail")?;
        let size = u32::from_be_bytes(tag[..4].try_into().unwrap_or_default()) as usize;
        at = end
            .checked_add(size)
            .and_then(|at| at.checked_add(4))
            .filter(|&at| at <= input.len())
            .ok_or("truncated native thumbnail chunk")?;
        if &tag[4..] == b"IEND" && size == 0 {
            return Ok(at);
        }
    }
}

fn project_preview(input: Vec<u8>, thumbnail: bool, state: &Value) -> Result<Vec<u8>, String> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let (prefix, archive) = split_archive(&input)?;
    let start = header_uint(prefix, 24)?;
    let end = start
        .checked_add(header_uint(prefix, 28)?)
        .ok_or("invalid native primary thumbnail length")?;
    let main = prefix
        .get(start..end)
        .ok_or("invalid native primary thumbnail offset")?;
    if !main.starts_with(PNG) || png_end(main)? != main.len() {
        return Err("unknown native project thumbnail format".into());
    }
    let mut cursor = end;
    let mut additional = 0;
    while prefix[cursor..].starts_with(PNG) {
        cursor += png_end(&prefix[cursor..])?;
        additional += 1;
    }
    match (thumbnail, additional) {
        (true, 3) | (false, 0) => Ok(input),
        (true, 0) | (false, 3) => {
            let mut prefix = prefix[..end].to_vec();
            if thumbnail {
                prefix.extend(crate::compat_fit::native_previews(state)?);
            }
            prefix.extend_from_slice(&input[cursor..header_uint(&input, 32)?]);
            join_archive(prefix, archive)
        }
        _ => Err("unknown native project multi-view thumbnail layout".into()),
    }
}

fn archive_members<'a>(members: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Result<Vec<u8>, String> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        zip.start_file(name, options).map_err(|error| error.to_string())?;
        zip.write_all(bytes).map_err(|error| error.to_string())?;
    }
    Ok(zip.finish().map_err(|error| error.to_string())?.into_inner())
}

fn authored_garment(state: &Value, composable: bool) -> Result<Vec<u8>, String> {
    let native_author::Garment { body, images } = native_author::encode(state, composable)?;
    let manifest = serde_json::to_vec(&std::iter::once("garment.pac")
        .chain(images.keys().map(String::as_str)).map(|name| (name, name))
        .collect::<std::collections::BTreeMap<_, _>>()).map_err(|error| error.to_string())?;
    let members = [("garment.pac", body.as_slice()), ("clofiles.json", manifest.as_slice())];
    let zip = archive_members(members.into_iter().chain(
        images.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    ))?;
    let mut header = ZPAC.to_vec();
    header.extend(12u32.to_le_bytes());
    header.resize(40, 0);
    join_archive(header, &zip)
}

fn drafted_garment(state: &Value, start: usize) -> Result<Vec<u8>, String> {
    use super::super::array;
    let mut patterns = array(state, "patterns")[start..].to_vec();
    let ids = patterns.iter().map(|panel| panel["id"].clone()).collect::<HashSet<_>>();
    let mut slots = HashMap::new();
    let mut fabrics = Vec::new();
    for panel in &mut patterns {
        let index = panel["fabric_index"].as_u64().ok_or("invalid drafted fabric index")?;
        let slot = if let Some(&slot) = slots.get(&index) {
            slot
        } else {
            let mut fabric = array(state, "fabrics").iter().find(|fabric| fabric["index"] == index)
                .ok_or("drafted panel refers to a missing fabric")?.clone();
            let slot = fabrics.len();
            fabric["index"] = json!(slot);
            fabrics.push(fabric);
            slots.insert(index, slot);
            slot
        };
        panel["fabric_index"] = json!(slot);
    }
    let mut seams = Vec::new();
    for seam in array(state, "seams") {
        match (ids.contains(&seam["pattern_a"]), ids.contains(&seam["pattern_b"])) {
            (true, true) => seams.push(seam),
            (false, false) => (),
            _ => return Err("native draft append cannot join sewing across the original boundary".into()),
        }
    }
    let mut simulation = state["simulation"].clone();
    if simulation.is_object() {
        let meshes = array(&simulation, "meshes").iter()
            .filter(|mesh| ids.contains(&mesh["id"])).collect::<Vec<_>>();
        simulation["meshes"] = json!(meshes);
    }
    authored_garment(&json!({
        "patterns": patterns, "seams": seams, "fabrics": fabrics, "avatar": state["avatar"],
        "colorways": [&state["colorways"][0]], "current_colorway": 0,
        "simulation": simulation,
    }), true)
}

fn author_scene(source: &Value) -> bool {
    !source.is_object()
        || (source["authored"] == true && super::super::array(source, "appended").is_empty())
}

fn native_project(garment: Option<&[u8]>, state: &Value, thumbnail: bool) -> Result<Vec<u8>, String> {
    const WORKSPACE: &str = "garment.osw";
    const GARMENT: &str = "garment.zpac";
    let zero = 0u32.to_le_bytes();
    let linked = 1u32.to_le_bytes();
    let version = 13u32.to_le_bytes();
    let one = 1f32.to_le_bytes();
    let null = b"NULL";
    let cloth_path = garment.map_or(null.as_slice(), |_| GARMENT.as_bytes());
    let scene_properties = encode_map(&[
        (b"qsScenePropFileName", BYTE_TYPE, null),
        (b"qsScenePropFileNameUTF8", BYTE_TYPE, null),
    ])?;
    let animation = encode_map(&[
        (b"iAnimationToX", 4, &zero),
        (b"fAnimationScale", 6, &one),
        (b"iAnimationToY", 4, &linked),
        (b"iAnimationToZ", 4, &2i32.to_le_bytes()),
        (b"bAnimationInvertX", 1, &[0]),
        (b"bAnimationInvertY", 1, &[0]),
        (b"bAnimationInvertZ", 1, &[0]),
        (b"bAnimationCreateTransAnim", 1, &[0]),
        (b"dAnimationFPS", 7, &30f64.to_le_bytes()),
        (b"fAnimationTimeScale", 6, &one),
    ])?;
    // The CLO workspace reader also expects the linked scene and animation
    // records when a project contains neither scene props nor animation.
    let workspace = encode_map(&[
        (b"listAvatarFilePath", LIST_TYPE, &zero),
        (b"uiVersion", 5, &version),
        (b"uiAvatarLink", 5, &linked),
        (b"qsClothFilePathUTF8", BYTE_TYPE, cloth_path),
        (b"qsPointCacheShapeName", BYTE_TYPE, null),
        (b"uiClothLink", 5, &linked),
        (b"uiAvatarCount", 5, &zero),
        (b"qsClothFilePath", BYTE_TYPE, cloth_path),
        (b"uiScenePropLink", 5, &linked),
        (b"uiScenePropCount", 5, &zero),
        (b"mapSceneProperty", MAP_TYPE, &scene_properties),
        (b"uiAnimationLink", 5, &linked),
        (b"qsAnimationCacheFileName", BYTE_TYPE, b""),
        (b"qsAnimationFileName", BYTE_TYPE, null),
        (b"qsAnimationCacheFileNameUTF8", BYTE_TYPE, b""),
        (b"qsAnimationFileNameUTF8", BYTE_TYPE, null),
        (b"qsPointCacheShapeNameUTF8", BYTE_TYPE, null),
        (b"mapAnimationParam", MAP_TYPE, &animation),
    ])?;
    let mut manifest = serde_json::Map::from_iter([(WORKSPACE.into(), json!(WORKSPACE))]);
    let mut members = vec![(WORKSPACE, workspace.as_slice())];
    if let Some(garment) = garment {
        manifest.insert(GARMENT.into(), json!(GARMENT));
        members.push((GARMENT, garment));
    }
    let manifest = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    members.push(("clofiles.json", &manifest));
    let archive = archive_members(members)?;

    let primary = crate::compat_fit::native_primary_preview(state)?;
    let primary_size = u32::try_from(primary.len()).map_err(|_| "native project thumbnail too large")?;
    let mut prefix = ZPRJ.to_vec();
    // Version, legacy preview span, primary PNG offset and length, ZIP offset and length.
    for field in [12u32, 16, 40, primary_size, 0, 0] {
        prefix.extend_from_slice(&field.to_le_bytes());
    }
    prefix.extend(primary);
    if thumbnail {
        prefix.extend(crate::compat_fit::native_previews(state)?);
    }
    let outer = join_archive(prefix, &archive)?;
    if state["avatar"].is_null() && super::super::array(state, "additional_avatars").is_empty() {
        return Ok(outer);
    }
    let avatars = project_avatar_edits(&outer, state)?;
    rewrite_archive(&outer, ZPRJ, None, avatars)
}

fn source_path(source: &Value) -> Result<&Path, String> {
    source["path"].as_str().map(Path::new).ok_or_else(||
        "native source record has no archive path".into(),
    )
}

fn read_source(source: &Value) -> Result<Vec<u8>, String> {
    let template = source_path(source)?;
    let expected = stamp(template)?;
    if expected != source["stamp"] {
        return Err("native source archive changed since it was opened".into());
    }
    let mut original = fs::read(template).map_err(|error| format!("{}: {error}", template.display()))?;
    if stamp(template)? != expected || Some(original.len() as u64) != expected[0].as_u64() {
        return Err("native source archive changed during save".into());
    }
    if source["recovered_zpac"] == true && !restore_zpac_footer(&mut original) {
        return Err("native ZPAC source could not be recovered during save".into());
    }
    Ok(original)
}

fn garment_archive(original: Vec<u8>, project: bool) -> Result<Vec<u8>, String> {
    if project {
        member(&mut open_archive(Cursor::new(original), ZPRJ)?, "zpac")
    } else {
        Ok(original)
    }
}

fn read_avatar_source(avatar: &Value) -> Result<Vec<u8>, String> {
    let source = &avatar["native_archive"];
    if !source.is_object() {
        return Err("native ZPRJ requires a saved AVT source for each avatar".into());
    }
    let data = read_source(source)?;
    let data = if let Some(member) = source["member"].as_str() {
        let mut project = open_archive(Cursor::new(data), ZPRJ)?;
        let index = project.file_names().position(|name| name.is_ok_and(|name| name == member))
            .ok_or("native avatar source member is missing")?;
        avatar_payload(&mut project, index)?.1
    } else {
        data
    };
    member(&mut open_archive(Cursor::new(&data), AVT)?, "dan")?;
    Ok(data)
}

pub(super) fn read_avatar_dan(avatar: &Value) -> Result<Vec<u8>, String> {
    let data = read_avatar_source(avatar)?;
    member(&mut open_archive(Cursor::new(data), AVT)?, "dan")
}

fn project_avatar_edits(input: &[u8], state: &Value) -> Result<TextureAssets, String> {
    let mut outer = open_archive(Cursor::new(input), ZPRJ)?;
    let mut changes = TextureAssets::default();
    for index in avatar_members(&outer)? {
        let file = outer.by_index(index).map_err(|error| error.to_string())?;
        changes.removed.insert(file.name().map_err(|error| error.to_string())?.into_owned());
    }
    let primary = match state["avatar"].as_array() {
        Some(values) => values.as_slice(),
        None if state["avatar"].is_null() => &[],
        None => std::slice::from_ref(&state["avatar"]),
    };
    let additional = super::super::array(state, "additional_avatars");
    if primary.len() + additional.len() > 32 {
        return Err("native project has more than 32 avatars".into());
    }
    let mut bytes = 0u64;
    let mut identities = AvatarIdentities::default();
    for (index, avatar) in primary.iter().chain(additional).enumerate() {
        let mut data = read_avatar_source(avatar)?;
        if primary.len() + additional.len() > 1 {
            let mut archive = open_archive(Cursor::new(&data), AVT)?;
            let dan = member(&mut archive, "dan")?;
            if let Some(updated) = identities.separate(&dan)? {
                data = rewrite_archive(&data, AVT, Some((b".dan", &updated)), TextureAssets::default())?;
            }
        }
        bytes += data.len() as u64;
        if bytes > ARCHIVE_LIMIT {
            return Err("embedded avatar archives exceed 512 MiB".into());
        }
        changes.added.insert(format!("avatar_{index:02}.avt"), data);
    }
    let (name, workspace) = optional_member(&mut outer, "osw")?
        .ok_or("native project has no workspace avatar table")?;
    let count = (changes.added.len() as u32).to_le_bytes();
    let mut paths = count.to_vec();
    if !changes.added.is_empty() {
        paths.push(BYTE_TYPE);
        for name in changes.added.keys() {
            paths.extend_from_slice(&(name.len() as u32).to_le_bytes());
        }
        for name in changes.added.keys() {
            paths.extend_from_slice(name.as_bytes());
        }
    }
    let workspace = Fields::read(&workspace)?.replace(b"listAvatarFilePath", LIST_TYPE, &paths)?;
    let workspace = Fields::read(&workspace)?.replace(b"uiAvatarCount", 5, &count)?;
    changes.replaced.insert(name, workspace);
    Ok(changes)
}

fn rebind_project_avatars(state: &mut Value, path: &Path) -> Result<(), String> {
    let source = path.canonicalize().map_err(|error| error.to_string())?;
    let stamp = stamp(&source)?;
    let mut archive = open_archive(bounded_file(&source)?, ZPRJ)?;
    let names = avatar_members(&archive)?.iter().map(|&index| {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        Ok::<_, String>(file.name().map_err(|error| error.to_string())?.into_owned())
    }).collect::<Result<Vec<_>, String>>()?;
    if names.len() != super::super::avatar_names(state).len() {
        return Ok(());
    }
    let mut members = names.into_iter();
    let mut rebind = |avatar: &mut Value| {
        if let Some(member) = members.next() {
            avatar["native_archive"] = json!({"path": source, "stamp": stamp, "member": member});
        }
    };
    if let Some(avatars) = state["avatar"].as_array_mut() {
        avatars.iter_mut().for_each(&mut rebind);
    } else if state["avatar"].is_object() {
        rebind(&mut state["avatar"]);
    }
    if let Some(avatars) = state["additional_avatars"].as_array_mut() {
        avatars.iter_mut().for_each(rebind);
    }
    Ok(())
}

fn append_archives(
    raw: &[u8], mut pac: Vec<u8>, parts: &[Value], drafted: Option<Vec<u8>>,
) -> Result<(Vec<u8>, TextureAssets), String> {
    let mut receiver = open_archive(Cursor::new(raw), ZPAC)?;
    let mut asset_names = archive_names(&receiver)?;
    let mut images = TextureAssets::default();
    let mut total_bytes = raw.len() as u64;
    let parts = parts.iter().map(|source| garment_archive(read_source(source)?, source["project"] == true));
    for bytes in parts.chain(drafted.map(Ok)) {
        let bytes = bytes?;
        let mut archive = open_archive(Cursor::new(bytes), ZPAC)?;
        let incoming = member(&mut archive, "pac")?;
        if pac.len() + incoming.len() > 256 * 1024 * 1024 {
            return Err("native composition PAC payload exceeds 256 MiB".into());
        }
        let incoming_names = archive_names(&archive)?;
        let mut renamed = HashMap::new();
        let mut labels = None;
        for index in 0..archive.len() {
            let file = archive.by_index(index).map_err(|error| error.to_string())?;
            let name = file.name().map_err(|error| error.to_string())?.into_owned();
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".pac") || matches!(lower.as_str(), "clofiles.json" | "displaysettings.json") {
                continue;
            }
            if file.is_dir() || file.size() > 64 * 1024 * 1024 || name.contains(['/', '\\']) {
                return Err("unsupported native composition asset".into());
            }
            let mut contents = Vec::new();
            file.take(64 * 1024 * 1024 + 1).read_to_end(&mut contents).map_err(|error| error.to_string())?;
            let target = if let Some(previous) = asset_names.get(&lower) {
                let matches = if let Some(stored) = images.added.get(previous) {
                    *stored == contents
                } else {
                    let mut entry = receiver.by_name(previous).map_err(|error| error.to_string())?;
                    let mut stored = Vec::new();
                    (&mut entry).take(contents.len() as u64 + 1).read_to_end(&mut stored)
                        .map_err(|error| error.to_string())?;
                    stored == contents
                };
                if matches {
                    if *previous != name {
                        renamed.insert(lower, previous.clone());
                    }
                    continue;
                }
                if labels.is_none() {
                    labels = Some(native_textures::archive_texture_labels(&incoming)?);
                }
                let label = labels.as_ref().and_then(|labels| labels.get(&lower))
                    .ok_or_else(|| format!("native composition cannot relink conflicting asset {name:?}"))?;
                available_texture_name(&name, label, &asset_names, &incoming_names)?
            } else {
                name.clone()
            };
            if target != name {
                renamed.insert(lower, target.clone());
            }
            total_bytes += contents.len() as u64;
            if total_bytes > ARCHIVE_LIMIT {
                return Err("native composition assets exceed 512 MiB".into());
            }
            asset_names.insert(target.to_ascii_lowercase(), target.clone());
            images.added.insert(target, contents);
        }
        let updated = native_textures::remap_archive_textures(&incoming, &renamed)?;
        pac = native_compose::join(&pac, updated.as_deref().unwrap_or(&incoming))?;
    }
    Ok((pac, images))
}

fn archive_names<R: Read + std::io::Seek>(archive: &zip::ZipArchive<R>) -> Result<HashMap<String, String>, String> {
    let mut names = HashMap::new();
    for name in archive.file_names() {
        let name = name.map_err(|error| error.to_string())?.into_owned();
        if names.insert(name.to_ascii_lowercase(), name).is_some() {
            return Err("native composition has ambiguous archive filenames".into());
        }
    }
    Ok(names)
}

fn available_texture_name(
    name: &str, label: &str, occupied: &HashMap<String, String>, incoming: &HashMap<String, String>,
) -> Result<String, String> {
    let path = Path::new(name);
    let extension = path.extension().and_then(|part| part.to_str()).ok_or("conflicting asset has no extension")?;
    // CLO removes the final numeric suffix when displaying the texture label.
    (1_000_001..=1_004_096).map(|index| format!("{label}_{index}.{extension}"))
        .find(|candidate| {
            let folded = candidate.to_ascii_lowercase();
            !occupied.contains_key(&folded) && !incoming.contains_key(&folded)
        }).ok_or_else(|| "native composition could not allocate a distinct asset filename".into())
}

pub(in crate::compat_scene) fn save(
    path: &Path,
    state: &mut Value,
    project: bool,
    thumbnail: bool,
) -> Result<(), String> {
    let output = encode(state, project, thumbnail)?;
    write(path, &output)?;
    if project {
        remember_project_save(state, path);
    } else if author_scene(&state[SOURCE]) && state[SOURCE]["project"] != true {
        remember(state, path, false, false);
        if state[SOURCE].is_object() {
            state[SOURCE]["authored"] = json!(true);
        }
    } else if state[SOURCE]["empty_append"] == true {
        remember_empty_append(state, path, false, false);
    } else if state[SOURCE]["project"] != true {
        // The saved garment has compact indices. Keep the original parts
        // when their indices differ from the live fabric catalog.
        let source = &state[SOURCE];
        let unused_fabrics = super::super::assigned_fabric_count(state) != super::super::array(state, "fabrics").len();
        let appended = !super::super::array(source, "appended").is_empty();
        let compacted = source["compacted_fabrics"] == true
            || source.get("retained_panels").is_some() && unused_fabrics;
        if !appended || !unused_fabrics {
            remember(state, path, false, false);
            if compacted && state[SOURCE].is_object() {
                state[SOURCE]["compacted_fabrics"] = json!(true);
            }
        }
    }
    Ok(())
}

pub(in crate::compat_scene) fn remember_project_save(state: &mut Value, path: &Path) {
    let source = &state[SOURCE];
    let authored = author_scene(source) || super::super::array(state, "patterns").is_empty();
    let compacted = source["compacted_fabrics"] == true
        || (source["empty_append"] == true || source.get("retained_panels").is_some()
            || !super::super::array(source, "appended").is_empty())
            && super::super::assigned_fabric_count(state) != super::super::array(state, "fabrics").len();
    let _ = rebind_project_avatars(state, path);
    remember(state, path, true, false);
    if authored && state[SOURCE].is_object() {
        state[SOURCE]["authored"] = json!(true);
    }
    if compacted && state[SOURCE].is_object() {
        state[SOURCE]["compacted_fabrics"] = json!(true);
    }
}

pub(in crate::compat_scene) fn encode(
    state: &Value,
    project: bool,
    thumbnail: bool,
) -> Result<Vec<u8>, String> {
    if project && super::super::array(state, "patterns").is_empty()
        && super::super::array(state, "imported_meshes").is_empty()
    {
        let output = native_project(None, state, thumbnail)?;
        if output.len() as u64 > ARCHIVE_LIMIT {
            return Err("native output archive exceeds 512 MiB".into());
        }
        return Ok(output);
    }
    let source = &state[SOURCE];
    if author_scene(source) {
        let garment = authored_garment(state, false)?;
        let output = if project { native_project(Some(&garment), state, thumbnail)? } else { garment };
        if output.len() as u64 > ARCHIVE_LIMIT {
            return Err("native output archive exceeds 512 MiB".into());
        }
        return Ok(output);
    }
    source_path(source)?;
    let parts = super::super::array(source, "appended");
    let draft = drafted_append(state)?;
    let source_project = source["project"] == true;
    let empty_append = source["empty_append"] == true;
    let switched = state["current_colorway"] != source["current_colorway"];
    let changed_material = source["fabrics"].as_u64() != Some(fabric_fingerprint(state));
    let only_unused_changed = changed_material && !has_material_edits(source)
        && source["assigned_fabrics"].as_u64() == Some(assigned_fabric_fingerprint(state));
    let recolors: HashSet<[usize; 2]> = serde_json::from_value(source["colorway_recolors"].clone())
        .map_err(|_| "invalid native colorway recolor targets")?;
    let texture_edits: Vec<ColorwayTextureEdit> = serde_json::from_value(source["colorway_textures"].clone())
        .map_err(|_| "invalid native colorway texture targets")?;
    let write_colorways = switched || !recolors.is_empty() || !texture_edits.is_empty();
    let can_save_materials = if empty_append || !parts.is_empty() || source["compacted_fabrics"] == true {
        !write_colorways && (!changed_material || only_unused_changed) && !has_material_edits(source)
    } else {
        !write_colorways && (!changed_material || only_unused_changed)
            || super::super::array(source, "recolored").is_empty()
                && super::super::array(source, "retextured").is_empty()
                && verify_colorway_materials(state, source, &recolors, &texture_edits)
    };
    let changed_garment = source["garment"].as_u64() != Some(garment_fingerprint(state));
    let only_elastic = changed_garment
        && source["garment_before_elastic"].as_u64() == Some(garment_fingerprint_inner(state, true, true));
    let write_pose = changed_garment && !only_elastic && parts.is_empty() && !empty_append
        && (source["layout"].as_u64() == Some(layout_fingerprint(state))
            || source["layout_before_elastic"].as_u64() == Some(garment_fingerprint_inner(state, true, false)));
    if draft.is_none() && (changed_garment && !only_elastic && !write_pose || !can_save_materials) {
        return Err("native save requires the original layout, with compatible pose, name and fabric edits".into());
    }
    let original = read_source(source)?;
    let garment = garment_archive(original.clone(), source_project)?;
    let mut garment_archive = open_archive(Cursor::new(garment.as_slice()), ZPAC)?;
    let mut pac = member(&mut garment_archive, "pac")?;
    let retained_panels = source.get("retained_panels");
    if let Some(retained) = retained_panels {
        let slots = serde_json::from_value::<Vec<usize>>(retained.clone())
            .map_err(|_| "invalid native retained panel slots")?;
        pac = native_compose::keep_panels(&pac, &slots)?;
    }
    if empty_append {
        // The destination's empty-scene fabric is not part of the exported
        // garment. Its palette is retained when adding each source part.
        let name = state["colorways"][0].as_str().ok_or("native ZPAC has no active colorway name")?;
        pac = native_materials::default_append_colorway(&pac, name)?;
    }
    let (pac, appended_assets) = if parts.is_empty() && draft.is_none() {
        (pac, TextureAssets::default())
    } else {
        let drafted = draft.map(|start| drafted_garment(state, start)).transpose()?;
        append_archives(&garment, pac, parts, drafted)?
    };
    let asset_names = || {
        garment_archive.file_names().map(|name| name.map(|name| name.to_ascii_lowercase()))
            .collect::<Result<HashSet<_>, _>>().map_err(|error| error.to_string())
    };
    let pose_count = if let Some(start) = draft {
        if source["draft_append"]["has_pose"] == true { start } else { 0 }
    } else if write_pose {
        super::super::array(state, "patterns").len()
    } else {
        0
    };
    let panel_edits = update_panels(&pac, state, pose_count, draft.is_some())?;
    let (edited, assets) = if !parts.is_empty() || empty_append || draft.is_some() {
        (Some(panel_edits.unwrap_or(pac)), appended_assets)
    } else if write_colorways {
        let index = state["current_colorway"]
            .as_u64()
            .and_then(|index| u32::try_from(index).ok())
            .ok_or("invalid active colorway index")?;
        let pac = panel_edits.as_deref().unwrap_or(&pac);
        let recolored = if recolors.is_empty() {
            None
        } else {
            Some(native_recolor::recolor_colorways(pac, state, &recolors, index, &texture_edits)?)
        };
        if texture_edits.is_empty() {
            let pac = match recolored {
                Some(pac) => pac,
                None => native_materials::select_colorway(pac, index)?,
            };
            (Some(pac), TextureAssets::default())
        } else {
            let (pac, added) = native_textures::replace_colorways(
                recolored.as_deref().unwrap_or(pac), state, &texture_edits, index, asset_names()?,
            )?;
            (Some(pac), TextureAssets { added, ..TextureAssets::default() })
        }
    } else {
        let recolored = native_recolor::recolor(
            panel_edits.as_deref().unwrap_or(&pac),
            state,
            super::super::array(source, "recolored"),
            super::super::array(source, "retextured"),
        )?;
        let edited = recolored.or(panel_edits);
        let retextured = super::super::array(source, "retextured");
        let occupied = if retextured.is_empty() { HashSet::new() } else { asset_names()? };
        let (textures, assets) = native_textures::replacements(
            edited.as_deref().unwrap_or(&pac),
            state,
            super::super::array(source, "textures"),
            super::super::array(source, "recolored"),
            retextured,
            occupied,
        )?;
        (textures.or(edited).or_else(|| retained_panels.map(|_| pac)), assets)
    };
    let edited = if retained_panels.is_some() {
        edited.map(native_compose::compact_deleted_panel_fabrics).transpose()?
    } else {
        edited
    };
    let has_edits = edited.is_some();
    let garment = match edited {
        Some(bytes) => rewrite_archive(&garment, ZPAC, Some((b".pac", &bytes)), assets)?,
        None => garment,
    };
    let output = if project && (!source_project || empty_append) {
        native_project(Some(&garment), state, thumbnail)?
    } else if project {
        let avatar_changed = source["avatars"].as_u64() != Some(avatar_fingerprint(state));
        let avatar_assets = if avatar_changed {
            project_avatar_edits(&original, state)?
        } else {
            TextureAssets::default()
        };
        let project_bytes = if has_edits || avatar_changed {
            rewrite_archive(&original, ZPRJ, Some((b".zpac", &garment)), avatar_assets)?
        } else {
            original
        };
        project_preview(project_bytes, thumbnail, state)?
    } else {
        garment
    };
    if output.len() as u64 > ARCHIVE_LIMIT {
        return Err("native output archive exceeds 512 MiB".into());
    }
    Ok(output)
}
