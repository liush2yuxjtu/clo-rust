//! Preserve proprietary scene payloads while updating verified editable fields.

use super::{
    clo_native::{member, open_archive, ARCHIVE_LIMIT, ZPAC, ZPRJ},
    native_pose::{encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE},
    native_recolor, native_textures, write,
};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Write};
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
        "current_colorway",
        "imported_meshes",
    ] {
        state[key].hash(&mut hash);
    }
    for fabric in super::super::array(state, "fabrics") {
        hash_except(fabric, &["color", "rgba", "texture_maps", "info"], &mut hash);
        hash_except(&fabric["info"], &["catalog"], &mut hash);
        hash_except(&fabric["info"]["catalog"], &["Color", "Texture"], &mut hash);
    }
    let simulation = &state["simulation"];
    hash_except(simulation, &["meshes"], &mut hash);
    for mesh in super::super::array(simulation, "meshes") {
        hash_except(mesh, &["name"], &mut hash);
    }
    hash.finish()
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
                "avatars": avatar_fingerprint(state),
                "textures": super::super::array(state, "fabrics").iter().map(|fabric| &fabric["texture_maps"]).collect::<Vec<_>>(),
            }))
        })
        .unwrap_or(Value::Null);
    state[SOURCE] = value;
}

pub(in crate::compat_scene) fn mark_color(state: &mut Value, index: u64) {
    if let Some(source) = state.get_mut(SOURCE).filter(|source| source.is_object()) {
        let indices = super::super::vec_mut(source, "recolored");
        let index = json!(index);
        if !indices.contains(&index) {
            indices.push(index);
        }
    }
}

pub(in crate::compat_scene) fn mark_texture(state: &mut Value, index: u64, map: &str) {
    if let Some(source) = state.get_mut(SOURCE).filter(|source| source.is_object()) {
        let edits = super::super::vec_mut(source, "retextured");
        let edit = json!({"fabric": index, "map": map});
        if !edits.contains(&edit) {
            edits.push(edit);
        }
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

fn replace_member(
    input: &[u8],
    header: &[u8],
    suffix: &[u8],
    replacement: &[u8],
    mut assets: HashMap<String, Vec<u8>>,
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
        let file = source.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name_raw();
        let asset = std::str::from_utf8(name)
            .ok()
            .and_then(|name| assets.remove(name));
        let body = if name.len() >= suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        {
            if replaced {
                return Err("multiple native archive members match the replacement".into());
            }
            replaced = true;
            Some(replacement)
        } else {
            asset.as_deref()
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
    if !replaced || !assets.is_empty() {
        return Err("native archive is missing a member being replaced".into());
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
    let source = &state[SOURCE];
    let template = source["path"].as_str().map(Path::new).ok_or(
        "native save requires a compatible .zprj or .zpac loaded into this Rust session; use .clorust for a new Rust scene",
    )?;
    let source_project = source["project"] == true;
    if project && !source_project {
        return Err("saving a .zprj requires a loaded native .zprj project".into());
    }
    if source["garment"].as_u64() != Some(garment_fingerprint(state))
        || project && source["avatars"].as_u64() != Some(avatar_fingerprint(state))
    {
        return Err("native save currently preserves the loaded scene, pattern renames and verified fabric edits; save other edits as .clorust".into());
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
    let pac = member(
        &mut open_archive(Cursor::new(garment.as_slice()), ZPAC)?,
        "pac",
    )?;
    let renamed = renamed_pac(&pac, state)?;
    let recolored = native_recolor::recolor(
        renamed.as_deref().unwrap_or(&pac),
        state,
        super::super::array(source, "recolored"),
    )?;
    let edited = recolored.or(renamed);
    let (textures, assets) = native_textures::replacements(
        edited.as_deref().unwrap_or(&pac),
        state,
        super::super::array(source, "textures"),
        super::super::array(source, "recolored"),
        super::super::array(source, "retextured"),
    )?;
    let edited = textures.or(edited);
    let has_edits = edited.is_some();
    let garment = match edited {
        Some(bytes) => replace_member(&garment, ZPAC, b".pac", &bytes, assets)?,
        None => garment,
    };
    let output = if project {
        let project_bytes = if has_edits {
            replace_member(&original, ZPRJ, b".zpac", &garment, HashMap::new())?
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
    write(path, &output)?;
    if project == source_project {
        remember(state, path, project);
    }
    Ok(())
}
