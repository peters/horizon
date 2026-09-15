use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

// These paths must match the fonts embedded by src/app/bootstrap.rs.
const REQUIRED_FONTS: &[&str] = &[
    "assets/fonts/InterVariable.ttf",
    "assets/fonts/JetBrainsMono-Regular.ttf",
    "assets/fonts/NotoSansCJKsc-Regular.otf",
    "assets/fonts/NotoSansSymbols2-Regular.ttf",
];
const LFS_POINTER_HEADER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

pub fn validate(manifest_dir: &Path) -> io::Result<()> {
    for relative in REQUIRED_FONTS {
        println!("cargo:rerun-if-changed={relative}");
        check_font(&manifest_dir.join(relative)).map_err(|error| {
            let reason = match error.kind() {
                io::ErrorKind::NotFound => "is missing",
                io::ErrorKind::InvalidData => "is a Git LFS pointer, not a hydrated font",
                _ => "could not be read",
            };
            io::Error::new(
                error.kind(),
                format!(
                    "required embedded font {relative} {reason}. Install Git LFS, then run \
                     `git lfs install` and `git lfs pull` from the repository root and rebuild. \
                     If the file is still missing, restore it from a complete checkout with LFS assets."
                ),
            )
        })?;
    }
    Ok(())
}

fn check_font(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0; LFS_POINTER_HEADER.len()];
    match file.read_exact(&mut header) {
        Ok(()) if header == LFS_POINTER_HEADER => Err(io::Error::new(io::ErrorKind::InvalidData, "Git LFS pointer")),
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
        Err(error) => Err(error),
    }
}
