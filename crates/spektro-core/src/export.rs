//! Write rendered images: EXR (half float, linear) and JPEG (8-bit, already display encoded).

use spektrafilm_math::image::ImageBuf;
use spektrafilm_math::precision::to_f32;
use std::path::Path;

/// CIE xy chromaticities for the EXR header.
#[derive(Debug, Clone, Copy)]
pub struct Chromaticities {
    pub red: (f32, f32),
    pub green: (f32, f32),
    pub blue: (f32, f32),
    pub white: (f32, f32),
}

impl Chromaticities {
    pub fn for_space(name: &str) -> Option<Self> {
        match name {
            "Rec. 2020" | "Rec2020" | "ITU-R BT.2020" => Some(Chromaticities {
                red: (0.708, 0.292),
                green: (0.170, 0.797),
                blue: (0.131, 0.046),
                white: (0.3127, 0.3290),
            }),
            "ACES2065-1" => Some(Chromaticities {
                red: (0.7347, 0.2653),
                green: (0.0, 1.0),
                blue: (0.0001, -0.077),
                white: (0.32168, 0.33767),
            }),
            "sRGB" => Some(Chromaticities {
                red: (0.64, 0.33),
                green: (0.30, 0.60),
                blue: (0.15, 0.06),
                white: (0.3127, 0.3290),
            }),
            // Any other engine colourspace: primaries from its RGB→XYZ columns.
            other => {
                let cs = spektrafilm_math::colourspaces::lookup(other).ok()?;
                let m = cs.rgb_to_xyz;
                let xy = |c: usize| {
                    let s = m[0][c] + m[1][c] + m[2][c];
                    ((m[0][c] / s) as f32, (m[1][c] / s) as f32)
                };
                Some(Chromaticities { red: xy(0), green: xy(1), blue: xy(2), white: (cs.whitepoint[0] as f32, cs.whitepoint[1] as f32) })
            }
        }
    }
}

/// Encode a display-referred image as JPEG bytes (in memory, for previews).
pub fn encode_jpeg(img: &ImageBuf, quality: u8) -> anyhow::Result<Vec<u8>> {
    let bytes: Vec<u8> = img.data.iter().map(|v| (to_f32(*v).clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let buf: image::RgbImage = image::ImageBuffer::from_raw(img.width, img.height, bytes).ok_or_else(|| anyhow::anyhow!("bad buffer size"))?;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&buf)?;
    Ok(out)
}

/// RGB half-float EXR with ZIP compression. Values are written as-is (linear).
pub fn write_exr(path: &Path, img: &ImageBuf, chroma: Option<Chromaticities>, color_space_name: &str) -> anyhow::Result<()> {
    use exr::prelude::*;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let w = img.width as usize;
    let h = img.height as usize;
    let get = |x: usize, y: usize| -> (f16, f16, f16) {
        let i = (y * w + x) * 3;
        (
            f16::from_f32(to_f32(img.data[i])),
            f16::from_f32(to_f32(img.data[i + 1])),
            f16::from_f32(to_f32(img.data[i + 2])),
        )
    };
    let mut layer_attrs = LayerAttributes::named("rgb");
    layer_attrs.other.insert(Text::from("spektro:color_space"), AttributeValue::Text(Text::from(color_space_name)));
    let mut image_attrs = ImageAttributes::new(IntegerBounds::from_dimensions((w, h)));
    if let Some(c) = chroma {
        image_attrs.chromaticities = Some(exr::meta::attribute::Chromaticities {
            red: Vec2(c.red.0, c.red.1),
            green: Vec2(c.green.0, c.green.1),
            blue: Vec2(c.blue.0, c.blue.1),
            white: Vec2(c.white.0, c.white.1),
        });
    }
    let layer = Layer::new(
        (w, h),
        layer_attrs,
        Encoding { compression: Compression::ZIP16, ..Encoding::default() },
        SpecificChannels::rgb(|Vec2(x, y)| get(x, y)),
    );
    let mut image = Image::from_layer(layer);
    image.attributes = image_attrs;
    let tmp = path.with_extension("exr.part");
    image.write().to_file(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 8-bit JPEG. The buffer is expected to be display-encoded already (spektrafilm applies the sRGB
/// curve when `output_cctf_encoding` is on); values are clamped and rounded.
pub fn write_jpeg(path: &Path, img: &ImageBuf, quality: u8) -> anyhow::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let w = img.width;
    let h = img.height;
    let bytes: Vec<u8> = img.data.iter().map(|v| (to_f32(*v).clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let buf: image::RgbImage = image::ImageBuffer::from_raw(w, h, bytes).ok_or_else(|| anyhow::anyhow!("bad buffer size"))?;
    let tmp = path.with_extension("jpg.part");
    let file = std::fs::File::create(&tmp)?;
    let mut out = std::io::BufWriter::new(file);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode_image(&buf)?;
    drop(enc);
    std::io::Write::flush(&mut out)?;
    drop(out);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spektrafilm_math::precision::from_f32;

    fn gradient() -> ImageBuf {
        let (w, h) = (8u32, 4u32);
        let mut data = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                data.push(from_f32(x as f32 / 7.0));
                data.push(from_f32(y as f32 / 3.0));
                data.push(from_f32(0.5));
            }
        }
        ImageBuf::from_data(w, h, data)
    }

    #[test]
    fn writes_exr_and_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.exr");
        write_exr(&p, &gradient(), Chromaticities::for_space("Rec. 2020"), "Rec. 2020").unwrap();
        use exr::prelude::*;
        let img = read_first_rgba_layer_from_file(
            &p,
            |res, _| vec![(0f32, 0f32, 0f32, 0f32); res.width() * res.height()],
            |px, pos, (r, g, b, a): (f32, f32, f32, f32)| px[pos.y() * 8 + pos.x()] = (r, g, b, a),
        )
        .unwrap();
        let px = img.layer_data.channel_data.pixels[1 * 8 + 7];
        assert!((px.0 - 1.0).abs() < 1e-3);
        assert!((px.1 - 1.0 / 3.0).abs() < 1e-3);
        assert!(img.attributes.chromaticities.is_some());
    }

    #[test]
    fn writes_jpeg() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.jpg");
        write_jpeg(&p, &gradient(), 90).unwrap();
        let (w, h) = image::image_dimensions(&p).unwrap();
        assert_eq!((w, h), (8, 4));
    }
}
