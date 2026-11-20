use std::{
    fs::File,
    io::{BufRead, BufReader, Cursor, Read, Seek},
    path::Path,
};

const MAX_PIXELS: u64 = 4096 * 4096;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const DECODE_BYTES: usize = 96 * 1024 * 1024;

// First histogram rank for each successive output level from 199 through 245.
// Measured over a uniform 256-level ramp exported by CLO 2026.1.382.
const CLO_DESATURATION_THRESHOLDS: [u8; 47] = [
    2, 5, 7, 10, 13, 15, 18, 22, 25, 28, 31, 35, 38, 41, 46, 49, 53, 58, 63, 66,
    71, 76, 81, 86, 91, 96, 100, 107, 112, 119, 124, 130, 137, 143, 150, 156, 165,
    171, 179, 186, 194, 202, 211, 221, 229, 239, 247,
];

fn lightness(pixel: &[u8]) -> usize {
    if let [r, g, b, ..] = pixel {
        let low = (*r).min(*g).min(*b);
        let high = (*r).max(*g).max(*b);
        (usize::from(low) + usize::from(high)) / 2
    } else {
        usize::from(pixel[0])
    }
}

pub struct BaseTexture {
    width: usize,
    height: usize,
    channels: usize,
    pixels: Vec<u8>,
    desaturated: bool,
}

impl BaseTexture {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| format!("open texture: {error}"))?;
        if file.metadata().map_err(|error| error.to_string())?.len() > MAX_FILE_BYTES {
            return Err("texture exceeds 64 MiB".into());
        }
        let mut source = BufReader::new(file);
        if is_jpeg(source.fill_buf().map_err(|error| error.to_string())?) {
            Self::decode_jpeg(source)
        } else {
            Self::decode(source)
        }
    }

    fn decode_jpeg(source: impl Read) -> Result<Self, String> {
        let mut decoder = jpeg_decoder::Decoder::new(source);
        decoder.set_max_decoding_buffer_size(DECODE_BYTES);
        decoder
            .read_info()
            .map_err(|error| format!("decode JPEG: {error}"))?;
        let image = decoder.info().ok_or("JPEG has no image metadata")?;
        let (width, height) = (u32::from(image.width), u32::from(image.height));
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(format!("JPEG texture must have between 1 and {MAX_PIXELS} pixels"));
        }
        let channels = match image.pixel_format {
            jpeg_decoder::PixelFormat::L8 => 1,
            jpeg_decoder::PixelFormat::RGB24 => 3,
            _ => return Err("JPEG textures must be 8-bit grayscale or RGB".into()),
        };
        let pixels = decoder
            .decode()
            .map_err(|error| format!("read JPEG pixels: {error}"))?;
        Ok(Self {
            width: width as usize,
            height: height as usize,
            channels,
            pixels,
            desaturated: false,
        })
    }

    fn encode_png(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, self.width as u32, self.height as u32);
        encoder.set_color(match self.channels {
            1 => png::ColorType::Grayscale,
            2 => png::ColorType::GrayscaleAlpha,
            3 => png::ColorType::Rgb,
            _ => png::ColorType::Rgba,
        });
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
        Ok(bytes)
    }

    fn decode<R: BufRead + Seek>(source: R) -> Result<Self, String> {
        let mut decoder = png::Decoder::new_with_limits(
            source,
            png::Limits {
                bytes: DECODE_BYTES,
            },
        );
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder
            .read_info()
            .map_err(|error| format!("decode PNG: {error}"))?;
        let info = reader.info();
        if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
            return Err(format!("base texture PNG exceeds {MAX_PIXELS} pixels"));
        }
        if info.animation_control.is_some() {
            return Err("animated PNG base textures are not supported".into());
        }
        let output_size = reader
            .output_buffer_size()
            .ok_or("base texture PNG is too large")?;
        let mut pixels = vec![0; output_size];
        let frame = reader
            .next_frame(&mut pixels)
            .map_err(|error| format!("read PNG pixels: {error}"))?;
        let channels = match frame.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => {
                return Err("indexed base texture PNG was not expanded".into())
            }
        };
        let (width, height) = (frame.width as usize, frame.height as usize);
        if frame.bit_depth != png::BitDepth::Eight
            || width == 0
            || height == 0
            || frame.line_size != width * channels
        {
            return Err("invalid base texture PNG dimensions or pixel format".into());
        }
        pixels.truncate(frame.buffer_size());
        Ok(Self {
            width,
            height,
            channels,
            pixels,
            desaturated: false,
        })
    }

    pub fn desaturate(&mut self) {
        if self.desaturated {
            return;
        }
        let mut histogram = [0u64; 256];
        for pixel in self.pixels.chunks_exact(self.channels) {
            histogram[lightness(pixel)] += 1;
        }
        let count = (self.width * self.height) as u64;
        let mut cumulative = 0;
        let tones = histogram.map(|frequency| {
            cumulative += frequency;
            let rank = (cumulative * 255 / count) as u8;
            198 + CLO_DESATURATION_THRESHOLDS.partition_point(|&cutoff| cutoff <= rank) as u8
        });
        let color_channels = if self.channels < 3 { 1 } else { 3 };
        for pixel in self.pixels.chunks_exact_mut(self.channels) {
            let gray = tones[lightness(pixel)];
            pixel[..color_channels].fill(gray);
        }
        self.desaturated = true;
    }

    pub fn sample(&self, uv: [f64; 2], base: [u8; 3]) -> [u8; 3] {
        let at = |unit: f64, count: usize| {
            ((unit.clamp(0.0, 1.0) * count as f64) as usize).min(count - 1)
        };
        let start = (at(uv[1], self.height) * self.width + at(uv[0], self.width)) * self.channels;
        let p = &self.pixels[start..start + self.channels];
        if self.desaturated {
            let tint = base.map(|channel| ((u16::from(p[0]) * u16::from(channel) + 127) / 255) as u8);
            return match self.channels {
                2 | 4 => composite(tint, p[self.channels - 1], base),
                _ => tint,
            };
        }
        match self.channels {
            1 => [p[0]; 3],
            2 => composite([p[0]; 3], p[1], base),
            3 => [p[0], p[1], p[2]],
            _ => composite([p[0], p[1], p[2]], p[3], base),
        }
    }
}

fn composite(fg: [u8; 3], alpha: u8, bg: [u8; 3]) -> [u8; 3] {
    let alpha = u16::from(alpha);
    std::array::from_fn(|i| {
        ((u16::from(fg[i]) * alpha + u16::from(bg[i]) * (255 - alpha) + 127) / 255) as u8
    })
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

pub(crate) fn png_for_export(path: &Path, desaturated: bool) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("open base texture: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read base texture: {error}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("texture exceeds 64 MiB".into());
    }
    let jpeg = is_jpeg(&bytes);
    let mut texture = if jpeg {
        BaseTexture::decode_jpeg(Cursor::new(&bytes))?
    } else {
        BaseTexture::decode(Cursor::new(&bytes))?
    };
    if desaturated {
        texture.desaturate();
    }
    if jpeg || desaturated {
        texture.encode_png()
    } else {
        Ok(bytes)
    }
}
