//! Render package icons from the same original geometry as the native window. No image inputs.
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = args.next().map(PathBuf::from).ok_or("an explicit NEW absolute output directory is required")?;
    if args.next().is_some() || !root.is_absolute() {
        return Err("use one absolute output directory".into());
    }
    // Atomic creation refuses every existing output, including links and earlier package assets.
    std::fs::create_dir(&root)?;
    let icon = filmcraft_ui_egui::brand::icon();
    let image = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba).ok_or("icon geometry and byte count differ")?;
    let iconset = root.join("Poema.iconset");
    std::fs::create_dir(&iconset)?;
    for size in [16_u32, 32, 128, 256, 512] {
        for scale in [1_u32, 2] {
            let pixels = size * scale;
            let suffix = if scale == 2 { "@2x" } else { "" };
            let name = format!("icon_{size}x{size}{suffix}.png");
            let rendered = image::imageops::resize(&image, pixels, pixels, image::imageops::FilterType::Lanczos3);
            rendered.save(iconset.join(&name))?;
            std::fs::write(
                iconset.join(format!("{name}.attribution")),
                format!(
                    "asset: {name}\ntitle: Poema native caesura icon\nauthor: Poema fork contributors\nsource: original work, crates/ui-egui/src/brand.rs\nlicense: MIT OR Apache-2.0\nlicense-file: LICENSE-MIT and LICENSE-APACHE in the native source distribution\nadded: 2026-10-31 by Poema fork contributors\nnotes: Rendered from the native window geometry; no upstream brand artwork.\n"
                ),
            )?;
        }
    }
    println!("{}", root.display());
    Ok(())
}
