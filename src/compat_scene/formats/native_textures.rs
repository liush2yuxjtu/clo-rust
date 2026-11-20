//! Resolve and replace texture images referenced by native fabric records.

use super::clo_native::find;
use super::native_materials::{face_records, face_type};
use super::native_pose::{encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE};
use super::native_recolor::edit_materials;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hasher};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::ZipArchive;

const MAX_IMAGE: u64 = 64 * 1024 * 1024;
const TYPES: &[(&str, i32)] = &[
    ("base", 0),
    ("normal", 4),
    ("displacement", 5),
    ("roughness", 11),
    ("metalness", 12),
    ("opacity", 21),
];

fn integer(field: &Fields<'_>, key: &[u8]) -> Result<i32, String> {
    let raw = field
        .get(key, 4)?
        .try_into()
        .map_err(|_| "invalid native texture field width")?;
    Ok(i32::from_le_bytes(raw))
}

fn filename<'a>(texture: &Fields<'a>) -> Result<&'a str, String> {
    let (raw, legacy) = match texture.get(b"qsUniqueFileNameUTF8", BYTE_TYPE) {
        Ok(raw) if !raw.is_empty() => (raw, b"qsUniqueFileName".as_slice()),
        _ => (
            texture.get(b"qsFileNameUTF8", BYTE_TYPE)?,
            b"qsFileName".as_slice(),
        ),
    };
    let name = std::str::from_utf8(raw).map_err(|_| "invalid native texture asset name")?;
    if name.is_empty() || name.contains(['/', '\\', '\0']) || matches!(name, "." | "..") {
        return Err("native texture asset must be a single archive filename".into());
    }
    if texture.get(legacy, BYTE_TYPE)? != raw {
        return Err("native texture filenames do not match".into());
    }
    Ok(name)
}

fn textures<'a>(face: &Fields<'a>) -> Result<Vec<Fields<'a>>, String> {
    map_list(face.get(b"listTexture", LIST_TYPE)?, 64)
}

pub(super) fn base_texture_label<'a>(face: &Fields<'a>) -> Option<&'a str> {
    let texture = textures(face)
        .ok()?
        .into_iter()
        .find(|texture| integer(texture, b"enType") == Ok(0))?;
    let original = std::str::from_utf8(texture.get(b"qsFileNameUTF8", BYTE_TYPE).ok()?).ok()?;
    Path::new(original.rsplit(['/', '\\']).next()?)
        .file_stem()?
        .to_str()
}

fn cache_image(bytes: &[u8]) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let folder = std::env::var_os("CLO_RUST_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("clo-rust-results"))
        .join("native-textures");
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let folder = folder.canonicalize().map_err(|e| e.to_string())?;
    let mut hash = DefaultHasher::new();
    hash.write(bytes);
    let stem = format!("texture-{:016x}-{}", hash.finish(), bytes.len());
    let mut path = folder.join(format!("{stem}.png"));
    loop {
        match File::create_new(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes) {
                    let _ = fs::remove_file(&path);
                    return Err(error.to_string());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::symlink_metadata(&path)
                    .is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64)
                    && fs::read(&path).is_ok_and(|stored| stored == bytes)
                {
                    return Ok(path);
                }
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                path = folder.join(format!("{stem}-{}-{id}.png", std::process::id()));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn load_image<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<PathBuf, String> {
    let mut file = archive.by_name(name).map_err(|e| e.to_string())?;
    if file.is_dir() || file.size() > MAX_IMAGE {
        return Err("native texture image exceeds supported size".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_IMAGE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != file.size() {
        return Err("unexpected native texture length".into());
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") && !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Err("native texture is not PNG or JPEG".into());
    }
    cache_image(&bytes)
}

fn front_texture_refs<'a>(material: &Fields<'a>) -> Result<Vec<(&'static str, &'a str)>, String> {
    let (faces, legacy_front) = face_records(material)?;
    let mut refs = Vec::new();
    for (index, face) in faces.iter().enumerate() {
        if face_type(face, index, legacy_front)? != 0 {
            continue;
        }
        for texture in textures(face)? {
            let kind = integer(&texture, b"enType")?;
            if let Some((key, _)) = TYPES.iter().find(|(_, type_id)| *type_id == kind) {
                if let Ok(name) = filename(&texture) {
                    refs.push((*key, name));
                }
            }
        }
    }
    Ok(refs)
}

pub(super) fn fabric_maps<R: Read + Seek>(
    material: &Fields<'_>,
    archive: &mut ZipArchive<R>,
) -> Result<(Value, Vec<String>), String> {
    let mut maps = json!({});
    let mut warnings = Vec::new();
    let mut total = 0u64;
    for (key, name) in front_texture_refs(material)? {
        let size = archive.by_name(name).map_or(0, |file| file.size());
        if total.saturating_add(size) > 256 * 1024 * 1024 {
            return Err("fabric texture cache exceeded 256 MiB".into());
        }
        match load_image(archive, name) {
            Ok(path) => {
                total += size;
                maps[key] = json!(path);
            }
            Err(error) => warnings.push(format!("native texture {name}: {error}")),
        }
    }
    Ok((maps, warnings))
}

pub(super) fn hydrate<R: Read + Seek>(
    pac: &[u8],
    archive: &mut ZipArchive<R>,
    state: &mut Value,
) -> Result<(), String> {
    if super::super::array(state, "fabrics").is_empty() {
        return Ok(());
    }
    let root = Fields::read(pac)?;
    let fabrics = map_list(root.get(b"listFabric", LIST_TYPE)?, 4096)?;
    let mut cache: HashMap<String, PathBuf> = HashMap::new();
    let mut totals = 0u64;
    let mut warnings = Vec::new();
    for (index, fabric) in fabrics.iter().enumerate() {
        let material = fabric.map(b"mapMaterial2D")?;
        for (key, name) in front_texture_refs(&material)? {
            let cached = if let Some(path) = cache.get(name) {
                path.clone()
            } else {
                // Bound extracted assets from one scene, including multiple fabrics.
                let size = archive.by_name(name).map_or(0, |file| file.size());
                if totals.saturating_add(size) > 256 * 1024 * 1024 {
                    warnings.push("native texture cache exceeded 256 MiB".to_owned());
                    continue;
                }
                match load_image(archive, name) {
                    Ok(path) => {
                        totals += size;
                        cache.insert(name.to_owned(), path.clone());
                        path
                    }
                    Err(error) => {
                        warnings.push(format!("native texture {name}: {error}"));
                        continue;
                    }
                }
            };
            state["fabrics"][index]["texture_maps"][key] = json!(cached);
        }
    }
    state["native_texture_warnings"] = json!(warnings);
    Ok(())
}

struct Edit {
    kind: i32,
    image: Option<Vec<u8>>,
    matches: RefCell<Vec<String>>,
}

fn update_face(face: &Fields<'_>, edits: &Vec<Edit>) -> Result<Option<Vec<u8>>, String> {
    if integer(face, b"enMaterialFaceType")? != 0 {
        return Ok(None);
    }
    let list = textures(face)?;
    let mut reset = false;
    let mut converted = HashMap::new();
    let mut removed = Vec::new();
    for edit in edits {
        let mut found = None;
        for (index, texture) in list.iter().enumerate() {
            if integer(texture, b"enType")? != edit.kind {
                continue;
            }
            if edit.image.is_none() {
                removed.push(index);
                reset |= edit.kind == 0;
                continue;
            }
            let name = filename(texture)?;
            let image_type = integer(texture, b"enImageType")?;
            if !matches!(image_type, 2 | 5)
                || !Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                return Err("native texture replacement requires a PNG archive slot".into());
            }
            if found.replace(name).is_some() {
                return Err("native texture references are not uniquely paired".into());
            }
            if image_type == 2 {
                converted.insert(
                    index,
                    texture.replace(b"enImageType", 4, &5i32.to_le_bytes())?,
                );
            }
        }
        if edit.image.is_some() {
            let name = found.ok_or("native save requires an existing texture slot on the fabric")?;
            edit.matches.borrow_mut().push(name.to_owned());
            reset |= edit.kind == 0;
        }
    }
    if !reset && converted.is_empty() && removed.is_empty() {
        return Ok(None);
    }
    let mut updated = if reset {
        face.replace(b"fDesaturated", 6, &0.0f32.to_le_bytes())?
    } else {
        face.encoded().to_vec()
    };
    if !converted.is_empty() || !removed.is_empty() {
        let textures: Vec<_> = list
            .iter()
            .enumerate()
            .filter(|(index, _)| !removed.contains(index))
            .map(|(index, texture)| {
                converted
                    .remove(&index)
                    .map_or(Cow::Borrowed(texture.encoded()), Cow::Owned)
            })
            .collect();
        updated = Fields::read(&updated)?.replace(
            b"listTexture",
            LIST_TYPE,
            &encoded_list(&textures)?,
        )?;
    }
    Ok(Some(updated))
}

fn occurrences(mut data: &[u8], needle: &[u8]) -> usize {
    let mut count = 0;
    while let Some(index) = find(data, needle) {
        count += 1;
        data = &data[index + needle.len()..];
    }
    count
}

pub(super) fn replacements(
    pac: &[u8],
    state: &Value,
    baseline: &[Value],
    recolored: &[Value],
    retextured: &[Value],
) -> Result<(Option<Vec<u8>>, HashMap<String, Vec<u8>>), String> {
    let mut edits = HashMap::new();
    let fabrics = super::super::array(state, "fabrics");
    if fabrics.len() != baseline.len() {
        return Err("native fabric texture baseline no longer matches the scene".into());
    }
    let mut image_bytes = 0;
    for (index, fabric) in fabrics.iter().enumerate() {
        for &(key, kind) in TYPES {
            let current = &fabric["texture_maps"][key];
            let explicit = retextured
                .iter()
                .find(|edit| edit["fabric"].as_u64() == Some(index as u64) && edit["map"] == key);
            if current == &baseline[index][key] && explicit.is_none() {
                continue;
            }
            // The recolor pass handles desaturating base and clearing opacity.
            if matches!(key, "base" | "opacity")
                && current.is_null()
                && recolored.iter().any(|id| id.as_u64() == Some(index as u64))
                && !explicit.is_some_and(|edit| edit["clear"] == true)
            {
                continue;
            }
            let image = if current.is_null() && explicit.is_some_and(|edit| edit["clear"] == true) {
                None
            } else {
                let path = current.as_str().filter(|path| !path.is_empty())
                    .ok_or("native texture removal requires an explicit texture edit")?;
                Some(crate::compat_fit::png_for_export(Path::new(path))?)
            };
            image_bytes += image.as_ref().map_or(0, Vec::len);
            if image_bytes > 256 * 1024 * 1024 {
                return Err("replacement texture assets exceed 256 MiB".into());
            }
            edits.entry(index).or_insert_with(Vec::new).push(Edit {
                kind,
                image,
                matches: RefCell::new(Vec::new()),
            });
        }
    }
    let updated = edit_materials(pac, &edits, update_face)?;
    let mut images = HashMap::new();
    for edit in edits.into_values().flatten() {
        let Some(image) = edit.image else { continue };
        let matches = edit.matches.into_inner();
        let name = matches
            .first()
            .ok_or("native texture binding was not found")?;
        if matches.len() < 4
            || matches.iter().any(|candidate| candidate != name)
            || occurrences(pac, name.as_bytes()) != matches.len() * 2
            || images.contains_key(name)
        {
            return Err(
                "native texture asset is shared or its references could not be verified".into(),
            );
        }
        images.insert(name.clone(), image);
    }
    Ok((updated, images))
}
