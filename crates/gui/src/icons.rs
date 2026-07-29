//! Embedded tray icons: the PNGs from data/icons are compiled in with
//! include_bytes! and decoded once into the ARGB32 (network byte order,
//! non-premultiplied) pixmaps ksni expects.
//!
//! Two variants ship: `dark` (white glyph, for dark themes) and
//! `light` (#1E1E1E glyph, for light themes), each in six sizes so the
//! StatusNotifier host can pick what fits.

use std::sync::OnceLock;

use relm4::gtk;

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

// ---------------------------------------------------------- UI icons

/// Names of the custom UI icon set (data/icons/ui/*.svg, rendered to
/// PNG by data/icons/render-ui-icons.sh). Referenced by tests and kept
/// as the canonical list for the render/embed pipeline.
#[allow(dead_code)]
pub const UI_ICONS: [&str; 18] = [
    "app-generic", "apps", "blocked", "connection", "connections", "dns",
    "download", "filtering", "gateway", "license", "pause", "plane",
    "play", "session", "settings", "socket", "upload", "uptime",
];

macro_rules! ui_icon_pngs {
    ($($name:literal),*) => {
        fn ui_icon_bytes(name: &str, dark: bool) -> Option<&'static [u8]> {
            match (name, dark) {
                $(
                    ($name, true) => Some(include_bytes!(concat!(
                        "../../../data/icons/ui/rendered/", $name, "-dark-48.png"
                    ))),
                    ($name, false) => Some(include_bytes!(concat!(
                        "../../../data/icons/ui/rendered/", $name, "-light-48.png"
                    ))),
                )*
                _ => None,
            }
        }
    };
}

ui_icon_pngs!(
    "app-generic", "apps", "blocked", "connection", "connections", "dns",
    "download", "filtering", "gateway", "license", "pause", "plane",
    "play", "session", "settings", "socket", "upload", "uptime"
);

/// Themed icon name for a UI icon (registered via gresource in main).
/// Used where only an icon *name* is accepted (view stack tabs).
pub fn ui_icon_name(name: &str, dark: bool) -> String {
    format!(
        "travelmode-ui-{name}-{variant}",
        variant = if dark { "dark" } else { "light" }
    )
}

/// Load a custom UI icon as a texture (48px PNG; display at 24px for
/// crisp rendering at scale factor 2). `dark = true` selects the white
/// glyph. Textures are cheap enough to create per refresh; no cache.
pub fn ui_icon(name: &str, dark: bool) -> Option<gtk::gdk::Texture> {
    let bytes = ui_icon_bytes(name, dark)?;
    match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_static(bytes)) {
        Ok(texture) => Some(texture),
        Err(e) => {
            tracing::warn!(name, dark, error = %e, "ui icon texture failed");
            None
        }
    }
}

/// A 24px image widget showing a custom UI icon.
pub fn ui_image(name: &str, dark: bool) -> gtk::Image {
    let image = gtk::Image::new();
    image.set_pixel_size(24);
    set_ui_icon(&image, name, dark);
    image
}

/// Re-point an image at a custom UI icon (e.g. after a theme change).
pub fn set_ui_icon(image: &gtk::Image, name: &str, dark: bool) {
    if let Some(texture) = ui_icon(name, dark) {
        image.set_paintable(Some(&texture));
    }
}

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
    fn all_ui_icons_are_embedded() {
        // Every declared UI icon must have both variants compiled in.
        // (Decoding to a texture requires a display, so we check the
        // PNG magic bytes here instead.)
        for name in UI_ICONS {
            for dark in [true, false] {
                let bytes = ui_icon_bytes(name, dark)
                    .unwrap_or_else(|| panic!("missing ui icon {name}"));
                assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G'], "{name} not a PNG");
            }
        }
    }

    #[test]
    fn ui_icon_themed_names() {
        assert_eq!(ui_icon_name("plane", true), "travelmode-ui-plane-dark");
        assert_eq!(ui_icon_name("apps", false), "travelmode-ui-apps-light");
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

