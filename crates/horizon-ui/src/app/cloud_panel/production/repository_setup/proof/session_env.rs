//! The candidate image's session environment, read without running the image: `docker
//! cp` takes its session-env.d folder out of a created container, and Horizon applies it
//! the way a worker session does. So the CI container starts Horizon's localci directly,
//! and no program of the image runs before it.
use std::process::Command;

/// Where a worker image keeps its session environment.
const FOLDER: &str = "/etc/horizon-worker/session-env.d";

/// The largest session environment file the worker reads, and the most files.
const FILE_LIMIT: u64 = 16 * 1024;
const FILES: usize = 64;

/// The files PAM reads for an SSH session on Ubuntu (`pam_env` with `readenv`), in order.
const PAM_FILES: [&str; 2] = ["/etc/environment", "/etc/default/locale"];

/// What horizon-worker-run exports for every session before it starts the agent, with
/// fixed stand-ins for the values that name the session; HOME the proof sets itself.
pub(super) const WORKER_EXPORTS: [(&str, &str); 7] = [
    ("DISPLAY", ":99"),
    ("HORIZON_BROWSER_ROOT", "/workspace/home/.horizon"),
    ("HORIZON_CLOUD_ACTOR", "cloud:proof"),
    ("HORIZON_BROWSER_ACTOR", "horizon:cloud-proof"),
    ("HORIZON_DEVICE_ACTOR", "horizon:cloud-proof"),
    ("HORIZON_SESSION_DIR", "/workspace/session-data/proof"),
    ("HORIZON_GPU_LOCK", "/workspace/locks/gpu.lock"),
];

/// The PATH sshd gives a session on Ubuntu when /etc/environment names none.
const SSH_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games";

/// A docker command run within the proof's time limit and cancellation, returning what
/// it wrote to standard output.
pub(super) type Run<'a> = &'a dyn Fn(Command) -> Result<Vec<u8>, String>;

/// The `-e` arguments that give a container of `image` the environment a worker session
/// gets; the image is created under the name `container` for the copy and removed
/// again. Every docker command goes through `run`, so a daemon that hangs cannot hold
/// the proof, and what docker copies out arrives as a tar stream that `run` stops at its
/// limit, so an image cannot fill this computer's disk or memory with it.
pub(super) fn arguments(image: &str, container: &str, run: Run<'_>) -> Result<Vec<String>, String> {
    let mut create = Command::new("docker");
    create.args(["create", "--name", container, image]);
    run(create).map_err(|error| format!("cannot read the image's session environment: {error}"))?;
    let folder = copied(container, FOLDER, run);
    let pam_files = PAM_FILES.map(|file| (file, copied_following(container, file, run)));
    let mut remove = Command::new("docker");
    // With the anonymous volumes the image's VOLUME lines made at create.
    remove.args(["rm", "--force", "--volumes", container]);
    let _ = run(remove);
    let folder = folder.map_err(|error| format!("cannot read the image's session environment: {error}"))?;
    // What PAM gives an SSH session from the image's files, in the order it reads them.
    let mut pam: Vec<(String, String)> = Vec::new();
    for (file, copied) in pam_files {
        let copied = copied.map_err(|error| format!("cannot read the image's session environment: {error}"))?;
        if let Some(archive) = copied {
            for (key, value) in pam_file(&archive, file)? {
                pam.retain(|(known, _)| *known != key);
                pam.push((key, value));
            }
        }
    }
    let base = pam
        .iter()
        .find_map(|(key, value)| (key == "PATH").then(|| value.clone()))
        .unwrap_or_else(|| SSH_PATH.to_owned());
    let files = match folder {
        Some(archive) => session_files(&archive).map_err(|error| format!("session environment refused: {error}"))?,
        None => Vec::new(),
    };
    let (variables, directives) = load(&files).map_err(|error| format!("session environment refused: {error}"))?;
    // PAM's variables first, then what horizon-worker-run exports for every session, with
    // stand-ins for the per-session ones, then the image's session-env.d, which may not
    // name the latter, and PATH as the worker builds it.
    let mut arguments: Vec<String> = pam
        .into_iter()
        .filter(|(key, _)| key != "PATH" && key != "HOME")
        .chain(
            WORKER_EXPORTS
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        )
        .chain(variables)
        .flat_map(|(key, value)| ["-e".to_owned(), format!("{key}={value}")])
        .collect();
    arguments.extend(["-e".to_owned(), format!("PATH={}", session_path(&base, &directives))]);
    // A worker session does not inherit the image's ENV, but a container of it does:
    // each variable only the image sets is emptied, read from the image's metadata.
    let mut inspect = Command::new("docker");
    inspect.args(["image", "inspect", "--format", "{{json .Config.Env}}", image]);
    let listed = run(inspect).map_err(|error| format!("cannot read the image's environment: {error}"))?;
    let image_env: Vec<String> = serde_json::from_slice::<Option<Vec<String>>>(&listed)
        .map_err(|error| format!("cannot read the image's environment: {error}"))?
        .unwrap_or_default();
    let mut emptied = image_only(&image_env, &arguments);
    emptied.extend(arguments);
    Ok(emptied)
}

/// The PATH a worker session of `image` gets, read as [`arguments`] reads it.
pub(super) fn session_path_of(image: &str, container: &str, run: Run<'_>) -> Result<String, String> {
    arguments(image, container, run)?
        .iter()
        .rev()
        .find_map(|argument| argument.strip_prefix("PATH=").map(str::to_owned))
        .ok_or_else(|| "the session environment names no PATH".to_owned())
}

/// The `-e KEY=` arguments that empty each variable the image's ENV sets and the session
/// does not, apart from HOME, which the proof sets itself.
fn image_only(image_env: &[String], session: &[String]) -> Vec<String> {
    let set: Vec<&str> = session
        .chunks(2)
        .filter_map(|pair| pair.get(1)?.split_once('=').map(|(key, _)| key))
        .collect();
    let mut keys: Vec<&str> = image_env
        .iter()
        .filter_map(|entry| entry.split_once('=').map(|(key, _)| key))
        .filter(|key| !key.is_empty() && *key != "HOME" && !set.contains(key))
        .collect();
    keys.dedup();
    keys.into_iter()
        .flat_map(|key| ["-e".to_owned(), format!("{key}=")])
        .collect()
}

/// The tar stream of `path` in the created container, or none when the image has no such
/// path.
pub(super) fn copied(container: &str, path: &str, run: Run<'_>) -> Result<Option<Vec<u8>>, String> {
    copy_out(container, path, false, run)
}

/// As [`copied`], following a link at `path` inside the image the way a program that
/// reads the file does; a link that leads nowhere counts as no file.
pub(super) fn copied_following(container: &str, path: &str, run: Run<'_>) -> Result<Option<Vec<u8>>, String> {
    copy_out(container, path, true, run)
}

fn copy_out(container: &str, path: &str, follow: bool, run: Run<'_>) -> Result<Option<Vec<u8>>, String> {
    let mut copy = Command::new("docker");
    copy.arg("cp");
    if follow {
        copy.arg("--follow-link");
    }
    copy.arg(format!("{container}:{path}")).arg("-");
    match run(copy) {
        Ok(archive) => Ok(Some(archive)),
        Err(error) if error.contains("Could not find the file") || error.contains("No such") => Ok(None),
        Err(error) => Err(error),
    }
}

/// One entry of a copied tar stream.
pub(super) enum Unpacked {
    Folder,
    /// A regular file: its bytes and its permission bits.
    File(Vec<u8>, u32),
    Other,
}

/// The entries of a copied tar stream by their paths, each file at most [`FILE_LIMIT`]
/// bytes and no more than [`FILES`] + 1 entries, checked as they are read.
pub(super) fn unpack(archive: &[u8]) -> Result<Vec<(String, Unpacked)>, String> {
    unpack_within(archive, FILE_LIMIT, FILES + 1)
}

/// As [`unpack`], with the limits given.
pub(super) fn unpack_within(
    archive: &[u8],
    file_limit: u64,
    entry_limit: usize,
) -> Result<Vec<(String, Unpacked)>, String> {
    use std::io::Read as _;
    let mut entries = Vec::new();
    let mut tar = tar::Archive::new(archive);
    for entry in tar.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        if entries.len() >= entry_limit {
            return Err(format!("more than {entry_limit} entries"));
        }
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .trim_end_matches('/')
            .to_owned();
        let unpacked = match entry.header().entry_type() {
            tar::EntryType::Directory => Unpacked::Folder,
            tar::EntryType::Regular => {
                let size = entry.header().size().map_err(|error| error.to_string())?;
                if size > file_limit {
                    return Err(format!("{path}: larger than {file_limit} bytes"));
                }
                let mut bytes = Vec::new();
                entry
                    .by_ref()
                    .take(file_limit)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                let mode = entry.header().mode().map_err(|error| error.to_string())?;
                Unpacked::File(bytes, mode)
            }
            _ => Unpacked::Other,
        };
        entries.push((path, unpacked));
    }
    Ok(entries)
}

/// The files of a copied session-env.d folder by name, refused the way the worker
/// refuses them: the folder must be one, and each entry in it a regular file.
fn session_files(archive: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    for (path, entry) in unpack(archive)? {
        let mut parts = path.split('/');
        let (Some(_), name) = (parts.next(), parts.next()) else {
            continue;
        };
        match (name, entry) {
            (None, Unpacked::Folder) => {}
            (None, _) => return Err(format!("{FOLDER}: not a directory")),
            (Some(name), Unpacked::File(bytes, _)) if parts.next().is_none() => files.push((name.to_owned(), bytes)),
            (Some(name), _) => return Err(format!("{name}: not a regular file")),
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// The variables a PAM environment file such as /etc/environment gives a session: each
/// `KEY=VALUE` line, quotes around the value removed; comments, blank lines and lines
/// that name no variable are skipped, as `pam_env` skips them.
fn pam_variables(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            // `export KEY=VALUE` is shell, which pam_env does not read: its name has a
            // space, so it names no variable and is skipped, as pam_env skips it.
            let line = line.trim();
            let (key, value) = line.split_once('=')?;
            (!line.starts_with('#') && variable_name(key)).then(|| (key.to_owned(), value.trim_matches('"').to_owned()))
        })
        .collect()
}

/// The variables of the image's PAM environment file `file`, copied as `archive`. A
/// worker session reads that file whatever it holds, so one the proof cannot read the
/// same way, too large, not UTF-8 or not a regular file, refuses the image instead of
/// the proof running with another environment.
fn pam_file(archive: &[u8], file: &str) -> Result<Vec<(String, String)>, String> {
    // Copied following a link, the one entry is named after the link's target.
    let entries =
        unpack_within(archive, FILE_LIMIT, 1).map_err(|error| format!("the image's {file} is refused: {error}"))?;
    match entries.into_iter().next().map(|(_, entry)| entry) {
        Some(Unpacked::File(bytes, _)) => std::str::from_utf8(&bytes)
            .map(pam_variables)
            .map_err(|_| format!("the image's {file} is not UTF-8")),
        _ => Err(format!(
            "the image's {file} is not a regular file, and a worker session would read it all the same"
        )),
    }
}

/// Session variables, in order, and PATH directives: whether each prepends, and its folder.
type Loaded = (Vec<(String, String)>, Vec<(bool, String)>);

/// The variables and PATH directives of session-env.d files, read with the worker's own
/// rules (horizon-worker-session-env), so a file the worker would refuse fails the proof
/// instead of passing it with another environment.
fn load(files: &[(String, Vec<u8>)]) -> Result<Loaded, String> {
    let mut variables: Vec<(String, String)> = Vec::new();
    let mut directives = Vec::new();
    if files.len() > FILES {
        return Err(format!("{FOLDER}: more than {FILES} files"));
    }
    for (name, bytes) in files {
        if !file_name(name) {
            return Err(format!(
                "{name}: name must be at most 100 letters, digits, \".\", \"_\" or \"-\" ending in .env"
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| format!("{name}: not UTF-8"))?;
        for (number, line) in text.split('\n').enumerate() {
            let at = format!("{name}:{}", number + 1);
            if line.chars().any(char::is_control) {
                return Err(format!("{at}: control character in line"));
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("{at}: expected KEY=VALUE, PATH_PREPEND=DIR or PATH_APPEND=DIR"));
            };
            if value.chars().count() > 4096 {
                return Err(format!("{at}: value longer than 4096 characters"));
            }
            if key == "PATH_PREPEND" || key == "PATH_APPEND" {
                if !value.starts_with('/') || value.contains(':') {
                    return Err(format!("{at}: {key} needs one absolute directory without \":\""));
                }
                directives.push((key == "PATH_PREPEND", value.to_owned()));
            } else if !variable_name(key) {
                return Err(format!("{at}: invalid variable name"));
            } else if reserved(key) {
                return Err(format!("{at}: {key} cannot be set by an image layer"));
            } else {
                variables.retain(|(existing, _)| existing != key);
                variables.push((key.to_owned(), value.to_owned()));
            }
        }
    }
    Ok((variables, directives))
}

/// A session-env.d file name the worker accepts.
fn file_name(name: &str) -> bool {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-');
    name.len() <= 100
        && std::path::Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "env")
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && name.bytes().all(allowed)
}

/// A variable name the worker accepts.
fn variable_name(key: &str) -> bool {
    key.len() <= 128
        && key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The variables the worker sets or clears for every session.
fn reserved(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "HOME"
            | "DISPLAY"
            | "HORIZON"
            | "DISABLE_AUTOUPDATER"
            | "ANTHROPIC_API_KEY"
            | "ANTHROPIC_WORKSPACE_ID"
            | "ANTHROPIC_CUSTOM_HEADERS"
    ) || key.starts_with("HORIZON_")
}

/// The session's PATH: each `PATH_PREPEND` first and each `PATH_APPEND` last, in order, and a
/// directory already on it, from the base or an earlier line, staying where it is.
fn session_path(base: &str, directives: &[(bool, String)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in base.split(':') {
        if !parts.iter().any(|existing| existing == part) {
            parts.push(part.to_owned());
        }
    }
    for (prepend, directory) in directives {
        if parts.contains(directory) {
            continue;
        }
        if *prepend {
            parts.insert(0, directory.clone());
        } else {
            parts.push(directory.clone());
        }
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        entries
            .iter()
            .map(|(name, text)| ((*name).to_owned(), text.as_bytes().to_vec()))
            .collect()
    }

    fn archive(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in entries {
            let mut header = tar::Header::new_gnu();
            if let Some(bytes) = content {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                builder.append_data(&mut header, path, *bytes).unwrap();
            } else {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                builder.append_data(&mut header, path, std::io::empty()).unwrap();
            }
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn session_files_apply_the_way_the_worker_applies_them() {
        let (variables, directives) = load(&files(&[
            (
                "20-tools.env",
                "# tools\nDOTNET_ROOT=/opt/dotnet\nPATH_PREPEND=/opt/dotnet\n",
            ),
            (
                "30-more.env",
                "PATH_APPEND=/opt/extra\nPATH_APPEND=/usr/bin\nDOTNET_ROOT=/opt/other\n",
            ),
        ]))
        .unwrap();
        assert_eq!(
            variables,
            [("DOTNET_ROOT".to_owned(), "/opt/other".to_owned())],
            "a later value replaces"
        );
        assert_eq!(
            session_path("/usr/bin:/bin", &directives),
            "/opt/dotnet:/usr/bin:/bin:/opt/extra"
        );
    }

    #[test]
    fn a_file_the_worker_refuses_fails_the_proof() {
        for (name, text) in [
            ("bad name.env", "A=1\n"),
            ("20-x.env", "PATH=/opt\n"),
            ("20-x.env", "HORIZON_TOKEN=1\n"),
            ("20-x.env", "PATH_PREPEND=relative\n"),
            ("20-x.env", "1X=2\n"),
            ("20-x.env", "A=1\tB\n"),
            ("20-x.env", "no separator\n"),
        ] {
            assert!(load(&files(&[(name, text)])).is_err(), "{name}: {text:?}");
        }
    }

    #[test]
    fn an_environment_file_gives_every_variable_or_refuses_the_image() {
        let file = |bytes: &[u8]| archive(&[("environment", Some(bytes))]);
        let read = |bytes: &[u8]| pam_file(&file(bytes), "/etc/environment");
        assert_eq!(
            read(b"PATH=\"/opt/x:/usr/bin\"\n# note\nJAVA_HOME=/opt/java\nnot a variable\n").unwrap(),
            [
                ("PATH".to_owned(), "/opt/x:/usr/bin".to_owned()),
                ("JAVA_HOME".to_owned(), "/opt/java".to_owned())
            ]
        );
        let through_link = archive(&[("locale.conf", Some(b"LANG=C.UTF-8\n"))]);
        assert_eq!(
            pam_file(&through_link, "/etc/default/locale").unwrap(),
            [("LANG".to_owned(), "C.UTF-8".to_owned())],
            "a link's target is read under its own name"
        );
        assert!(read(b"PATH=/opt/\xff\n").is_err(), "not UTF-8");
        assert!(read(&vec![b'#'; 32 * 1024]).is_err(), "too large");
        assert!(
            pam_file(&archive(&[("environment", None)]), "/etc/environment").is_err(),
            "a folder"
        );
    }

    #[test]
    fn a_variable_only_the_image_sets_is_emptied() {
        let session = vec![
            "-e".to_owned(),
            "DOTNET_ROOT=/opt/dotnet".to_owned(),
            "-e".to_owned(),
            "PATH=/opt/dotnet:/usr/bin".to_owned(),
        ];
        let image_env = ["PATH=/usr/bin", "JAVA_HOME=/opt/java", "DOTNET_ROOT=/old", "HOME=/root"].map(str::to_owned);
        assert_eq!(image_only(&image_env, &session), ["-e", "JAVA_HOME="]);
    }

    #[test]
    fn a_copied_file_keeps_its_permission_bits() {
        let copied = archive(&[("localci", Some(b"binary"))]);
        let unpacked = unpack(&copied).unwrap();
        assert!(
            matches!(&unpacked[..], [(path, Unpacked::File(bytes, 0o644))] if path == "localci" && bytes == b"binary"),
            "the mode a check of the file reads is the one the image has"
        );
    }

    #[test]
    fn a_copied_folder_is_read_within_its_limits() {
        let good = archive(&[("session-env.d", None), ("session-env.d/20-x.env", Some(b"A=1\n"))]);
        assert_eq!(
            session_files(&good).unwrap(),
            [("20-x.env".to_owned(), b"A=1\n".to_vec())]
        );
        let large = vec![b'x'; 20_000];
        let too_large = archive(&[("session-env.d", None), ("session-env.d/20-x.env", Some(&large))]);
        assert!(session_files(&too_large).unwrap_err().contains("larger than"));
        let nested = archive(&[("session-env.d", None), ("session-env.d/sub", None)]);
        assert!(session_files(&nested).unwrap_err().contains("not a regular file"));
        let not_folder = archive(&[("session-env.d", Some(b"A=1\n"))]);
        assert!(session_files(&not_folder).unwrap_err().contains("not a directory"));
        let many: Vec<(String, Option<&[u8]>)> = (0..80)
            .map(|index| (format!("session-env.d/{index:02}.env"), Some(&b""[..])))
            .collect();
        let many: Vec<(&str, Option<&[u8]>)> = many.iter().map(|(name, bytes)| (name.as_str(), *bytes)).collect();
        assert!(session_files(&archive(&many)).unwrap_err().contains("entries"));
    }

    #[test]
    fn pam_skips_what_names_no_variable() {
        assert_eq!(
            pam_variables("LANG=\"C.UTF-8\"\n\n#LANG=x\n1X=2\nexport LC_ALL=C\n"),
            [("LANG".to_owned(), "C.UTF-8".to_owned())],
            "a shell export line is not PAM's"
        );
    }
}
