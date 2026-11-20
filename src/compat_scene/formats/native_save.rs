//! Preserve proprietary scene payloads while updating verified editable fields.

use super::{
    clo_native::{member, open_archive, ARCHIVE_LIMIT, ZPAC, ZPRJ},
    native_pose::{encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE},
    native_materials, native_recolor, native_textures, write,
};
use serde_json::{json, Value};
use native_textures::ColorwayTextureEdit;
use std::borrow::Cow;
use std::collections::{hash_map::DefaultHasher, BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

pub(in crate::compat_scene) const SOURCE: &str = "native_archive_source";

fn stamp(path: &Path) -> Result<Value, String> {
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

fn garment_fingerprint(state: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    for pattern in super::super::array(state, "patterns") {
        hash_except(pattern, &["name"], &mut hash);
    }
    for key in [
        "seams",
        "native_seam_groups",
        "garment_spec",
        "colorways",
        "imported_meshes",
    ] {
        state[key].hash(&mut hash);
    }
    let simulation = &state["simulation"];
    hash_except(simulation, &["meshes"], &mut hash);
    for mesh in super::super::array(simulation, "meshes") {
        hash_except(mesh, &["name"], &mut hash);
    }
    hash.finish()
}

fn fabric_fingerprint(state: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    state["current_colorway"].hash(&mut hash);
    for fabric in super::super::array(state, "fabrics") {
        hash_editable_material(fabric, false, true, &mut hash);
    }
    hash.finish()
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

pub(in crate::compat_scene) fn remember(state: &mut Value, path: &Path, project: bool) {
    let value = path
        .canonicalize()
        .ok()
        .and_then(|path| {
            Some(json!({
                "stamp": stamp(&path).ok()?,
                "path": path.to_str()?,
                "project": project,
                "garment": garment_fingerprint(state),
                "fabrics": fabric_fingerprint(state),
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

fn renamed_pac(pac: &[u8], state: &Value) -> Result<Option<Vec<u8>>, String> {
    let root = Fields::read(pac)?;
    let editor = root.map(b"mapPatternEditor")?;
    let patterns = editor.map(b"mapPatternList")?;
    let records = patterns.get(b"listPattern", LIST_TYPE)?;
    let names = super::super::array(state, "patterns");
    let mut current = 0;
    let mut edited = false;
    let records = map_list(records, 4096)?;
    let mut bytes = Vec::with_capacity(records.len());
    for record in records {
        if record.get(b"uiPatternType", 5)? != 0u32.to_le_bytes() {
            bytes.push(Cow::Borrowed(record.encoded()));
            continue;
        }
        let name = names
            .get(current)
            .and_then(|piece| piece["name"].as_str())
            .ok_or("native write requires the original editable panel count")?;
        current += 1;
        let element = record.map(b"mapElement")?;
        if element.get(b"qsNameUTF8", BYTE_TYPE)? == name.as_bytes() {
            bytes.push(Cow::Borrowed(record.encoded()));
            continue;
        }
        if name.is_empty()
            || name.len() > 255
            || name.chars().any(char::is_control)
            || ["qsNameUTF8", "qsCategoryForDXFUTF8", "mapPatternDynamics"]
                .iter()
                .any(|marker| name.contains(marker))
        {
            return Err("native panel names must be 1 to 255 UTF-8 bytes without control characters or PAC field markers".into());
        }
        let element = element.replace(b"qsNameUTF8", BYTE_TYPE, name.as_bytes())?;
        bytes.push(Cow::Owned(record.replace(
            b"mapElement",
            MAP_TYPE,
            &element,
        )?));
        edited = true;
    }
    if current != names.len() {
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

#[derive(Default)]
struct TextureAssets {
    replaced: HashMap<String, Vec<u8>>,
    added: BTreeMap<String, Vec<u8>>,
}

fn replace_member(
    input: &[u8],
    header: &[u8],
    suffix: &[u8],
    replacement: &[u8],
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
    let mut replaced = false;
    for index in 0..source.len() {
        let mut file = source.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name_raw();
        if !assets.added.is_empty()
            && assets.added.contains_key(&file.name().map_err(|error| error.to_string())?.to_ascii_lowercase())
        {
            return Err("native texture asset name already exists in the archive".into());
        }
        let asset = std::str::from_utf8(name)
            .ok()
            .and_then(|name| assets.replaced.remove(name));
        let manifest = if name == b"clofiles.json" && !assets.added.is_empty() {
            let mut data = Vec::new();
            (&mut file).take(4 * 1024 * 1024 + 1).read_to_end(&mut data)
                .map_err(|error| error.to_string())?;
            if data.len() > 4 * 1024 * 1024 {
                return Err("native file manifest exceeds 4 MiB".into());
            }
            let mut entries: serde_json::Map<String, Value> = serde_json::from_slice(&data)
                .map_err(|error| format!("invalid native file manifest: {error}"))?;
            for name in assets.added.keys() {
                entries.insert(name.clone(), json!(name));
            }
            Some(serde_json::to_vec_pretty(&entries).map_err(|error| error.to_string())?)
        } else {
            None
        };
        let name = file.name_raw();
        let body = if name.len() >= suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        {
            if replaced {
                return Err("multiple native archive members match the replacement".into());
            }
            replaced = true;
            Some(replacement)
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
    if !replaced || !assets.replaced.is_empty() {
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

pub(in crate::compat_scene) fn save(
    path: &Path,
    state: &mut Value,
    project: bool,
    thumbnail: bool,
) -> Result<(), String> {
    let output = encode(state, project, thumbnail)?;
    write(path, &output)?;
    if (state[SOURCE]["project"] == true) == project {
        remember(state, path, project);
    }
    Ok(())
}

pub(in crate::compat_scene) fn encode(
    state: &Value,
    project: bool,
    thumbnail: bool,
) -> Result<Vec<u8>, String> {
    let source = &state[SOURCE];
    let template = source["path"].as_str().map(Path::new).ok_or(
        "native save requires a compatible .zprj or .zpac loaded into this Rust session; use .clorust for a new Rust scene",
    )?;
    let source_project = source["project"] == true;
    if project && !source_project {
        return Err("saving a .zprj requires a loaded native .zprj project".into());
    }
    let switched = state["current_colorway"] != source["current_colorway"];
    let changed_material = source["fabrics"].as_u64() != Some(fabric_fingerprint(state));
    let recolors: HashSet<[usize; 2]> = serde_json::from_value(source["colorway_recolors"].clone())
        .map_err(|_| "invalid native colorway recolor targets")?;
    let texture_edits: Vec<ColorwayTextureEdit> = serde_json::from_value(source["colorway_textures"].clone())
        .map_err(|_| "invalid native colorway texture targets")?;
    let write_colorways = switched || !recolors.is_empty() || !texture_edits.is_empty();
    let can_save_materials = !write_colorways && !changed_material
        || super::super::array(source, "recolored").is_empty()
            && super::super::array(source, "retextured").is_empty()
            && verify_colorway_materials(state, source, &recolors, &texture_edits);
    if source["garment"].as_u64() != Some(garment_fingerprint(state))
        || !can_save_materials
        || project && source["avatars"].as_u64() != Some(avatar_fingerprint(state))
    {
        return Err("native save supports the loaded scene, pattern renames, verified fabric edits and archived colorway edits. Save other edits as .clorust".into());
    }
    let expected = stamp(template)?;
    if expected != source["stamp"] {
        return Err("native source archive changed since it was opened".into());
    }
    let original =
        fs::read(template).map_err(|error| format!("{}: {error}", template.display()))?;
    if stamp(template)? != expected || Some(original.len() as u64) != expected[0].as_u64() {
        return Err("native source archive changed during save".into());
    }
    let garment = if source_project {
        let mut archive = open_archive(Cursor::new(original.as_slice()), ZPRJ)?;
        member(&mut archive, "zpac")?
    } else {
        original.clone()
    };
    let mut garment_archive = open_archive(Cursor::new(garment.as_slice()), ZPAC)?;
    let pac = member(&mut garment_archive, "pac")?;
    let renamed = renamed_pac(&pac, state)?;
    let (edited, assets) = if write_colorways {
        let index = state["current_colorway"]
            .as_u64()
            .and_then(|index| u32::try_from(index).ok())
            .ok_or("invalid active colorway index")?;
        let pac = renamed.as_deref().unwrap_or(&pac);
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
            let occupied = garment_archive.file_names()
                .map(|name| name.map(|name| name.to_ascii_lowercase()))
                .collect::<Result<_, _>>().map_err(|error| error.to_string())?;
            let (pac, added) = native_textures::replace_colorways(
                recolored.as_deref().unwrap_or(pac), state, &texture_edits, index, occupied,
            )?;
            (Some(pac), TextureAssets { added, ..TextureAssets::default() })
        }
    } else {
        let recolored = native_recolor::recolor(
            renamed.as_deref().unwrap_or(&pac),
            state,
            super::super::array(source, "recolored"),
            super::super::array(source, "retextured"),
        )?;
        let edited = recolored.or(renamed);
        let (textures, assets) = native_textures::replacements(
            edited.as_deref().unwrap_or(&pac),
            state,
            super::super::array(source, "textures"),
            super::super::array(source, "recolored"),
            super::super::array(source, "retextured"),
        )?;
        (textures.or(edited), TextureAssets { replaced: assets, ..TextureAssets::default() })
    };
    let has_edits = edited.is_some();
    let garment = match edited {
        Some(bytes) => replace_member(&garment, ZPAC, b".pac", &bytes, assets)?,
        None => garment,
    };
    let output = if project {
        let project_bytes = if has_edits {
            replace_member(&original, ZPRJ, b".zpac", &garment, TextureAssets::default())?
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
