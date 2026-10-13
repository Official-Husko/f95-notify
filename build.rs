//! Decodes `icon.png` at build time into a small raw RGBA buffer, so the
//! binary needs no image decoder at runtime.

use std::{env, fs, path::Path};

/// Edge length of the embedded icon. Tray hosts and notification servers scale it as needed.
const SIZE: usize = 64;

fn main() {
    let src = "icon.png";
    println!("cargo:rerun-if-changed={src}");

    let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(src).expect("open icon.png")));
    let mut reader = decoder.read_info().expect("read icon.png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("icon.png size")];
    let info = reader.next_frame(&mut buf).expect("decode icon.png");
    assert_eq!(info.color_type, png::ColorType::Rgba, "icon.png must be RGBA");
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "icon.png must be 8-bit");

    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = downscale(&buf[..w * h * 4], w, h, SIZE);

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("icon.rgba");
    fs::write(out, rgba).expect("write icon.rgba");
}

/// Alpha-weighted box filter, which avoids dark fringes around transparent edges.
fn downscale(src: &[u8], w: usize, h: usize, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 4);
    for oy in 0..size {
        let (y0, y1) = (oy * h / size, ((oy + 1) * h / size).max(oy * h / size + 1));
        for ox in 0..size {
            let (x0, x1) = (ox * w / size, ((ox + 1) * w / size).max(ox * w / size + 1));
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &src[(y * w + x) * 4..][..4];
                    let pa = p[3] as u64;
                    r += p[0] as u64 * pa;
                    g += p[1] as u64 * pa;
                    b += p[2] as u64 * pa;
                    a += pa;
                    n += 1;
                }
            }
            let avg = |c: u64| c.checked_div(a).unwrap_or(0) as u8;
            out.extend_from_slice(&[avg(r), avg(g), avg(b), (a / n) as u8]);
        }
    }
    out
}
