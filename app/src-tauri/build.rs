use std::path::{Path, PathBuf};

fn main() {
    embed_mobile();
    tauri_build::build()
}

/// Embed the phone app (`npm run build:mobile`, into `dist-mobile`) so the
/// desktop app can serve it with nothing to install beside it. When it has
/// not been built, the table is empty and the server says so.
fn embed_mobile() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist-mobile");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files = Vec::new();
    collect(&dir, &dir, &mut files);
    files.sort();
    let mut out = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (url, path) in files {
        out.push_str(&format!("    ({url:?}, include_bytes!({:?})),\n", path.display().to_string()));
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("mobile_assets.rs");
    std::fs::write(dest, out).unwrap();
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
            out.push((url, std::fs::canonicalize(&p).unwrap_or(p)));
        }
    }
}
