//! Bounded, alpha-aware box reduction. Non-interlaced PNGs never expand into a
//! full-sized RGBA image: retain only decoder rows, one accumulator row, and the
//! output (at most 320×240). Interlaced PNGs use the existing bounded fallback.
use std::io::Cursor;

pub fn thumbnail(png: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new(Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    decoder.set_limits(png::Limits {
        bytes: 16 * 1024 * 1024,
    });
    let mut reader = decoder.read_info().map_err(|_| "图片无法预览")?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 25_000_000 {
        return Err("图片超过预览大小限制".into());
    }
    if reader.info().interlaced {
        return crate::store::make_thumbnail(png);
    }
    let (tw, th) = dimensions(w, h);
    let color = reader.output_color_type().0;
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return Err("图片颜色格式无法预览".into()),
    };
    // Premultiplied RGB, alpha sum, sample count; prevent transparent pixels
    // bleeding their hidden RGB into the visible thumbnail.
    let mut sums = vec![[0u64; 5]; tw as usize];
    let mut output = vec![0u8; (tw * th * 4) as usize];
    for y in 0..h {
        let row = reader
            .next_row()
            .map_err(|_| "图片像素读取失败")?
            .ok_or("图片数据不完整")?;
        if row.data().len() != w as usize * channels {
            return Err("图片行长度无效".into());
        }
        for (x, p) in row.data().chunks_exact(channels).enumerate() {
            let [r, g, b, a] = match channels {
                1 => [p[0], p[0], p[0], 255],
                2 => [p[0], p[0], p[0], p[1]],
                3 => [p[0], p[1], p[2], 255],
                _ => [p[0], p[1], p[2], p[3]],
            };
            let sum = &mut sums[x * tw as usize / w as usize];
            let a = u64::from(a);
            sum[0] += u64::from(r) * a;
            sum[1] += u64::from(g) * a;
            sum[2] += u64::from(b) * a;
            sum[3] += a;
            sum[4] += 1;
        }
        let dy = u64::from(y) * u64::from(th) / u64::from(h);
        let next_dy = u64::from(y + 1) * u64::from(th) / u64::from(h);
        if next_dy != dy {
            for (x, sum) in sums.iter_mut().enumerate() {
                let pixel = &mut output[(dy as usize * tw as usize + x) * 4..][..4];
                for c in 0..3 {
                    if let Some(avg) = (sum[c] + sum[3] / 2).checked_div(sum[3]) {
                        pixel[c] = avg as u8;
                    }
                }
                pixel[3] = ((sum[3] + sum[4] / 2) / sum[4]) as u8;
                *sum = [0; 5];
            }
        }
    }
    reader.finish().map_err(|_| "图片校验失败")?;
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, tw, th);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| "缩略图编码失败")?;
        writer
            .write_image_data(&output)
            .map_err(|_| "缩略图编码失败")?;
    }
    Ok(encoded)
}

fn dimensions(w: u32, h: u32) -> (u32, u32) {
    if w <= 320 && h <= 240 {
        return (w, h);
    }
    if u64::from(w) * 240 > u64::from(h) * 320 {
        (320, (u64::from(h) * 320 / u64::from(w)).max(1) as u32)
    } else {
        ((u64::from(w) * 240 / u64::from(h)).max(1) as u32, 240)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encode(w: u32, h: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, w, h);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(data)
                .unwrap();
        }
        bytes
    }
    #[test]
    fn dimensions_preserve_aspect_and_never_upscale() {
        assert_eq!(dimensions(5000, 5000), (240, 240));
        assert_eq!(dimensions(4000, 1000), (320, 80));
        assert_eq!(dimensions(1, 2000), (1, 240));
        assert_eq!(dimensions(17, 9), (17, 9));
    }
    #[test]
    fn rgb_and_gray_alpha_keep_small_pixels() {
        for (color, data, expected) in [
            (
                png::ColorType::Rgb,
                vec![10, 20, 30, 40, 50, 60],
                vec![10, 20, 30, 255, 40, 50, 60, 255],
            ),
            (
                png::ColorType::GrayscaleAlpha,
                vec![50, 128, 70, 255],
                vec![50, 50, 50, 128, 70, 70, 70, 255],
            ),
        ] {
            let out = thumbnail(&encode(2, 1, color, &data)).unwrap();
            assert_eq!(
                image::load_from_memory(&out)
                    .unwrap()
                    .into_rgba8()
                    .into_raw(),
                expected
            );
        }
    }
    #[test]
    fn reduction_averages_premultiplied_alpha_and_rejects_truncation() {
        let mut pixels = Vec::new();
        for _ in 0..320 {
            pixels.extend_from_slice(&[255, 0, 0, 255, 0, 0, 255, 0]);
        }
        let input = encode(640, 1, png::ColorType::Rgba, &pixels);
        let out = image::load_from_memory(&thumbnail(&input).unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(out.dimensions(), (320, 1));
        assert!(out.pixels().all(|p| p.0 == [255, 0, 0, 128]));
        assert!(thumbnail(&input[..input.len() / 2]).is_err());
    }
}
