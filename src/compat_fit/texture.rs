use std::{
    fs::File,
    io::{BufRead, BufReader, Cursor, Read, Seek},
    path::Path,
};

const MAX_DIMENSION: u32 = 4096;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const DECODE_BYTES: usize = 96 * 1024 * 1024;

pub struct BaseTexture {
    width: usize,
    height: usize,
    channels: usize,
    pixels: Vec<u8>,
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
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(format!(
                "JPEG texture dimensions must be within {MAX_DIMENSION} × {MAX_DIMENSION}"
            ));
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
        })
    }

    fn encode_png(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, self.width as u32, self.height as u32);
        encoder.set_color(if self.channels == 1 {
            png::ColorType::Grayscale
        } else {
            png::ColorType::Rgb
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
        if info.width > MAX_DIMENSION || info.height > MAX_DIMENSION {
            return Err(format!(
                "base texture PNG dimensions exceed {MAX_DIMENSION} × {MAX_DIMENSION}"
            ));
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
        })
    }

    pub fn sample(&self, uv: [f64; 2], base: [u8; 3]) -> [u8; 3] {
        let at = |unit: f64, count: usize| {
            ((unit.clamp(0.0, 1.0) * count as f64) as usize).min(count - 1)
        };
        let start = (at(uv[1], self.height) * self.width + at(uv[0], self.width)) * self.channels;
        let p = &self.pixels[start..start + self.channels];
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

pub(crate) fn png_for_export(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("open base texture: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read base texture: {error}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("texture exceeds 64 MiB".into());
    }
    if is_jpeg(&bytes) {
        BaseTexture::decode_jpeg(Cursor::new(bytes))?.encode_png()
    } else {
        BaseTexture::decode(Cursor::new(&bytes))?;
        Ok(bytes)
    }
}
