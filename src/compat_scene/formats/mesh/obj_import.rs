use super::{filename, Mesh};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

type Corner = (u32, Option<u32>);
type Face = [Corner; 3];

struct Part {
    name: String,
    material: Option<String>,
    has_uv: bool,
    faces: Vec<Face>,
}

fn index(term: &str, count: usize, kind: &str) -> Result<u32, String> {
    let value: i64 = term
        .parse()
        .map_err(|_| format!("invalid OBJ {kind} index"))?;
    let absolute = if value > 0 {
        value - 1
    } else {
        count as i64 + value
    };
    if value == 0 || absolute < 0 || absolute >= count as i64 {
        return Err(format!("OBJ {kind} index is out of range"));
    }
    u32::try_from(absolute).map_err(|_| format!("OBJ has too many {kind} indices"))
}

fn coordinates<'a, const N: usize>(
    mut words: impl Iterator<Item = &'a str>,
    kind: &str,
) -> Result<[f64; N], String> {
    let mut values = [0.0; N];
    for value in &mut values {
        *value = words
            .next()
            .ok_or_else(|| format!("incomplete {kind}"))?
            .parse::<f64>()
            .map_err(|_| format!("invalid {kind}"))?;
        if !value.is_finite() {
            return Err(format!("non-finite {kind}"));
        }
    }
    Ok(values)
}

fn payload(line: &str) -> (&str, &str) {
    let line = line.split('#').next().unwrap_or("").trim();
    line.split_once(char::is_whitespace)
        .map_or((line, ""), |(key, words)| (key, words.trim()))
}

fn unquote(path: &str) -> &str {
    path.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(path)
}

fn read_library(path: &Path) -> Result<String, String> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    if !path.is_file() {
        return Err(format!(
            "OBJ material library is unavailable: {}",
            path.display()
        ));
    }
    let mut data = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(LIMIT + 1).read_to_string(&mut data))
        .map_err(|error| format!("read OBJ material library {}: {error}", path.display()))?;
    if data.len() as u64 > LIMIT {
        return Err("OBJ material library exceeds 16 MiB".into());
    }
    Ok(data)
}

fn read_materials(files: &[PathBuf], warnings: &mut Vec<String>) -> HashMap<String, Value> {
    let mut materials = HashMap::new();
    for file in files {
        let data = match read_library(file) {
            Ok(data) => data,
            Err(error) => {
                warnings.push(error);
                continue;
            }
        };
        let parent = file.parent().unwrap_or_else(|| Path::new("."));
        let mut current = None;
        for line in data.lines() {
            let (key, words) = payload(line);
            if key == "newmtl" {
                current = (!words.is_empty()).then_some(words);
                if let Some(name) = current {
                    materials.insert(
                        name.to_owned(),
                        json!({
                            "name": name, "color": [1.0, 1.0, 1.0], "rgba": [1.0, 1.0, 1.0, 1.0],
                            "texture_maps": {}, "info": {"source": "wavefront_mtl"}
                        }),
                    );
                }
                continue;
            }
            let Some(material) = current.and_then(|name| materials.get_mut(name)) else {
                continue;
            };
            match key {
                "Kd" => {
                    if let Ok(rgb) = coordinates::<3>(words.split_whitespace(), "MTL diffuse color")
                    {
                        let [r, g, b] = rgb.map(|value| value.clamp(0.0, 1.0));
                        let alpha = material["rgba"][3].as_f64().unwrap_or(1.0);
                        material["color"] = json!([r, g, b]);
                        material["rgba"] = json!([r, g, b, alpha]);
                    }
                }
                "d" | "Tr" => {
                    if let Ok([opacity]) = coordinates::<1>(words.split_whitespace(), "MTL opacity")
                    {
                        let alpha = if key == "Tr" { 1.0 - opacity } else { opacity };
                        material["rgba"][3] = json!(alpha.clamp(0.0, 1.0));
                    }
                }
                "map_Kd" => {
                    let reference = unquote(words);
                    let texture = parent.join(reference);
                    if !words.starts_with('-')
                        && texture
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                        && texture.is_file()
                    {
                        match texture.canonicalize() {
                            Ok(texture) => material["texture_maps"]["base"] = json!(texture),
                            Err(error) => {
                                warnings.push(format!("resolve OBJ base texture: {error}"))
                            }
                        }
                    } else {
                        warnings.push(format!(
                            "OBJ base texture is unavailable or unsupported: {reference}"
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    materials
}

pub(super) fn read(
    path: &Path,
    load_materials: bool,
) -> Result<(Vec<Mesh>, Vec<Value>, Vec<String>), String> {
    let data = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut positions = Vec::new();
    let mut texcoords = Vec::new();
    let mut parts: Vec<Part> = Vec::new();
    let mut libraries = Vec::new();
    let mut name = filename(path).to_owned();
    let mut material = None;
    let mut new_part = true;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for line in data.lines() {
        let (key, words) = payload(line);
        match key {
            "v" => positions.push(coordinates::<3>(words.split_whitespace(), "OBJ vertex")?),
            "vt" => {
                let mut values = words.split_whitespace();
                let [u] = coordinates::<1>(&mut values, "OBJ UV")?;
                let [v] =
                    coordinates::<1>(std::iter::once(values.next().unwrap_or("0")), "OBJ UV")?;
                texcoords.push([u, 1.0 - v]);
            }
            "o" | "g" => {
                name = words.to_owned();
                new_part = true;
            }
            "usemtl" if load_materials => material = (!words.is_empty()).then_some(words),
            "mtllib" if load_materials && !words.is_empty() => {
                let exact = parent.join(unquote(words));
                if exact.is_file() || words.starts_with('"') {
                    libraries.push(exact);
                } else {
                    libraries.extend(words.split_whitespace().map(|name| parent.join(name)));
                }
            }
            "f" => {
                let polygon = words
                    .split_whitespace()
                    .map(|word| {
                        let mut fields = word.split('/');
                        let vertex = index(fields.next().unwrap_or(""), positions.len(), "vertex")?;
                        let uv = fields
                            .next()
                            .filter(|field| !field.is_empty())
                            .map(|field| index(field, texcoords.len(), "UV"))
                            .transpose()?;
                        Ok((vertex, uv))
                    })
                    .collect::<Result<Vec<Corner>, String>>()?;
                if polygon.len() < 3 {
                    return Err("OBJ face has fewer than three vertices".into());
                }
                let has_uv = polygon.iter().all(|corner| corner.1.is_some());
                if new_part
                    || !parts.last().is_some_and(|part| {
                        part.name == name
                            && part.material.as_deref() == material
                            && part.has_uv == has_uv
                    })
                {
                    parts.push(Part {
                        name: name.clone(),
                        material: material.map(str::to_owned),
                        has_uv,
                        faces: Vec::new(),
                    });
                }
                new_part = false;
                let part = parts.last_mut().expect("OBJ face has a part");
                for i in 1..polygon.len() - 1 {
                    part.faces.push([polygon[0], polygon[i], polygon[i + 1]]);
                }
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        return Err("OBJ has no triangle geometry".into());
    }

    let mut warnings = Vec::new();
    let materials = read_materials(&libraries, &mut warnings);
    let mut fabrics = Vec::new();
    let mut fabric_ids = HashMap::new();
    let mut meshes = Vec::new();
    for part in parts {
        let fabric_index = if let Some(name) = part.material {
            if let Some(fabric) = materials.get(&name) {
                let id = *fabric_ids.entry(name).or_insert_with(|| {
                    let id = fabrics.len();
                    let mut fabric = fabric.clone();
                    fabric["index"] = json!(id);
                    fabrics.push(fabric);
                    id
                });
                json!(id)
            } else {
                let warning = format!("OBJ material is undefined: {name}");
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
                Value::Null
            }
        } else {
            Value::Null
        };
        let mut mesh = Mesh {
            rest_mm_uv: false,
            id: Value::Null,
            name: part.name,
            fabric_index,
            texcoords: Vec::new(),
            positions: Vec::new(),
            triangles: Vec::new(),
        };
        let mut mapping = HashMap::new();
        for face in part.faces {
            let mut triangle = [0; 3];
            for (target, (vertex, uv)) in triangle.iter_mut().zip(face) {
                let uv = uv.filter(|_| part.has_uv);
                let next = u32::try_from(mesh.positions.len())
                    .map_err(|_| "OBJ has too many mesh vertices")?;
                *target = *mapping.entry((vertex, uv)).or_insert_with(|| {
                    mesh.positions.push(positions[vertex as usize]);
                    if let Some(uv) = uv {
                        mesh.texcoords.push(texcoords[uv as usize]);
                    }
                    next
                });
            }
            mesh.triangles.push(triangle);
        }
        mesh.validate()?;
        meshes.push(mesh);
    }
    Ok((meshes, fabrics, warnings))
}
