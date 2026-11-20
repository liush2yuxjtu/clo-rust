use super::{
    array,
    solver::{self, cross, dot, sub},
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
    strokes: Vec<Vec<Point>>,
}

impl Drawing {
    fn from_mesh(mesh: &Value) -> Result<Self, String> {
        Ok(Self {
            color: [0; 3],
            points: serde_json::from_value(mesh["positions"].clone())
                .map_err(|e| format!("invalid snapshot mesh positions: {e}"))?,
            faces: serde_json::from_value(mesh["triangles"].clone())
                .map_err(|e| format!("invalid snapshot mesh faces: {e}"))?,
            texcoords: serde_json::from_value(mesh["texcoords"].clone()).unwrap_or_default(),
            texture: None,
            strokes: Vec::new(),
        })
    }
}

#[derive(Default)]
struct TextureCache {
    by_path: BTreeMap<(String, bool), Option<Rc<BaseTexture>>>,
    warnings: Vec<String>,
}

impl TextureCache {
    fn load(&mut self, path: &str, desaturated: bool) -> Option<Rc<BaseTexture>> {
        self.by_path
            .entry((path.to_owned(), desaturated))
            .or_insert_with(|| match BaseTexture::load(Path::new(path)) {
                Ok(mut texture) => {
                    if desaturated {
                        texture.desaturate();
                    }
                    Some(Rc::new(texture))
                }
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
        if let Some((fabric, path)) =
            fabric.and_then(|f| Some((f, f["texture_maps"]["base"].as_str()?)))
        {
            if drawing.texcoords.len() == drawing.points.len() {
                drawing.texture = self.load(path, fabric["base_texture_desaturated"] == true);
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
    if flat {
        return array(&state["patterns"]).iter().map(|pattern| {
            Ok(Drawing {
                points: crate::compat_scene::layout_outline(pattern)?,
                faces: Vec::new(),
                color: [0; 3],
                texcoords: Vec::new(),
                texture: None,
                strokes: crate::compat_scene::internal_strokes(pattern)?,
            })
        }).collect();
    }
    let cached = array(&state["simulation"]["meshes"]);
    if cached.is_empty() {
        let meshes = solver::initial_meshes(state)?;
        return meshes
            .into_iter()
            .enumerate()
            .filter(|(_, mesh)| !hidden(state, &mesh.id))
            .map(|(index, mesh)| {
                let mut drawing = Drawing {
                    points: mesh.positions,
                    faces: mesh.triangles,
                    color: [0; 3],
                    texcoords: mesh.texcoords,
                    texture: None,
                    strokes: Vec::new(),
                };
                textures.apply(state, &mesh.fabric_index, index, &mut drawing);
                Ok(drawing)
            })
            .collect();
    }
    let mut drawn = cached
        .iter()
        .enumerate()
        .filter(|(_, mesh)| !hidden(state, &mesh["id"]))
        .map(|(index, mesh)| {
            let fabric_id = array(&state["patterns"])
                .iter()
                .find(|p| p["id"] == mesh["id"])
                .map_or(&mesh["fabric_index"], |p| &p["fabric_index"]);
            let mut drawing = Drawing::from_mesh(mesh)?;
            if super::mesh_uses_rest_mm(state, mesh) {
                drawing.texcoords = solver::uv_coordinates(&drawing.texcoords);
            }
            textures.apply(state, fabric_id, index, &mut drawing);
            Ok(drawing)
        })
        .collect::<Result<Vec<_>, String>>()?;
    if state["simulation"]["source"] == "mixed_saved_pose" {
        for (i, panel) in array(&state["patterns"]).iter().enumerate() {
            if panel["hide"] == true || cached.iter().any(|mesh| mesh["id"] == panel["id"]) {
                continue;
            }
            let mesh = solver::new_mesh(panel, f64::INFINITY, state)?;
            let mut drawing = Drawing {
                points: mesh.positions,
                faces: mesh.triangles,
                color: [0; 3],
                texcoords: mesh.texcoords,
                texture: None,
                strokes: Vec::new(),
            };
            textures.apply(state, &mesh.fabric_index, i, &mut drawing);
            drawn.push(drawing);
        }
    }
    Ok(drawn)
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

const CAMERA_PRESETS: [&str; 10] = [
    "bottom", "3/4-right", "front", "3/4-left", "right", "top", "left", "focus", "back", "fit",
];

pub(crate) fn set_camera_preset(state: &mut Value, preset: &Value) -> Result<(), String> {
    let index = preset.as_u64().filter(|&index| index < CAMERA_PRESETS.len() as u64)
        .ok_or("unsupported snapshot camera preset")?;
    // In an unselected scene, both commands retain the current orientation.
    if !matches!(index, 7 | 9) {
        state["runtime"]["ui"]["view"] = json!(index);
    }
    Ok(())
}

fn current_camera(state: &Value) -> Result<&str, String> {
    let current = &state["runtime"]["ui"]["view"];
    match current.as_u64() {
        Some(index) => CAMERA_PRESETS.get(index as usize).copied()
            .ok_or_else(|| "unsupported snapshot camera preset".to_owned()),
        None => Ok(current.as_str().unwrap_or("front")),
    }
}

fn camera(view: &str) -> Result<Camera, String> {
    let name = view.to_ascii_lowercase();
    let angle = match name.as_str() {
        "front" | "2d" => 0.0,
        "back" => 180.0,
        "left" => -90.0,
        "right" => 90.0,
        "3/4-right" | "three-quarter" => 42.0,
        "3/4-left" => -42.0,
        "top" | "bottom" => 0.0,
        _ => return Err(format!("unknown snapshot view {view:?}")),
    };
    if matches!(name.as_str(), "top" | "bottom") {
        let sign = if name == "top" { 1.0 } else { -1.0 };
        return Ok(Camera {
            horizontal: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, -sign],
            depth: [0.0, sign, 0.0],
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
    channels: usize,
}

impl Canvas {
    fn new(flat: bool) -> Self {
        let [w, h] = SIZE;
        if flat {
            return Self { pixels: vec![0; w * h * 4], depth: Vec::new(), channels: 4 };
        }
        let mut pixels = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for _ in 0..w {
                let shade = (250 - y * 20 / h) as u8;
                pixels.extend([shade, shade, shade.saturating_add(2)]);
            }
        }
        Self {
            pixels,
            depth: vec![f64::NEG_INFINITY; w * h],
            channels: 3,
        }
    }

    fn triangle(
        &mut self,
        [a, b, c]: [Point; 3],
        color: Rgb,
        texture: Option<(&BaseTexture, [[f64; 2]; 3])>,
    ) {
        let [w, h] = SIZE;
        let signed = |a: Point, b: Point, x: f64, y: f64| {
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
        };
        let area = signed(a, b, c[0], c[1]);
        let normal = cross(sub(b, a), sub(c, a));
        let n = dot(normal, normal).sqrt().max(1e-15);
        let light = [-0.43, -0.49, 0.76];
        let factor = 0.67 + 0.33 * (dot(normal, light) / n).abs();
        let shade = |rgb: Rgb| rgb.map(|c| (f64::from(c) * factor).min(255.0) as u8);
        let flat_rgb = shade(color);
        if area.abs() < 1e-10 {
            // A panel viewed edge-on still has a visible one-pixel profile.
            for (start, end) in [(a, b), (b, c), (c, a)] {
                self.segment(start, end, flat_rgb);
            }
            return;
        }
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
                let index = (y as usize) * w + x as usize;
                let offset = self.channels * index;
                if self.channels == 4 {
                    self.pixels[offset + 3] = 255;
                } else {
                    let depth = a[2] + t * (b[2] - a[2]);
                    if depth < self.depth[index] {
                        continue;
                    }
                    self.depth[index] = depth;
                }
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
        let screen = |p| {
            let [x, y, z] = project(p, camera);
            [
                w as f64 / 2.0 + (x - center[0]) * scale,
                h as f64 / 2.0 - (y - center[1]) * scale,
                z * scale,
            ]
        };
        let points: Vec<_> = drawing.points.iter().copied().map(screen).collect();
        if flat {
            for (&start, &end) in points.iter().zip(points.iter().cycle().skip(1)) {
                self.segment(start, end, [45, 53, 68]);
            }
            for stroke in &drawing.strokes {
                for segment in stroke.windows(2) {
                    self.segment(screen(segment[0]), screen(segment[1]), [180, 53, 72]);
                }
            }
            return Ok(());
        }
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
            self.triangle([points[a], points[b], points[c]], drawing.color, texture);
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

fn write_chunk(file: &mut impl Write, kind: [u8; 4], data: &[u8]) -> std::io::Result<()> {
    let mut chunk = Vec::with_capacity(4 + data.len());
    chunk.extend(kind);
    chunk.extend_from_slice(data);
    file.write_all(&(data.len() as u32).to_be_bytes())?;
    file.write_all(&chunk)?;
    file.write_all(&crc32(&chunk).to_be_bytes())
}

fn png(file: &mut impl Write, canvas: &Canvas) -> std::io::Result<()> {
    let [w, h] = SIZE;
    let row_bytes = w * canvas.channels;
    let mut bytes = Vec::with_capacity(h * (row_bytes + 1));
    for line in canvas.pixels.chunks_exact(row_bytes) {
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
    let color_type = if canvas.channels == 4 { 6 } else { 2 };
    header.extend([8, color_type, 0, 0, 0]);
    file.write_all(b"\x89PNG\r\n\x1a\n")?;
    write_chunk(file, *b"IHDR", &header)?;
    write_chunk(file, *b"IDAT", &deflate)?;
    write_chunk(file, *b"IEND", &[])
}

fn render_canvas(drawings: &[Drawing], view: &str, flat: bool) -> Result<Canvas, String> {
    let camera = camera(view)?;
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for point in drawings.iter().flat_map(|d| d.points.iter().chain(d.strokes.iter().flatten())) {
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
    Ok(canvas)
}

fn make_image(
    drawings: &[Drawing],
    root: &Path,
    label: &str,
    view: &str,
    orientation: &str,
    flat: bool,
) -> Result<PathBuf, String> {
    let canvas = render_canvas(drawings, orientation, flat)?;
    let shape = if flat { "2d".to_owned() } else { view.replace('/', "") };
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

fn scene_drawings(
    state: &Value,
    flat: bool,
    textures: &mut TextureCache,
) -> Result<Vec<Drawing>, String> {
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
                        strokes: Vec::new(),
                    }),
            );
        }
    }
    drawings.extend(pieces(state, flat, textures)?);
    if !flat {
        for (index, mesh) in array(&state["imported_meshes"]).iter().enumerate() {
            let mut drawing = Drawing::from_mesh(mesh)?;
            textures.apply(state, &mesh["fabric_index"], index, &mut drawing);
            drawings.push(drawing);
        }
    }
    Ok(drawings)
}

pub(crate) fn native_previews(state: &Value) -> Result<Vec<u8>, String> {
    project_views(state, &["right", "back", "left"])
}

pub(crate) fn native_primary_preview(state: &Value) -> Result<Vec<u8>, String> {
    project_views(state, &["front"])
}

fn project_views(state: &Value, views: &[&str]) -> Result<Vec<u8>, String> {
    const SIDE: usize = 500;
    let [width, height] = SIZE;
    let scale = (width.max(height) as f64) / SIDE as f64;
    let margin_x = (SIDE as f64 - width as f64 / scale) / 2.0;
    let margin_y = (SIDE as f64 - height as f64 / scale) / 2.0;
    let drawings = scene_drawings(state, false, &mut TextureCache::default())?;
    let mut output = Vec::new();
    for &view in views {
        let canvas = render_canvas(&drawings, view, false)?;
        let mut pixels = Vec::with_capacity(SIDE * SIDE * 3);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let cx = ((x as f64 - margin_x) * scale).floor() as isize;
                let cy = ((y as f64 - margin_y) * scale).floor() as isize;
                if (0..width as isize).contains(&cx) && (0..height as isize).contains(&cy) {
                    let begin = (cy as usize * width + cx as usize) * 3;
                    pixels.extend_from_slice(&canvas.pixels[begin..begin + 3]);
                } else {
                    pixels.extend_from_slice(&canvas.pixels[..3]);
                }
            }
        }
        let mut encoder = png::Encoder::new(&mut output, SIDE as u32, SIDE as u32);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(&pixels)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
    }
    Ok(output)
}

pub(crate) fn export_snapshot(path: &Path, state: &Value, flat: bool) -> Result<(), String> {
    if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("png")) {
        return Err("snapshot export requires a .png path".into());
    }
    let view = if flat { "2d" } else { current_camera(state)? };
    let drawings = scene_drawings(state, flat, &mut TextureCache::default())?;
    let canvas = render_canvas(&drawings, view, flat)?;
    let mut bytes = Vec::new();
    png(&mut bytes, &canvas).map_err(|e| e.to_string())?;
    crate::compat_scene::write_artifact(path, &bytes)
}

pub fn snapshot(args: &Value, state: &mut Value, flat: bool) -> Result<Value, String> {
    if flat && array(&state["patterns"]).is_empty() {
        return Ok(json!({"images": [], "empty_scene": true}));
    }
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
    let root = std::env::var_os("CLO_RUST_OUTPUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("clo-rust-results"))
        .join("snapshots");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut textures = TextureCache::default();
    let drawings = scene_drawings(state, flat, &mut textures)?;
    let label = args["label"]
        .as_str()
        .filter(|label| !label.is_empty())
        .unwrap_or("snap");
    let mut images = Vec::new();
    for view in views {
        let orientation = if flat {
            "2d"
        } else {
            let lower = view.to_ascii_lowercase();
            let canonical = if lower == "three-quarter" { "3/4-right" } else { lower.as_str() };
            let index = CAMERA_PRESETS.iter().position(|&name| name == canonical)
                .ok_or_else(|| format!("unknown snapshot view {view:?}"))?;
            set_camera_preset(state, &json!(index))?;
            current_camera(state)?
        };
        let path = make_image(&drawings, &root, label, view, orientation, flat)?;
        images.push(json!({"view":view,"path":path,"mime_type":"image/png"}));
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

#[cfg(test)]
#[test]
fn edge_on_snapshot_keeps_a_one_pixel_profile() {
    let drawing = Drawing {
        points: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [25.0, 80.0, 0.0]],
        faces: vec![[0, 1, 2]],
        color: [20, 50, 220],
        texcoords: Vec::new(),
        texture: None,
        strokes: Vec::new(),
    };
    for (view, vertical) in [("left", true), ("right", true), ("top", false), ("bottom", false)] {
        let canvas = render_canvas(std::slice::from_ref(&drawing), view, false).unwrap();
        let pixels: Vec<_> = canvas.pixels.chunks_exact(3).enumerate().filter_map(|(index, rgb)| {
            let blue = i16::from(rgb[2]);
            (blue - i16::from(rgb[0]) > 70).then_some((index % SIZE[0], index / SIZE[0]))
        }).collect();
        assert!(pixels.len() > 100, "{view}");
        assert!(pixels.iter().all(|&(x, y)| {
            if vertical { x == pixels[0].0 } else { y == pixels[0].1 }
        }), "{view}");
    }
}

#[cfg(test)]
#[test]
fn flat_snapshot_renders_visible_inner_paths_only() {
    let mut perimeter = vec![json!([0, 0])];
    perimeter.extend((1..521).map(|n| json!([f64::from(n) * 100.0 / 521.0, 0, 2])));
    perimeter.extend([json!([100, 0]), json!([100, 100]), json!([0, 100])]);
    let state = json!({
        "patterns": [{
            "id": 0, "name": "Panel",
            "points": perimeter, "sampled_polyline": true,
            "native_children": [
                {"type": "InnerShape", "lines": [{"points": [[20, 50], [50, 65, 2], [80, 50]]}]},
                {"type": "InnerShape", "visible": false, "lines": [{"points": [[20, 30], [80, 30]]}]},
                {"type": "Base Line", "lines": [{"points": [[20, 25], [80, 25]]}]},
            ]
        }],
    });
    let drawings = scene_drawings(&state, true, &mut TextureCache::default()).unwrap();
    assert_eq!(drawings[0].points.len(), 524);
    assert_eq!(drawings[0].strokes.len(), 1);
    let canvas = render_canvas(&drawings, "2d", true).unwrap();
    let pixel = |x, y| {
        let offset = (y * SIZE[0] + x) * 4;
        &canvas.pixels[offset..offset + 4]
    };
    let interior = &[180, 53, 72, 255];
    assert_eq!(pixel(400, 260), interior);
    assert_ne!(pixel(400, 350), interior);
    assert_ne!(pixel(400, 470), interior);
    assert_ne!(pixel(400, 500), interior);
    assert_eq!(pixel(400, 350)[3], 0);
}
