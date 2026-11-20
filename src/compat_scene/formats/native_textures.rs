//! Resolve and replace texture images referenced by native fabric records.

use super::clo_native::find;
use super::native_materials::{
    colorway_materials, face_records, face_type, material, read_colorways, select_colorway,
};
use super::native_pose::{encode_map, encoded_list, map_list, Fields, BYTE_TYPE, LIST_TYPE, MAP_TYPE};
use super::native_recolor::edit_materials_in_active_colorway;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{DefaultHasher, Hasher};
use std::io::{Cursor, Read, Seek, Write};
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

#[derive(serde::Deserialize)]
pub(super) struct ColorwayTextureEdit {
    pub colorway: usize,
    pub fabric: usize,
    pub map: String,
    pub clear: bool,
}

#[derive(Default)]
pub(super) struct TextureAssets {
    pub replaced: HashMap<String, Vec<u8>>,
    pub added: BTreeMap<String, Vec<u8>>,
}

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
    texture_label(&texture)
}

fn texture_label<'a>(texture: &Fields<'a>) -> Option<&'a str> {
    let original = std::str::from_utf8(texture.get(b"qsFileNameUTF8", BYTE_TYPE).ok()?).ok()?;
    Path::new(original.rsplit(['/', '\\']).next()?).file_stem()?.to_str()
}

fn texture_vector(texture: &Fields<'_>, key: &[u8]) -> Option<[f64; 2]> {
    let bytes = texture.get(key, 10).ok()?;
    let values = [
        f64::from(f32::from_le_bytes(bytes.get(..4)?.try_into().ok()?)),
        f64::from(f32::from_le_bytes(bytes.get(4..)?.try_into().ok()?)),
    ];
    values.iter().all(|value| value.is_finite()).then_some(values)
}

fn texture_mapping(texture: &Fields<'_>) -> Option<([f64; 2], f64, [f64; 2])> {
    if integer(texture, b"enMappingType") != Ok(0) {
        return None;
    }
    let size = texture_vector(texture, b"v2PhysicalSize")?;
    let raw = texture.get(b"fSignedAngle", 6).ok()?.try_into().ok()?;
    let angle = f64::from(f32::from_le_bytes(raw));
    let translation = texture_vector(texture, b"v2Translation")?;
    (angle.is_finite() && size.iter().all(|value| *value > 0.0))
        .then_some((size, angle, translation))
}

pub(super) fn front_desaturated(material: &Fields<'_>) -> Result<bool, String> {
    let (faces, legacy_front) = face_records(material)?;
    for (index, face) in faces.iter().enumerate() {
        if face_type(face, index, legacy_front)? == 0 {
            return Ok(face.get(b"fDesaturated", 6).ok()
                .and_then(|raw| raw.try_into().ok())
                .is_some_and(|raw| f32::from_le_bytes(raw) == 1.0));
        }
    }
    Ok(false)
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

fn front_texture_refs<'a>(
    material: &Fields<'a>,
) -> Result<Vec<(&'static str, &'a str, Fields<'a>)>, String> {
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
                    refs.push((*key, name, texture));
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
    for (key, name, _) in front_texture_refs(material)? {
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
    let mut hydrate = |entry: &mut Value, material: &Fields<'_>| -> Result<(), String> {
        let (faces, legacy) = face_records(material)?;
        if let Some((_, face)) = faces.iter().enumerate()
            .find(|(index, face)| face_type(face, *index, legacy) == Ok(0))
        {
            if let Ok(intensity) = integer(face, b"iNormalIntensityInPercentage") {
                entry["native_normal_strength"] = json!(f64::from(intensity) / 100.0);
            }
        }
        if front_desaturated(material)? {
            entry["base_texture_desaturated"] = json!(true);
        }
        for (key, name, texture) in front_texture_refs(material)? {
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
            entry["texture_maps"][key] = json!(cached);
            if let Some(label) = texture_label(&texture) {
                entry["native_texture_names"][key] = json!(label);
            }
            if let Some((size, angle, translation)) = texture_mapping(&texture) {
                entry["native_texture_sizes_mm"][key] = json!(size);
                if angle != 0.0 || translation != [0.0; 2] {
                    entry["native_texture_transforms"][key] =
                        json!({"angle_degrees": angle, "translation_mm": translation});
                }
            }
        }
        Ok(())
    };
    for (index, fabric) in fabrics.iter().enumerate() {
        hydrate(&mut state["fabrics"][index], &fabric.map(b"mapMaterial2D")?)?;
    }
    let names = state["colorways"].clone();
    if names.as_array().is_some_and(|names| names.len() > 1) {
        let variants = (|| -> Result<Vec<(Vec<Value>, Vec<u32>)>, String> {
            let variants = colorway_materials(&root, &fabrics)?;
            let mut output = vec![(Vec::new(), Vec::new()); fabrics.len()];
            for materials in variants {
                for (index, colorway) in materials.iter().enumerate() {
                    let mut entry = material(&fabrics[index], &colorway.material)?;
                    entry["index"] = json!(index);
                    hydrate(&mut entry, &colorway.material)?;
                    output[index].0.push(entry);
                    output[index].1.push(colorway.chip);
                }
            }
            Ok(output)
        })();
        match variants {
            Ok(variants) => {
                for (index, (values, chips)) in variants.into_iter().enumerate() {
                    state["fabrics"][index]["native_colorway_materials"] =
                        json!({"names": names, "values": values, "chips": chips});
                }
            }
            Err(error) => warnings.push(error),
        }
    }
    state["native_texture_warnings"] = json!(warnings);
    Ok(())
}

struct Edit {
    kind: i32,
    image: Option<Vec<u8>>,
    new_asset: Option<(String, String)>,
    matches: RefCell<Vec<String>>,
    desaturated: bool,
}

fn update_face(face: &Fields<'_>, edits: &Vec<Edit>) -> Result<Option<Vec<u8>>, String> {
    if integer(face, b"enMaterialFaceType")? != 0 {
        return Ok(None);
    }
    let list = textures(face)?;
    let mut desaturation = None;
    let mut converted = HashMap::new();
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for edit in edits {
        let mut found = None;
        for (index, texture) in list.iter().enumerate() {
            if integer(texture, b"enType")? != edit.kind {
                continue;
            }
            if edit.image.is_none() {
                removed.push(index);
                if edit.kind == 0 {
                    desaturation = Some(false);
                }
                continue;
            }
            let archive_name = filename(texture)?;
            let name = edit.new_asset.as_ref().map_or(archive_name, |(asset, _)| asset);
            let image_type = integer(texture, b"enImageType")?;
            if !matches!(image_type, 2 | 5)
                || !Path::new(archive_name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                return Err("native texture replacement requires a PNG archive slot".into());
            }
            if found.replace(name).is_some() {
                return Err("native texture references are not uniquely paired".into());
            }
            let mut record = if image_type == 2 {
                Some(texture.replace(b"enImageType", 4, &5i32.to_le_bytes())?)
            } else {
                None
            };
            if let Some((asset, label)) = &edit.new_asset {
                let bytes = record.as_deref().unwrap_or_else(|| texture.encoded());
                let mut updated = label_texture(&Fields::read(bytes)?, label, asset)?;
                let size = image_physical_size(edit.image.as_deref().ok_or("texture image is missing")?)?;
                updated = Fields::read(&updated)?.replace(b"v2Size", 10, &size)?;
                updated = Fields::read(&updated)?.replace(b"v2PhysicalSize", 10, &size)?;
                record = Some(updated);
            }
            if let Some(record) = record {
                converted.insert(index, record);
            }
        }
        if let Some(image) = &edit.image {
            let name = match (found, &edit.new_asset) {
                (Some(name), _) => name,
                (None, Some((asset, label))) => {
                    added.push((edit.kind, Cow::Owned(new_texture_record(
                        face, edit.kind, label, asset, image,
                    )?)));
                    asset
                }
                _ => return Err("native save requires an existing texture slot on the fabric".into()),
            };
            edit.matches.borrow_mut().push(name.to_owned());
            if edit.kind == 0 {
                desaturation = Some(edit.desaturated);
            }
        }
    }
    if desaturation.is_none() && converted.is_empty() && removed.is_empty() && added.is_empty() {
        return Ok(None);
    }
    let mut updated = if let Some(desaturated) = desaturation {
        let flag = if desaturated { 1.0f32 } else { 0.0 };
        face.replace(b"fDesaturated", 6, &flag.to_le_bytes())?
    } else {
        face.encoded().to_vec()
    };
    if added.iter().any(|(kind, _)| *kind == 4) {
        // CLO enables normal mapping on the face when it adds the first normal image.
        updated = Fields::read(&updated)?.replace(b"fBumpMode", 6, &2.0f32.to_le_bytes())?;
        updated = Fields::read(&updated)?.replace(b"iNormalIntensityInPercentage", 4, &10i32.to_le_bytes())?;
    }
    if !converted.is_empty() || !removed.is_empty() || !added.is_empty() {
        let mut textures = list
            .iter()
            .enumerate()
            .filter(|(index, _)| !removed.contains(index))
            .map(|(index, texture)| {
                let data = converted
                    .remove(&index)
                    .map_or(Cow::Borrowed(texture.encoded()), Cow::Owned);
                Ok((integer(texture, b"enType")?, data))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if !added.is_empty() {
            textures.extend(added);
            textures.sort_by_key(|(kind, _)| *kind);
        }
        let textures: Vec<_> = textures.into_iter().map(|(_, data)| data).collect();
        updated = Fields::read(&updated)?.replace(
            b"listTexture",
            LIST_TYPE,
            &encoded_list(&textures)?,
        )?;
    }
    Ok(Some(updated))
}

fn image_physical_size(image: &[u8]) -> Result<Vec<u8>, String> {
    let info = png::Decoder::new(Cursor::new(image)).read_info().map_err(|e| e.to_string())?;
    let info = info.info();
    let density = info.pixel_dims.filter(|dims| dims.unit == png::Unit::Meter)
        .map(|dims| [dims.xppu, dims.yppu]).unwrap_or([2835; 2]);
    Ok([info.width, info.height].into_iter().zip(density)
        .flat_map(|(pixels, per_meter)| {
            let per_meter = if per_meter == 0 { 2835 } else { per_meter };
            (pixels as f32 * 1000.0 / per_meter as f32).to_le_bytes()
        }).collect())
}

fn new_texture_record(
    face: &Fields<'_>, kind: i32, label: &str, asset: &str, image: &[u8],
) -> Result<Vec<u8>, String> {
    let size = image_physical_size(image)?;
    let white: Vec<u8> = [1.0f32; 4].into_iter().flat_map(f32::to_le_bytes).collect();
    let color = encode_map(&[
        (b"qsPLMIdUTF8", BYTE_TYPE, b""), (b"v4Color", 12, &white),
        (b"qsName", BYTE_TYPE, b""), (b"qsPLMId", BYTE_TYPE, b""),
        (b"qsNameUTF8", BYTE_TYPE, b""),
    ])?;
    let zero = 0i32.to_le_bytes();
    let minus_one = (-1i32).to_le_bytes();
    let repeat = 10497i32.to_le_bytes();
    let kind_bytes = kind.to_le_bytes();
    let one = 1.0f32.to_le_bytes();
    let min_filter = 9987i32.to_le_bytes();
    let mag_filter = 9729i32.to_le_bytes();
    let millimeters_to_meters = 0.001f32.to_le_bytes();
    let mut fields: Vec<(&[u8], u8, &[u8])> = vec![
        (b"v2Translation", 10, &[0; 8]),
        (b"qsFileName", BYTE_TYPE, label.as_bytes()),
        (b"bAIFindBR", 1, &[0]), (b"enWrapS", 4, &repeat),
        (b"qsFileNameUTF8", BYTE_TYPE, label.as_bytes()),
        (b"qsAIFileName", BYTE_TYPE, b""), (b"qsAIFileNameUTF8", BYTE_TYPE, b""),
        (b"isSubstanceExtract", 1, &[0]), (b"qsAIFileImageType", 4, &zero),
        (b"qsAIFileLayerNo", 4, &minus_one), (b"qsAIFilePageNo", 4, &minus_one),
        (b"enImageType", 4, &[5, 0, 0, 0]), (b"bUseManualOriginColor", 1, &[0]),
        (b"qsUniqueFileName", BYTE_TYPE, asset.as_bytes()),
        (b"qsUniqueFileNameUTF8", BYTE_TYPE, asset.as_bytes()),
        (b"bIsJeanFile", 1, &[0]), (b"enType", 4, &kind_bytes),
        (b"fIntensity", 6, &one), (b"enWrapT", 4, &repeat),
        (b"enMinFilter", 4, &min_filter), (b"enMagFilter", 4, &mag_filter),
        (b"v4BorderColor", 12, &[0; 16]), (b"bUseOverallColorSwitch", 1, &[0]),
        (b"enMappingType", 4, &zero), (b"fSignedAngle", 6, &zero),
        (b"bAppliedTransformToThumbnail", 1, &[0]), (b"bColorInverted", 1, &[1]),
        (b"iAlphaMode", 4, &zero), (b"v2Size", 10, &size),
        (b"bButtonUnified", 1, &[1]), (b"v2PhysicalSize", 10, &size),
        (b"v2UvSize", 10, &white[..8]), (b"fUnifiedScale", 6, &millimeters_to_meters),
        (b"listLayer", LIST_TYPE, &zero), (b"bCanvasTransparency", 1, &[1]),
        (b"mapOverallColorSwitchColor", MAP_TYPE, &color),
        (b"mapManualOriginColor", MAP_TYPE, &color), (b"bTileTexture", 1, &[0]),
    ];
    if kind == 5 {
        for (key, field_type) in [
            (b"fDisplacementAmount".as_slice(), 6),
            (b"fDisplacementEdgeLength", 6),
            (b"fDisplacementShift", 6),
            (b"bDisplacementKeepContinuity", 1),
            (b"fDisplacementWaterLevel", 6),
        ] {
            fields.push((key, field_type, face.get(key, field_type)?));
        }
    }
    encode_map(&fields)
}

fn label_texture(texture: &Fields<'_>, label: &str, asset: &str) -> Result<Vec<u8>, String> {
    let data = texture.replace(b"qsFileName", BYTE_TYPE, label.as_bytes())?;
    let data = Fields::read(&data)?.replace(b"qsFileNameUTF8", BYTE_TYPE, label.as_bytes())?;
    // The source label and the unique ZIP member name serve different callers.
    let data = Fields::read(&data)?.set(b"qsUniqueFileName", BYTE_TYPE, asset.as_bytes())?;
    Fields::read(&data)?.set(b"qsUniqueFileNameUTF8", BYTE_TYPE, asset.as_bytes())
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
    mut occupied: HashSet<String>,
) -> Result<(Option<Vec<u8>>, TextureAssets), String> {
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
            let (image, new_asset) = if current.is_null() && explicit.is_some_and(|edit| edit["clear"] == true) {
                (None, None)
            } else {
                let path = current.as_str().filter(|path| !path.is_empty())
                    .ok_or("native texture removal requires an explicit texture edit")?;
                let path = Path::new(path);
                let new_asset = if explicit.is_some() && baseline[index][key].is_null() {
                    Some(allocate_asset(path, &mut occupied)?)
                } else {
                    None
                };
                (Some(crate::compat_fit::png_for_export(path, false)?), new_asset)
            };
            image_bytes += image.as_ref().map_or(0, Vec::len);
            if image_bytes > 256 * 1024 * 1024 {
                return Err("replacement texture assets exceed 256 MiB".into());
            }
            edits.entry(index).or_insert_with(Vec::new).push(Edit {
                kind,
                image,
                new_asset,
                matches: RefCell::new(Vec::new()),
                desaturated: fabric["base_texture_desaturated"] == true,
            });
        }
    }
    apply_edits(pac, edits, false)
}

fn apply_edits(
    pac: &[u8],
    edits: HashMap<usize, Vec<Edit>>,
    multiple: bool,
) -> Result<(Option<Vec<u8>>, TextureAssets), String> {
    let updated = edit_materials_in_active_colorway(pac, &edits, update_face, multiple)?;
    let mut images = TextureAssets::default();
    for edit in edits.into_values().flatten() {
        let Some(image) = edit.image else { continue };
        let matches = edit.matches.into_inner();
        let name = matches
            .first()
            .ok_or("native texture binding was not found")?;
        if matches.len() < 4
            || matches.iter().any(|candidate| candidate != name)
            || edit.new_asset.is_none() && occurrences(pac, name.as_bytes()) != matches.len() * 2
        {
            return Err(
                "native texture asset is shared or its references could not be verified".into(),
            );
        }
        let previous = if edit.new_asset.is_some() {
            images.added.insert(name.clone(), image)
        } else {
            images.replaced.insert(name.clone(), image)
        };
        if previous.is_some() {
            return Err("native texture asset is shared by edited materials".into());
        }
    }
    Ok((updated, images))
}

fn allocate_asset(path: &Path, occupied: &mut HashSet<String>) -> Result<(String, String), String> {
    let label = path.file_name().and_then(|name| name.to_str())
        .ok_or("native texture replacement needs a UTF-8 filename")?;
    let mut next = occupied.len();
    loop {
        let asset = format!("clo-rust-texture-{next}.png");
        if occupied.insert(asset.clone()) {
            return Ok((asset, label.to_owned()));
        }
        next += 1;
    }
}

pub(super) fn replace_colorways(
    pac: &[u8],
    state: &Value,
    targets: &[ColorwayTextureEdit],
    active: u32,
    mut occupied: HashSet<String>,
) -> Result<(Vec<u8>, BTreeMap<String, Vec<u8>>), String> {
    let (_, mut current) = read_colorways(pac)?;
    let fabrics = super::super::array(state, "fabrics");
    let mut edits = BTreeMap::<usize, HashMap<usize, Vec<Edit>>>::new();
    let mut image_bytes = 0;
    for target in targets {
        let material = super::super::fabric_for_colorway(
            state, fabrics.get(target.fabric).ok_or("native fabric index no longer exists")?,
            target.colorway,
        )?;
        let (_, kind) = TYPES.iter().find(|(key, _)| *key == target.map)
            .ok_or("unknown native texture map")?;
        let (image, new_asset) = if target.clear {
            (None, None)
        } else {
            let path = material["texture_maps"][&target.map].as_str()
                .ok_or("native texture replacement is missing its image")?;
            let path = Path::new(path);
            let asset = allocate_asset(path, &mut occupied)?;
            (Some(crate::compat_fit::png_for_export(path, false)?), Some(asset))
        };
        image_bytes += image.as_ref().map_or(0, Vec::len);
        if image_bytes > 256 * 1024 * 1024 {
            return Err("replacement texture assets exceed 256 MiB".into());
        }
        edits.entry(target.colorway).or_default().entry(target.fabric).or_default().push(Edit {
            kind: *kind, image, new_asset, matches: RefCell::new(Vec::new()),
            desaturated: material["base_texture_desaturated"] == true,
        });
    }
    let mut result = Cow::Borrowed(pac);
    let mut images = BTreeMap::new();
    for (colorway, edits) in edits {
        if colorway != current {
            result = Cow::Owned(select_colorway(&result, colorway as u32)?);
            current = colorway;
        }
        let (updated, assets) = apply_edits(&result, edits, true)?;
        if let Some(updated) = updated {
            result = Cow::Owned(updated);
        }
        for (name, data) in assets.added {
            if images.insert(name, data).is_some() {
                return Err("native texture replacement asset is shared by edited colorways".into());
            }
        }
    }
    if current != active as usize {
        result = Cow::Owned(select_colorway(&result, active)?);
    }
    Ok((result.into_owned(), images))
}
