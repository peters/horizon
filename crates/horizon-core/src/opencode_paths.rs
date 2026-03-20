use std::path::PathBuf;

pub(crate) fn opencode_db_path() -> Option<PathBuf> {
    opencode_db_path_with_env(
        std::env::consts::OS,
        env_path("HOME"),
        env_path("USERPROFILE"),
        env_path("XDG_DATA_HOME"),
        env_path("APPDATA"),
    )
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn opencode_db_path_with_env(
    target_os: &str,
    home: Option<PathBuf>,
    user_profile: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    appdata: Option<PathBuf>,
) -> Option<PathBuf> {
    let data_dir = match target_os {
        "macos" => home.map(|path| path.join("Library").join("Application Support")),
        "windows" => appdata.or_else(|| user_profile.map(|path| path.join("AppData").join("Roaming"))),
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
            None,
            Some(PathBuf::from("/tmp/data-home")),
            None,
        );

        assert_eq!(path, Some(PathBuf::from("/tmp/data-home/opencode/opencode.db")));
    }

    #[test]
    fn linux_falls_back_to_local_share() {
        let path = opencode_db_path_with_env("linux", Some(PathBuf::from("/home/tester")), None, None, None);

        assert_eq!(
            path,
            Some(PathBuf::from("/home/tester/.local/share/opencode/opencode.db"))
        );
    }

    #[test]
    fn macos_uses_application_support() {
        let path = opencode_db_path_with_env("macos", Some(PathBuf::from("/Users/tester")), None, None, None);

        assert_eq!(
            path,
            Some(PathBuf::from(
                "/Users/tester/Library/Application Support/opencode/opencode.db"
            ))
        );
    }

    #[test]
    fn windows_uses_appdata() {
        let appdata = PathBuf::from(r"C:\Users\tester\AppData\Roaming");
        let path = opencode_db_path_with_env(
            "windows",
            None,
            Some(PathBuf::from(r"C:\Users\tester")),
            None,
            Some(appdata.clone()),
        );

        assert_eq!(path, Some(appdata.join("opencode").join("opencode.db")));
    }

    #[test]
    fn windows_falls_back_to_userprofile_appdata() {
        let user_profile = PathBuf::from(r"C:\Users\tester");
        let path = opencode_db_path_with_env("windows", None, Some(user_profile.clone()), None, None);

        assert_eq!(
            path,
            Some(
                user_profile
                    .join("AppData")
                    .join("Roaming")
                    .join("opencode")
                    .join("opencode.db")
            )
        );
    }
}
