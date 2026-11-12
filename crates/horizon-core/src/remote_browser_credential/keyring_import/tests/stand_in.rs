//! Stand-in programs on a private `PATH`, so no test of the import reads the keyring
//! of the person who runs it.
//!
//! A stand-in is one `/bin/sh` script in a folder of a temporary directory, written
//! with the executable bit, and the environment of the import names that folder as
//! its whole `PATH`. The scripts use no program of the system beyond `/bin/sh`
//! itself and, for the bound, `/bin/sleep`, so a stand-in cannot reach a real
//! keyring even by accident.
//!
//! [`keyring`] writes the stand-in of one [`Platform`], so a test reads the keyring of
//! either system on the computer that runs the tests.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::super::Platform;
use super::super::ProgramEnvironment;

/// The item that the stand-in of the Secret Service answers: one item of the service,
/// its account on stderr, in the format of `secret-tool` of libsecret.
///
/// `user` and `value` hold no single quote, because a `/bin/sh` script quotes its
/// argument with one.
fn secret_service(user: &str, value: &str) -> String {
    format!(
        "#!/bin/sh\n\
         printf '%s\\n' '[/org/freedesktop/secrets/collection/login/7]'\n\
         printf '%s\\n' 'label = keyring@horizon-remote-browser'\n\
         printf 'secret = %s\\n' '{value}'\n\
         printf '%s\\n' 'created = '\n\
         printf '%s\\n' 'modified = '\n\
         printf '%s\\n' 'attribute.service = horizon-remote-browser' >&2\n\
         printf '%s\\n' 'attribute.username = {user}' >&2\n"
    )
}

/// The item that the stand-in of the Keychain answers: the value of the exact
/// arguments of one reading on stdout, and status 44 for another account.
fn keychain(user: &str, value: &str) -> String {
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = find-generic-password ] && [ \"$2\" = -s ] && \
         [ \"$3\" = horizon-remote-browser ] && [ \"$4\" = -a ] && \
         [ \"$5\" = '{user}' ] && [ \"$6\" = -w ]; then\n\
         printf '%s\\n' '{value}'\n\
         exit 0\n\
         fi\n\
         exit 44\n"
    )
}

/// Write the stand-in that `platform` reads its keyring with, which answers `value`
/// for the item of `user`.
pub(super) fn keyring(stand_ins: &StandIns, platform: Platform, user: &str, value: &str) {
    let (name, script) = match platform {
        Platform::SecretService => ("secret-tool", secret_service(user, value)),
        Platform::Keychain => ("security", keychain(user, value)),
    };
    stand_ins.program(name, &script);
}

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
    ///
    /// The program is a `/bin/sh` script that the import runs as a program of the
    /// `PATH`, and the import tries an `ETXTBSY` of a script that a parallel test
    /// still holds open again. A test of a recorder that reads its script instead
    /// of executing it uses `horizon_core::stand_in_shell_in`.
    pub(super) fn program(&self, name: &str, script: &str) -> PathBuf {
        let body = script.strip_prefix("#!/bin/sh\n").unwrap_or(script);
        crate::util::stand_in_program_in(self.folder.path(), name, body.trim_end_matches('\n'))
            .expect("the stand-in program")
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
