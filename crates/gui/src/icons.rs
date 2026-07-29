//! Embedded tray icons: the PNGs from data/icons are compiled in with
//! include_bytes! and decoded once into the ARGB32 (network byte order,
//! non-premultiplied) pixmaps ksni expects.
//!
//! Two variants ship: `dark` (white glyph, for dark themes) and
//! `light` (#1E1E1E glyph, for light themes), each in six sizes so the
//! StatusNotifier host can pick what fits.

use std::sync::OnceLock;

pub const SIZES: [u32; 6] = [24, 32, 48, 64, 128, 256];

macro_rules! variant_pngs {
    ($variant:literal) => {
        [
            include_bytes!(concat!("../../../data/icons/travelmode-", $variant, "-24.png")),
            include_bytes!(concat!("../../../data/icons/travelmode-", $variant, "-32.png")),
            include_bytes!(concat!("../../../data/icons/travelmode-", $variant, "-48.png")),
            include_bytes!(concat!("../../../data/icons/travelmode-", $variant, "-64.png")),
            include_bytes!(concat!("../../../data/icons/travelmode-", $variant, "-128.png")),
            include_bytes!(concat!("../../../data/icons/travelmode-", $variant, "-256.png")),
        ]
    };
}

const DARK_PNGS: [&[u8]; 6] = variant_pngs!("dark");
const LIGHT_PNGS: [&[u8]; 6] = variant_pngs!("light");

static DARK_ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
static LIGHT_ICONS: OnceLock<Vec<ksni::Icon>> = OnceLock::new();

/// Pixmaps for the requested variant. `dark = true` selects the white
/// glyph (for dark themes). Returns an empty list if decoding fails;
/// the caller then falls back to the themed icon name.
pub fn icons(dark: bool) -> Vec<ksni::Icon> {
    let cell = if dark { &DARK_ICONS } else { &LIGHT_ICONS };
    cell.get_or_init(|| {
        let pngs = if dark { &DARK_PNGS } else { &LIGHT_PNGS };
        pngs.iter()
            .enumerate()
            .filter_map(|(i, bytes)| {
                decode_png(bytes, SIZES[i])
                    .map_err(|e| {
                        tracing::warn!(size = SIZES[i], dark, error = %e, "icon decode failed");
                        e
                    })
                    .ok()
            })
            .collect()
    })
    .clone()
}

/// Decode one PNG into an ARGB32 pixmap. Only 8-bit RGBA is accepted —
/// that is what data/icons ships.
fn decode_png(bytes: &[u8], expected_size: u32) -> Result<ksni::Icon, String> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "unsupported PNG format {:?}/{:?} (want 8-bit RGBA)",
            info.color_type, info.bit_depth
        ));
    }
    if info.width != expected_size || info.height != expected_size {
        return Err(format!(
            "unexpected PNG size {}x{} (want {expected_size}x{expected_size})",
            info.width, info.height
        ));
    }
    let mut data = buf[..info.buffer_size()].to_vec();
    // RGBA → ARGB (ksni: "ARGB32 format, network byte order").
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    Ok(ksni::Icon {
        width: info.width as i32,
        height: info.height as i32,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_argb_rotation() {
        // Encode a 1x1 RGBA PNG with distinct channel values.
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[10, 20, 30, 40]).unwrap();
        }
        let icon = decode_png(&png_bytes, 1).unwrap();
        assert_eq!((icon.width, icon.height), (1, 1));
        // R,G,B,A = 10,20,30,40 becomes A,R,G,B = 40,10,20,30.
        assert_eq!(icon.data, vec![40, 10, 20, 30]);
    }

    #[test]
    fn embedded_icons_decode_at_all_sizes() {
        for dark in [true, false] {
            let icons = icons(dark);
            assert_eq!(icons.len(), SIZES.len(), "dark={dark}");
            for (icon, &size) in icons.iter().zip(SIZES.iter()) {
                assert_eq!(icon.width, size as i32);
                assert_eq!(icon.height, size as i32);
                assert_eq!(icon.data.len(), (size * size * 4) as usize);
            }
        }
    }

    #[test]
    fn rejects_wrong_size() {
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0u8; 16]).unwrap();
        }
        assert!(decode_png(&png_bytes, 1).is_err());
    }
}
