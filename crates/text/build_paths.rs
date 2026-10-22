//! Path checks for `build.rs`, kept in their own file so the crate's unit tests can run them
//! (`craft_fonts.rs` includes it under `cfg(test)`).

use std::path::{Path, PathBuf};

/// The canonical path of a craft-fonts manifest entry, which must name a file inside the
/// checkout's `fonts/` directory. Every entry is `include_bytes!`-ed into the binary, so an
/// absolute path, a `..` entry or a symlink out of `fonts/` would embed any file on the build
/// host, such as CI credentials (#8608). Canonicalising both sides resolves `..` and symlinks.
pub fn font_file(dir: &Path, file: &str) -> Result<PathBuf, String> {
    let root = dir.join("fonts").canonicalize().map_err(|e| format!("fonts/: {e}"))?;
    let path = dir.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(format!("{file} is not a file inside {}", root.display()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::font_file;

    #[test]
    fn manifest_entries_must_be_files_inside_fonts() {
        let dir = std::env::temp_dir().join(format!("photocraft-font-paths-{}", std::process::id()));
        let fonts = dir.join("fonts/face");
        std::fs::create_dir_all(&fonts).unwrap();
        std::fs::write(fonts.join("A.ttf"), b"font").unwrap();
        std::fs::write(dir.join("secret.txt"), b"token").unwrap();
        let secret = dir.join("secret.txt").canonicalize().unwrap();

        assert!(font_file(&dir, "fonts/face/A.ttf").unwrap().ends_with("A.ttf"));
        for bad in ["secret.txt", "fonts/../secret.txt", "fonts/face/../../secret.txt", "fonts/face", "fonts/missing.ttf"] {
            assert!(font_file(&dir, bad).is_err(), "{bad}");
        }
        assert!(font_file(&dir, &secret.display().to_string()).is_err(), "an absolute path");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
