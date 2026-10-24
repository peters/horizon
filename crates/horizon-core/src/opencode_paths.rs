use std::path::PathBuf;

use crate::local_store::env_path;

pub(crate) fn opencode_db_path() -> Option<PathBuf> {
    opencode_db_path_with_env(std::env::consts::OS, env_path("HOME"), env_path("XDG_DATA_HOME"))
}

fn opencode_db_path_with_env(
    target_os: &str,
    home: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
) -> Option<PathBuf> {
    let data_dir = match target_os {
        "macos" => home.map(|path| path.join("Library").join("Application Support")),
        _ => xdg_data_home.or_else(|| home.map(|path| path.join(".local").join("share"))),
    }?;

    Some(data_dir.join("opencode").join("opencode.db"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::opencode_db_path_with_env;

    #[test]
    fn linux_prefers_xdg_data_home() {
        let path = opencode_db_path_with_env(
            "linux",
            Some(PathBuf::from("/home/tester")),
            Some(PathBuf::from("/tmp/data-home")),
        );

        assert_eq!(path, Some(PathBuf::from("/tmp/data-home/opencode/opencode.db")));
    }

    #[test]
    fn linux_falls_back_to_local_share() {
        let path = opencode_db_path_with_env("linux", Some(PathBuf::from("/home/tester")), None);

        assert_eq!(
            path,
            Some(PathBuf::from("/home/tester/.local/share/opencode/opencode.db"))
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let path = opencode_db_path_with_env("macos", Some(PathBuf::from("/Users/tester")), None);

        assert_eq!(
            path,
            Some(PathBuf::from(
                "/Users/tester/Library/Application Support/opencode/opencode.db"
            ))
        );
    }
}
