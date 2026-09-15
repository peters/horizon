#![forbid(unsafe_code)]

use std::{fs, io};

#[path = "../build/font_assets.rs"]
mod font_assets;

const FONT_NAMES: &[&str] = &[
    "InterVariable.ttf",
    "JetBrainsMono-Regular.ttf",
    "NotoSansCJKsc-Regular.otf",
    "NotoSansSymbols2-Regular.ttf",
];

fn fixture() -> io::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    fs::create_dir_all(dir.path().join("assets/fonts"))?;
    for name in FONT_NAMES {
        fs::write(dir.path().join("assets/fonts").join(name), b"\0\x01\0\0")?;
    }
    Ok(dir)
}

#[test]
fn accepts_hydrated_repository_fonts() -> io::Result<()> {
    font_assets::validate(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
}

#[test]
fn reports_each_missing_font_with_recovery_instructions() -> io::Result<()> {
    for name in FONT_NAMES {
        let dir = fixture()?;
        fs::remove_file(dir.path().join("assets/fonts").join(name))?;
        let error = font_assets::validate(dir.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let message = error.to_string();
        assert!(message.contains(&format!("assets/fonts/{name} is missing")));
        assert!(message.contains("git lfs install"));
        assert!(message.contains("git lfs pull"));
        assert!(message.contains("restore it from a complete checkout"));
        assert!(!message.contains(dir.path().to_string_lossy().as_ref()));
    }
    Ok(())
}

#[test]
fn rejects_each_pointer_without_disclosing_contents() -> io::Result<()> {
    for name in FONT_NAMES {
        for newline in ["\n", "\r\n"] {
            let dir = fixture()?;
            let path = dir.path().join("assets/fonts").join(name);
            fs::write(
                &path,
                format!(
                    "version https://git-lfs.github.com/spec/v1{newline}\
                     oid sha256:{}{newline}size 12345{newline}private-test-marker",
                    "0".repeat(64)
                ),
            )?;
            let error = font_assets::validate(dir.path()).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            let message = error.to_string();
            assert!(message.contains(&format!("assets/fonts/{name} is a Git LFS pointer")));
            assert!(message.contains("git lfs install"));
            assert!(message.contains("git lfs pull"));
            assert!(!message.contains("private-test-marker"));
            assert!(!message.contains("sha256"));
            assert!(!message.contains(dir.path().to_string_lossy().as_ref()));
            fs::write(path, b"OTTO")?;
            font_assets::validate(dir.path())?;
        }
    }
    Ok(())
}
