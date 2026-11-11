//! Stand-in programs on a private `PATH`, so no test of the import reads the keyring
//! of the person who runs it.
//!
//! A stand-in is one `/bin/sh` script in a folder of a temporary directory, written
//! with the executable bit, and the environment of the import names that folder as
//! its whole `PATH`. The scripts use no program of the system beyond `/bin/sh`
//! itself and, for the bound, `/bin/sleep`, so a stand-in cannot reach a real
//! keyring even by accident.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::super::ProgramEnvironment;

/// A private folder of stand-in programs and the environment that finds them.
pub(super) struct StandIns {
    folder: TempDir,
}

impl StandIns {
    pub(super) fn new() -> Self {
        Self {
            folder: tempfile::tempdir().expect("a folder for the stand-ins"),
        }
    }

    /// Write `script` as the executable program `name` of this folder.
    pub(super) fn program(&self, name: &str, script: &str) -> PathBuf {
        let path = self.folder.path().join(name);
        fs::write(&path, script).expect("the stand-in program");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("an executable program");
        path
    }

    /// A file of this folder, such as the pid that a stand-in wrote.
    pub(super) fn file(&self, name: &str) -> PathBuf {
        self.folder.path().join(name)
    }

    /// The environment of an import that runs the programs of this folder.
    pub(super) fn environment(&self) -> ProgramEnvironment {
        ProgramEnvironment::with_path(self.root())
    }

    /// The folder of this set of stand-ins.
    pub(super) fn root(&self) -> &Path {
        self.folder.path()
    }
}

/// The program of a name that a test wrote, and a panic that names it when the
/// search of the import does not find it.
pub(super) fn found(stand_ins: &StandIns, name: &str) -> PathBuf {
    stand_ins
        .environment()
        .find(name)
        .unwrap_or_else(|| panic!("`{name}` is a program of this folder"))
}
