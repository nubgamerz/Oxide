//! Oxide's icon. `assets/oxide.svg` is the source; `assets/oxide.ico` (16 to 256 px, embedded
//! in the executable by `build.rs` for Explorer) and `assets/oxide-256.rgba` (the window and
//! taskbar icon) are rendered from it by the test below and committed, so a build needs no
//! SVG renderer. After editing the SVG, in pwsh:
//! `$env:UPDATE_DOCS='1'; cargo test -p daw-app icon_is_current; Remove-Item Env:\UPDATE_DOCS`.

/// The size of the window icon's pixels.
const WINDOW_SIZE: u32 = 256;

/// The window and taskbar icon.
pub fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/oxide-256.rgba").to_vec(),
        width: WINDOW_SIZE,
        height: WINDOW_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::WINDOW_SIZE;
    use std::path::Path;

    /// The sizes Windows picks from: Explorer's views, the taskbar, title bars, at any scale.
    const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

    /// The SVG drawn at `size` pixels square, as straight (not premultiplied) RGBA.
    fn render(svg: &[u8], size: u32) -> Vec<u8> {
        let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect()
    }

    /// The committed icon files are what the SVG renders to (or, with `UPDATE_DOCS` set, are
    /// rewritten from it).
    #[test]
    fn icon_is_current() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let svg = std::fs::read(assets.join("oxide.svg")).unwrap();
        let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
        for size in SIZES {
            let image = ico::IconImage::from_rgba_data(size, size, render(&svg, size));
            ico.add_entry(ico::IconDirEntry::encode(&image).unwrap());
        }
        let mut ico_bytes = Vec::new();
        ico.write(&mut ico_bytes).unwrap();
        let rgba = render(&svg, WINDOW_SIZE);
        let files = [("oxide.ico", ico_bytes), ("oxide-256.rgba", rgba)];
        if std::env::var_os("UPDATE_DOCS").is_some() {
            for (name, bytes) in &files {
                std::fs::write(assets.join(name), bytes).unwrap();
            }
            return;
        }
        for (name, bytes) in &files {
            let committed = std::fs::read(assets.join(name)).unwrap_or_default();
            assert!(
                &committed == bytes,
                "assets/{name} is stale: re-render it from oxide.svg (see the module docs)"
            );
        }
    }
}
