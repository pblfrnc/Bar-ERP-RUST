//! Foto do produto: reduz antes de gravar (tradução de `client/src/utils/imageResize.ts`). Foto de celular
//! (3–8 MB) vira um JPEG de ~100 KB, o que basta para o cardápio e não pesa o banco nem o backup.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Rgb, RgbImage};

const MAX_SIDE: u32 = 1000;
const QUALITY: u8 = 82;

/// Abre JPEG/PNG/WebP, aplica a rotação EXIF, limita o maior lado a 1000 px, põe fundo branco (PNG
/// transparente ficaria preto no JPEG) e grava JPEG qualidade 82.
pub fn shrink_photo(data: &[u8]) -> Result<Vec<u8>, String> {
    let open_err = || "Não foi possível abrir a imagem. Use uma foto JPEG, PNG ou WebP.".to_string();
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|_| open_err())?;
    let mut decoder = reader.into_decoder().map_err(|_| open_err())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|_| open_err())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let (w, h) = (img.width(), img.height());
    let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    let (nw, nh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    let resized = if scale < 1.0 { img.resize_exact(nw, nh, FilterType::Triangle) } else { img };

    let rgba = resized.to_rgba8();
    let mut flat = RgbImage::from_pixel(rgba.width(), rgba.height(), Rgb([255, 255, 255]));
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        flat.put_pixel(x, y, Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, QUALITY)
        .encode_image(&flat)
        .map_err(|_| "Não foi possível processar a imagem.".to_string())?;
    Ok(out)
}

/// Miniatura RGBA (lado maior até `max_side`) para mostrar na tela: (largura, altura, pixels).
pub fn thumbnail_rgba(data: &[u8], max_side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(data).ok()?;
    let thumb = if img.width().max(img.height()) > max_side { img.thumbnail(max_side, max_side) } else { img };
    let rgba = thumb.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};

    #[test]
    fn reduz_e_vira_jpeg_com_fundo_branco() {
        let big = RgbaImage::from_pixel(2400, 1200, Rgba([0, 0, 0, 0]));
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(big).write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let jpeg = shrink_photo(&png).unwrap();
        assert_eq!(&jpeg[..3], &[0xFF, 0xD8, 0xFF]);
        let back = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!((back.width(), back.height()), (1000, 500));
        assert!(back.get_pixel(10, 10)[0] > 240, "transparente vira branco");
        assert!(shrink_photo(b"nao e imagem").unwrap_err().contains("JPEG, PNG ou WebP"));
        let (tw, th, px) = thumbnail_rgba(&jpeg, 200).unwrap();
        assert_eq!((tw, th, px.len()), (200, 100, 200 * 100 * 4));
    }
}
