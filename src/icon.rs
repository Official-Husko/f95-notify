//! The application icon, decoded to raw RGBA by `build.rs`.

pub static RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.rgba"));

/// Edge length in pixels; the embedded icon is square.
pub fn size() -> u32 {
    (RGBA.len() / 4).isqrt() as u32
}
