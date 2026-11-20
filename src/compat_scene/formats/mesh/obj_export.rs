use super::{fabric_base_color, write, Mesh};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn sidecar(parent: &Path) -> Result<(PathBuf, File), String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("clo-obj-{}-{id}.mtl", std::process::id()));
        match File::create_new(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
}

fn write_bundle(
    path: &Path,
    meshes: &[Mesh],
    fabrics: &[Value],
    mtl_path: &Path,
    mut mtl_file: File,
    named_sidecar: bool,
    created: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let stem = mtl_path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("OBJ material filename must be valid UTF-8")?;
    let mut obj = format!("# CLO Rust, millimetres, Y up\nmtllib {stem}.mtl\n");
    let mut mtl = String::new();
    let fabric_by_index: HashMap<_, _> = fabrics
        .iter()
        .filter_map(|fabric| Some((fabric["index"].as_u64()?, fabric)))
        .collect();
    let mut materials = HashMap::new();
    let mut vertex_offset = 1u64;
    let mut uv_offset = 1u64;
    for mesh in meshes {
        let has_uv = !mesh.texcoords.is_empty();
        let fabric = mesh
            .fabric_index
            .as_u64()
            .and_then(|id| fabric_by_index.get(&id));
        let texture = fabric.and_then(|fabric| {
            let path = fabric["texture_maps"]["base"].as_str()?;
            (has_uv && !path.is_empty()).then_some((fabric, path))
        });
        let key = (
            fabric.and_then(|fabric| fabric["index"].as_u64()),
            texture.is_some(),
        );
        let id = if let Some(id) = materials.get(&key) {
            *id
        } else {
            let id = materials.len();
            let [r, g, b, alpha] = fabric.map_or([1.0; 4], |value| fabric_base_color(value));
            let _ = writeln!(
                mtl,
                "newmtl material_{id}\nKd {r} {g} {b}\nd {alpha}\nillum 1"
            );
            if let Some((fabric, source)) = texture {
                let png = crate::compat_fit::png_for_export(
                    Path::new(source), fabric["base_texture_desaturated"] == true,
                )?;
                let name = if named_sidecar {
                    super::write_colorway_image(path, fabric, &png)?
                } else {
                    let name = format!("{stem}-{id}.png");
                    let target = mtl_path.with_file_name(&name);
                    let mut file = File::create_new(&target)
                        .map_err(|error| format!("{}: {error}", target.display()))?;
                    created.push(target);
                    file.write_all(&png)
                        .and_then(|_| file.sync_all())
                        .map_err(|error| format!("write OBJ texture {name}: {error}"))?;
                    name
                };
                let _ = writeln!(mtl, "map_Kd {name}");
            }
            mtl.push('\n');
            materials.insert(key, id);
            id
        };

        let name = mesh.name.replace(['\n', '\r'], " ");
        let _ = writeln!(obj, "o {name}\nusemtl material_{id}");
        for [x, y, z] in &mesh.positions {
            let _ = writeln!(obj, "v {x:.9} {y:.9} {z:.9}");
        }
        for [u, v] in &mesh.texcoords {
            let _ = writeln!(obj, "vt {u:.9} {:.9}", 1.0 - v);
        }
        for face in &mesh.triangles {
            obj.push('f');
            for &index in face {
                let index = u64::from(index);
                if has_uv {
                    let _ = write!(obj, " {}/{}", vertex_offset + index, uv_offset + index);
                } else {
                    let _ = write!(obj, " {}", vertex_offset + index);
                }
            }
            obj.push('\n');
        }
        vertex_offset += mesh.positions.len() as u64;
        uv_offset += mesh.texcoords.len() as u64;
    }
    mtl_file
        .write_all(mtl.as_bytes())
        .and_then(|_| mtl_file.sync_all())
        .map_err(|error| format!("{}: {error}", mtl_path.display()))?;
    // Expose the OBJ only after its references are complete. Every sidecar is
    // owned by this export, so failure can remove it without touching an older export.
    write(path, obj.as_bytes())
}

pub(super) fn export(
    path: &Path,
    meshes: &[Mesh],
    fabrics: &[Value],
    named_sidecar: bool,
) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let (mtl_path, file) = if named_sidecar {
        let mtl_path = path.with_extension("mtl");
        let file = File::create_new(&mtl_path)
            .map_err(|error| format!("{}: {error}", mtl_path.display()))?;
        (mtl_path, file)
    } else {
        sidecar(parent)?
    };
    // Keep a relative request relative, including the returned companion path.
    let mtl_path = path.with_file_name(mtl_path.file_name().expect("generated MTL basename"));
    let mut created = vec![mtl_path.clone()];
    if let Err(error) = write_bundle(path, meshes, fabrics, &mtl_path, file, named_sidecar, &mut created) {
        for sidecar in created {
            let _ = fs::remove_file(sidecar);
        }
        return Err(error);
    }
    Ok(mtl_path)
}
