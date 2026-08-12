use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::{Error, Result};

pub(crate) fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn user_home_dir() -> Option<PathBuf> {
    user_home_dir_from_env(env_path("HOME"), env_path("USERPROFILE"))
}

pub(crate) fn codex_home_dir() -> Option<PathBuf> {
    codex_home_dir_from_env(env_path("CODEX_HOME"), user_home_dir())
}

fn user_home_dir_from_env(home: Option<PathBuf>, user_profile: Option<PathBuf>) -> Option<PathBuf> {
    home.or(user_profile)
}

fn codex_home_dir_from_env(codex_home: Option<PathBuf>, user_home: Option<PathBuf>) -> Option<PathBuf> {
    codex_home.or_else(|| user_home.map(|home| home.join(".codex")))
}

pub(crate) fn codex_db_path() -> Option<PathBuf> {
    codex_db_path_in(&codex_home_dir()?)
}

fn codex_db_path_in(codex_home: &Path) -> Option<PathBuf> {
    let supported = codex_home.join("state_5.sqlite");
    supported.is_file().then_some(supported)
}

pub(crate) fn open_read_only_sqlite(path: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(path, flags).map_err(|error| Error::State(error.to_string()))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{codex_db_path_in, codex_home_dir_from_env, user_home_dir_from_env};

    #[test]
    fn codex_store_uses_the_supported_state_database() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("state_5.sqlite"), []).expect("create current db");
        std::fs::write(temp.path().join("state_6.sqlite"), []).expect("create future db");

        assert_eq!(codex_db_path_in(temp.path()), Some(temp.path().join("state_5.sqlite")));
    }

    #[test]
    fn codex_store_does_not_open_an_unknown_schema() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("state_6.sqlite"), []).expect("create db");
        std::fs::write(temp.path().join("state_7.sqlite"), []).expect("create newer db");

        assert_eq!(codex_db_path_in(temp.path()), None);
    }

    #[test]
    fn codex_store_ignores_a_directory_with_the_supported_name() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::create_dir(temp.path().join("state_5.sqlite")).expect("create misleading directory");

        assert_eq!(codex_db_path_in(temp.path()), None);
    }

    #[test]
    fn user_home_falls_back_to_the_windows_profile() {
        let profile = std::path::PathBuf::from(r"C:\Users\tester");

        assert_eq!(user_home_dir_from_env(None, Some(profile.clone())), Some(profile));
    }

    #[test]
    fn codex_home_honors_the_explicit_override() {
        let codex_home = std::path::PathBuf::from("/stores/codex");
        let user_home = std::path::PathBuf::from("/Users/tester");

        assert_eq!(
            codex_home_dir_from_env(Some(codex_home.clone()), Some(user_home)),
            Some(codex_home)
        );
    }
}
