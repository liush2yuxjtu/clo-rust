use super::{
    array,
    solver::{self, cross, dot, sub},
    string,
    texture::BaseTexture,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

type Point = [f64; 3];
type Rgb = [u8; 3];
const SIZE: [usize; 2] = [800, 700];
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Drawing {
    points: Vec<Point>,
    faces: Vec<[usize; 3]>,
    color: Rgb,
    texcoords: Vec<[f64; 2]>,
    texture: Option<Rc<BaseTexture>>,
}

#[derive(Default)]
struct TextureCache {
    by_path: BTreeMap<String, Option<Rc<BaseTexture>>>,
    warnings: Vec<String>,
}

impl TextureCache {
    fn load(&mut self, path: &str) -> Option<Rc<BaseTexture>> {
        self.by_path
            .entry(path.to_owned())
            .or_insert_with(|| match BaseTexture::load(Path::new(path)) {
                Ok(texture) => Some(Rc::new(texture)),
                Err(error) => {
                    self.warnings
                        .push(format!("Base texture {path} was skipped: {error}"));
                    None
                }
            })
            .clone()
    }

    fn apply(&mut self, state: &Value, id: &Value, fallback: usize, drawing: &mut Drawing) {
        let fabric = array(&state["fabrics"])
            .iter()
            .find(|f| !id.is_null() && f["index"] == *id);
        drawing.color = fabric
            .and_then(|f| serde_json::from_value::<[f64; 3]>(f["color"].clone()).ok())
            .map(|c| c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8))
            .unwrap_or(
                [
                    [95, 151, 193],
                    [138, 122, 177],
                    [151, 172, 116],
                    [197, 135, 114],
                ][fallback % 4],
            );
        if let Some(path) = fabric.and_then(|f| f["texture_maps"]["base"].as_str()) {
            if drawing.texcoords.len() == drawing.points.len() {
                drawing.texture = self.load(path);
            } else {
                let message="Base texture was skipped for a saved mesh without rest UVs. Simulate again to generate them.";
                if !self.warnings.iter().any(|warning| warning == message) {
                    self.warnings.push(message.into());
                }
            }
        }
    }
}

fn pieces(state: &Value, flat: bool, textures: &mut TextureCache) -> Result<Vec<Drawing>, String> {
    let cached = array(&state["simulation"]["meshes"]);
    if flat || cached.is_empty() {
        let meshes = if flat {
            solver::layout_meshes(state)?
        } else {
            solver::initial_meshes(state)?
        };
        return Ok(meshes
            .into_iter()
            .enumerate()
            .filter(|(_, mesh)| flat || !hidden(state, &mesh.id))
            .map(|(index, mesh)| {
                let mut drawing = Drawing {
                    points: mesh.positions,
                    faces: mesh.triangles,
                    color: [0; 3],
                    texcoords: mesh.texcoords,
                    texture: None,
                };
                textures.apply(state, &mesh.fabric_index, index, &mut drawing);
                drawing
            })
            .collect());
    }
    cached
        .iter()
        .enumerate()
        .filter(|(_, mesh)| !hidden(state, &mesh["id"]))
        .map(|(index, mesh)| {
            let fabric_id = array(&state["patterns"])
                .iter()
                .find(|p| p["id"] == mesh["id"])
                .map_or(&mesh["fabric_index"], |p| &p["fabric_index"]);
            let mut drawing = Drawing {
                color: [0; 3],
                points: serde_json::from_value(mesh["positions"].clone())
                    .map_err(|e| format!("invalid saved mesh positions: {e}"))?,
                faces: serde_json::from_value(mesh["triangles"].clone())
                    .map_err(|e| format!("invalid saved mesh faces: {e}"))?,
                texcoords: serde_json::from_value(mesh["texcoords"].clone()).unwrap_or_default(),
                texture: None,
            };
            textures.apply(state, fabric_id, index, &mut drawing);
            Ok(drawing)
        })
        .collect()
}

fn hidden(state: &Value, id: &Value) -> bool {
    array(&state["patterns"])
        .iter()
        .find(|p| p["id"] == *id)
        .and_then(|p| p["hide"].as_bool())
        .unwrap_or(false)
}

struct Camera {
    horizontal: Point,
    up: Point,
    depth: Point,
}

fn camera(view: &str) -> Result<Camera, String> {
    let angle = match view {
        "front" | "2d" => 0.0,
        "back" => 180.0,
        "left" => -90.0,
        "right" => 90.0,
        "3/4-right" => 42.0,
        "3/4-left" => -42.0,
        "top" => 0.0,
        _ => return Err(format!("unknown snapshot view {view:?}")),
    };
    if view == "top" {
        return Ok(Camera {
            horizontal: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, -1.0],
            depth: [0.0, 1.0, 0.0],
        });
    }
    let (sin, cos) = f64::to_radians(angle).sin_cos();
    Ok(Camera {
        horizontal: [cos, 0.0, -sin],
        up: [0.0, 1.0, 0.0],
        depth: [sin, 0.0, cos],
    })
}

fn project(p: Point, camera: &Camera) -> Point {
    [
        dot(p, camera.horizontal),
        dot(p, camera.up),
        dot(p, camera.depth),
    ]
}

struct Canvas {
    pixels: Vec<u8>,
    depth: Vec<f64>,
}

impl Canvas {
    fn new(flat: bool) -> Self {
        let [w, h] = SIZE;
        let mut pixels = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let shade = if flat {
                    if y % 24 == 0 || x % 24 == 0 {
                        232
                    } else {
                        247
                    }
                } else {
                    (250 - y * 20 / h) as u8
                };
                pixels.extend([shade, shade, shade.saturating_add(2)]);
            }
        }
        Self {
            pixels,
            depth: vec![f64::NEG_INFINITY; w * h],
        }
    }

    fn triangle(
        &mut self,
        [a, b, c]: [Point; 3],
        color: Rgb,
        texture: Option<(&BaseTexture, [[f64; 2]; 3])>,
        flat: bool,
    ) {
        let [w, h] = SIZE;
        let signed = |a: Point, b: Point, x: f64, y: f64| {
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
        };
        let area = signed(a, b, c[0], c[1]);
        if area.abs() < 1e-10 {
            return;
        }
        let normal = cross(sub(b, a), sub(c, a));
        let factor = if flat {
            0.95
        } else {
            let n = dot(normal, normal).sqrt().max(1e-15);
            let light = [-0.43, -0.49, 0.76];
            0.67 + 0.33 * (dot(normal, light) / n).abs()
        };
        let shade = |rgb: Rgb| rgb.map(|c| (f64::from(c) * factor).min(255.0) as u8);
        let flat_rgb = shade(color);
        let x0 = a[0].min(b[0]).min(c[0]).floor().clamp(0.0, (w - 1) as f64) as usize;
        let x1 = a[0].max(b[0]).max(c[0]).ceil().clamp(0.0, (w - 1) as f64) as usize;
        let y0 = a[1].min(b[1]).min(c[1]).floor().clamp(0.0, (h - 1) as f64) as usize;
        let y1 = a[1].max(b[1]).max(c[1]).ceil().clamp(0.0, (h - 1) as f64) as usize;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let wx = x as f64 + 0.5;
                let wy = y as f64 + 0.5;
                let u = signed(b, c, wx, wy) / area;
                let v = signed(c, a, wx, wy) / area;
                let t = 1.0 - u - v;
                if u < 0.0 || v < 0.0 || t < 0.0 {
                    continue;
                }
                let depth = u * a[2] + v * b[2] + t * c[2];
                let index = y * w + x;
                if depth >= self.depth[index] {
                    self.depth[index] = depth;
                    let rgb = texture.map_or(flat_rgb, |(texture, coordinates)| {
                        let uv = std::array::from_fn(|i| {
                            u * coordinates[0][i] + v * coordinates[1][i] + t * coordinates[2][i]
                        });
                        shade(texture.sample(uv, color))
                    });
                    self.pixels[3 * index..3 * index + 3].copy_from_slice(&rgb);
                }
            }
        }
    }

    fn segment(&mut self, a: Point, b: Point, rgb: Rgb) {
        let [w, h] = SIZE;
        let steps = (a[0] - b[0]).abs().max((a[1] - b[1]).abs()).ceil() as usize;
        for i in 0..=steps {
            let t = (i as f64) / (steps.max(1) as f64);
            let x = (a[0] + t * (b[0] - a[0])).round() as isize;
            let y = (a[1] + t * (b[1] - a[1])).round() as isize;
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                let offset = 3 * ((y as usize) * w + x as usize);
                self.pixels[offset..offset + 3].copy_from_slice(&rgb);
            }
        }
    }

    fn drawing(
        &mut self,
        drawing: &Drawing,
        camera: &Camera,
        center: [f64; 2],
        scale: f64,
        flat: bool,
    ) -> Result<(), String> {
        let [w, h] = SIZE;
        let points: Vec<_> = drawing
            .points
            .iter()
            .map(|&p| {
                let [x, y, z] = project(p, camera);
                [
                    w as f64 / 2.0 + (x - center[0]) * scale,
                    h as f64 / 2.0 - (y - center[1]) * scale,
                    z * scale,
                ]
            })
            .collect();
        let mut edge_incidence = std::collections::BTreeMap::<(usize, usize), usize>::new();
        for &[a, b, c] in &drawing.faces {
            if [a, b, c].iter().any(|&i| i >= points.len()) {
                return Err("snapshot mesh has out-of-range triangle index".into());
            }
            let texture = drawing.texture.as_deref().map(|texture| {
                (
                    texture,
                    [
                        drawing.texcoords[a],
                        drawing.texcoords[b],
                        drawing.texcoords[c],
                    ],
                )
            });
            self.triangle(
                [points[a], points[b], points[c]],
                drawing.color,
                texture,
                flat,
            );
            if flat {
                for (start, end) in [(a, b), (b, c), (c, a)] {
                    *edge_incidence
                        .entry((start.min(end), start.max(end)))
                        .or_default() += 1;
                }
            }
        }
        for ((start, end), count) in edge_incidence {
            if count == 1 {
                self.segment(points[start], points[end], [45, 53, 68]);
            }
        }
        Ok(())
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &value in bytes {
        crc ^= u32::from(value);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn write_chunk(file: &mut fs::File, kind: [u8; 4], data: &[u8]) -> std::io::Result<()> {
    let mut chunk = Vec::with_capacity(4 + data.len());
    chunk.extend(kind);
    chunk.extend_from_slice(data);
    file.write_all(&(data.len() as u32).to_be_bytes())?;
    file.write_all(&chunk)?;
    file.write_all(&crc32(&chunk).to_be_bytes())
}

fn png(file: &mut fs::File, canvas: &Canvas) -> std::io::Result<()> {
    let [w, h] = SIZE;
    let mut bytes = Vec::with_capacity(h * (w * 3 + 1));
    for line in canvas.pixels.chunks_exact(w * 3) {
        bytes.push(0);
        bytes.extend_from_slice(line);
    }
    let mut deflate = vec![0x78, 0x01];
    let mut blocks = bytes.chunks(65535).peekable();
    while let Some(data) = blocks.next() {
        let len = data.len() as u16;
        deflate.push(u8::from(blocks.peek().is_none()));
        deflate.extend(len.to_le_bytes());
        deflate.extend((!len).to_le_bytes());
        deflate.extend_from_slice(data);
    }
    let (a, b) = bytes.iter().fold((1_u32, 0_u32), |(a, b), &byte| {
        let a = (a + u32::from(byte)) % 65521;
        (a, (b + a) % 65521)
    });
    deflate.extend(((b << 16) | a).to_be_bytes());
    let mut header = Vec::new();
    header.extend((w as u32).to_be_bytes());
    header.extend((h as u32).to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    file.write_all(b"\x89PNG\r\n\x1a\n")?;
    write_chunk(file, *b"IHDR", &header)?;
    write_chunk(file, *b"IDAT", &deflate)?;
    write_chunk(file, *b"IEND", &[])
}

fn make_image(
    drawings: &[Drawing],
    root: &PathBuf,
    label: &str,
    view: &str,
    flat: bool,
) -> Result<PathBuf, String> {
    let camera = camera(view)?;
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in drawings.iter().flat_map(|d| d.points.iter()) {
        let p = project(*point, &camera);
        for i in 0..2 {
            low[i] = low[i].min(p[i]);
            high[i] = high[i].max(p[i]);
        }
    }
    let [w, h] = SIZE;
    let mut canvas = Canvas::new(flat);
    if low.iter().all(|p| p.is_finite()) {
        let center = std::array::from_fn(|i| (low[i] + high[i]) * 0.5);
        let scale = ((w as f64 - 100.0) / (high[0] - low[0]).max(1.0))
            .min((h as f64 - 100.0) / (high[1] - low[1]).max(1.0));
        for drawing in drawings {
            canvas.drawing(drawing, &camera, center, scale, flat)?;
        }
    }
    let shape = if flat { "2d" } else { view };
    let label: String = label
        .chars()
        .take(60)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = root.join(format!("{label}_{shape}_{timestamp}_{seq}.png"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    if let Err(err) = png(&mut file, &canvas) {
        let _ = fs::remove_file(&path);
        return Err(format!("cannot write snapshot: {err}"));
    }
    Ok(path)
}

pub fn snapshot(args: &Value, state: &Value, flat: bool) -> Result<Value, String> {
    let defaults = if flat {
        vec!["2d"]
    } else {
        vec!["front", "3/4-right"]
    };
    let views = if flat || array(&args["views"]).is_empty() {
        defaults
    } else {
        array(&args["views"])
            .iter()
            .map(|view| view.as_str().ok_or("view must be a string"))
            .collect::<Result<_, _>>()?
    };
    for view in &views {
        let _ = camera(view)?;
    }
    let root = std::env::var_os("CLO_RUST_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("clo-rust-results"))
        .join("snapshots");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut drawings = Vec::new();
    if !flat {
        for avatar in std::iter::once(&state["avatar"]).chain(array(&state["additional_avatars"])) {
            drawings.extend(
                solver::avatar_geometry(avatar)
                    .into_iter()
                    .map(|(points, faces)| Drawing {
                        points,
                        faces,
                        color: [190, 183, 177],
                        texcoords: Vec::new(),
                        texture: None,
                    }),
            );
        }
    }
    let mut textures = TextureCache::default();
    drawings.extend(pieces(state, flat, &mut textures)?);
    let mut images = Vec::new();
    for view in &views {
        match make_image(&drawings, &root, string(args, "label", "snap"), view, flat) {
            Ok(path) => images.push(json!({"view":view,"path":path,"mime_type":"image/png"})),
            Err(err) => {
                for image in &images {
                    if let Some(path) = image["path"].as_str() {
                        let _ = fs::remove_file(path);
                    }
                }
                return Err(err);
            }
        }
    }
    let mut result = json!({"images":images});
    if !textures.warnings.is_empty() {
        result["render_warnings"] = json!(textures.warnings);
    }
    if flat {
        result["path"] = result["images"][0]["path"].clone();
    }
    Ok(result)
}
