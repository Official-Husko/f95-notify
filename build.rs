//! Build-time asset preparation, so the binaries need no image decoder at runtime:
//! - `icon.rgba`: `icon.png` downscaled to raw RGBA for the tray and notifications
//! - Windows only: a multi-size `.ico`, version info and a DPI-aware manifest,
//!   embedded as resources so Explorer and the taskbar show the app properly

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Edge length of the runtime icon. Tray hosts and notification servers scale it as needed.
const SIZE: usize = 64;
/// Sizes Windows picks from for the taskbar, Explorer and Alt+Tab.
const ICO_SIZES: [usize; 6] = [16, 24, 32, 48, 64, 256];

fn main() {
    let src = "icon.png";
    println!("cargo:rerun-if-changed={src}");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    let png_bytes = fs::read(src).expect("read icon.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(&png_bytes));
    let mut reader = decoder.read_info().expect("read icon.png header");
    let mut buf = vec![0; reader.output_buffer_size().expect("icon.png size")];
    let info = reader.next_frame(&mut buf).expect("decode icon.png");
    assert_eq!(info.color_type, png::ColorType::Rgba, "icon.png must be RGBA");
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "icon.png must be 8-bit");
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = &buf[..w * h * 4];

    fs::write(out.join("icon.rgba"), downscale(rgba, w, h, SIZE)).expect("write icon.rgba");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources(&out, rgba, w, h, &png_bytes);
    }
}

fn windows_resources(out: &Path, rgba: &[u8], w: usize, h: usize, png_bytes: &[u8]) {
    let ico = out.join("icon.ico");
    fs::write(&ico, build_ico(rgba, w, h, png_bytes)).expect("write icon.ico");

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let numeric = parts.iter().map(u16::to_string).collect::<Vec<_>>();

    let manifest = out.join("app.manifest");
    fs::write(&manifest, MANIFEST.replace("{version}", &numeric.join("."))).expect("write manifest");

    let rc = out.join("app.rc");
    let escape = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    fs::write(
        &rc,
        RC.replace("{ico}", &escape(&ico))
            .replace("{manifest}", &escape(&manifest))
            .replace("{numeric}", &numeric.join(","))
            .replace("{version}", &version),
    )
    .expect("write app.rc");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_required().expect("compile Windows resources");
}

/// An ICO with BMP entries for small sizes (understood everywhere) and the
/// original PNG for 256 px (the Vista+ convention for large icons).
fn build_ico(rgba: &[u8], w: usize, h: usize, png_bytes: &[u8]) -> Vec<u8> {
    let images: Vec<(usize, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|&size| {
            if size == 256 && (w, h) == (256, 256) {
                (size, png_bytes.to_vec())
            } else {
                (size, bmp_entry(&downscale(rgba, w, h, size), size))
            }
        })
        .collect();

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]); // reserved, type = icon
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, data) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        ico.extend_from_slice(&[dim, dim, 0, 0]); // width, height, palette, reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        ico.extend_from_slice(&data);
    }
    ico
}

/// A 32-bit BGRA DIB, stored bottom-up, followed by an all-zero AND mask.
fn bmp_entry(rgba: &[u8], size: usize) -> Vec<u8> {
    let mask_row = (size.div_ceil(32)) * 4;
    let mut bmp = Vec::with_capacity(40 + size * size * 4 + mask_row * size);
    bmp.extend_from_slice(&40u32.to_le_bytes()); // header size
    bmp.extend_from_slice(&(size as i32).to_le_bytes());
    bmp.extend_from_slice(&(2 * size as i32).to_le_bytes()); // XOR image + AND mask
    bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
    bmp.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    bmp.extend_from_slice(&[0; 24]); // no compression; remaining fields unused
    for row in rgba.chunks_exact(size * 4).rev() {
        for p in row.chunks_exact(4) {
            bmp.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    bmp.resize(bmp.len() + mask_row * size, 0);
    bmp
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

const RC: &str = r#"1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Official Husko"
      VALUE "FileDescription", "F95 Notify"
      VALUE "FileVersion", "{version}"
      VALUE "LegalCopyright", "GPL-2.0-only"
      VALUE "ProductName", "F95 Notify"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#;

/// Per-monitor DPI awareness keeps the tray menu and login window sharp;
/// Common Controls 6 gives native themed menus.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="OfficialHusko.F95Notify" version="{version}"/>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;
