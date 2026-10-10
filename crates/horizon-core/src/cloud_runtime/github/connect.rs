//! Connect GitHub: creates this machine's GitHub App with the manifest flow.
//!
//! Horizon serves a page on a loopback port that posts the manifest to GitHub. The
//! person clicks **Create GitHub App**; GitHub redirects back with a one-time code,
//! which becomes the app's identity and client secret. The secret goes to a private
//! file; the app's private key is never kept.
use super::{Mode, Settings};
use crate::cloud_runtime::{Error, Result};
use horizon_cloud::github::{Client, Error as GitHubError, Secret};
use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

/// How long Horizon waits for the person to create the app.
const TIMEOUT: Duration = Duration::from_mins(10);
/// The directory in the cloud root for client secrets; each app has its own file, so a
/// new app never replaces the secret of the app that the saved settings name.
const SECRET_DIRECTORY: &str = "credentials";
/// Shown as the app's homepage on GitHub.
const HOMEPAGE: &str = "https://github.com/peters/horizon";
/// Shown on the app's page on GitHub.
const DESCRIPTION: &str = "Gives Horizon cloud workers short-lived access to the repositories you choose. \
                           Horizon created this app on your computer and never stores its private key. \
                           You choose where it is installed and can remove it at any time.";
/// The longest app name GitHub accepts.
const MAX_NAME: usize = 34;

/// A name for a new app: "Horizon for <login>" when the person's GitHub `login` is known
/// and that name fits and is free; otherwise, since GitHub needs app names to be unique, a
/// short random suffix follows "Horizon". Only a public app shows as taken, so GitHub's
/// form may still refuse the name of a private one; the person can change it there, and
/// may rename the app on GitHub later.
#[must_use]
pub fn app_name(login: Option<&str>) -> String {
    name_for(login, |slug| Client::new().app_exists(slug) != Ok(false))
}

/// [`app_name`] where `taken` tells whether an app has a slug; a name that may be taken
/// is not used.
pub(super) fn name_for(login: Option<&str>, taken: impl FnOnce(&str) -> bool) -> String {
    if let Some(login) = login.filter(|login| super::stored::valid_login(login)) {
        let name = format!("Horizon for {login}");
        // GitHub's slug of the name: lowercase, a hyphen for each space.
        if name.len() <= MAX_NAME && !taken(&format!("horizon-for-{}", login.to_ascii_lowercase())) {
            return name;
        }
    }
    format!("Horizon {}", &uuid::Uuid::new_v4().simple().to_string()[..6])
}

/// The GitHub login of a sign-in this computer already keeps, read without the network:
/// the permission to publish images in `docker_config`, else this computer's sign-in for
/// an earlier app in `root`.
#[must_use]
pub fn known_login(root: &Path, docker_config: &Path) -> Option<String> {
    let earlier = std::fs::read_dir(root.join(SECRET_DIRECTORY))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("github-host-") && name.ends_with(".json"))
        });
    std::iter::once(docker_config.join(super::publish::STORE))
        .chain(earlier)
        .find_map(|path| super::stored::load(&path))
        .map(|stored| stored.login)
}

/// Starts the manifest flow and opens its first page with `open`. The receiver gets
/// the new app's settings once GitHub created it.
/// # Errors
/// The loopback port could not be opened.
pub fn start(
    root: &Path,
    name: &str,
    open: fn(&str) -> std::io::Result<()>,
    cancel: crate::cloud_runtime::Cancellation,
) -> Result<Receiver<Result<Settings>>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let state = uuid::Uuid::new_v4().simple().to_string();
    let root = root.to_owned();
    let name = name.to_owned();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(serve(&listener, port, &state, &name, &root, &cancel));
    });
    open(&format!("http://127.0.0.1:{port}/"))?;
    Ok(rx)
}

/// Serves the start page and waits for GitHub's redirect, until `cancel` ends it; a
/// cancelled flow keeps no app.
fn serve(
    listener: &TcpListener,
    port: u16,
    state: &str,
    name: &str,
    root: &Path,
    cancel: &crate::cloud_runtime::Cancellation,
) -> Result<Settings> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        cancel.check()?;
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(code) = answer(stream, port, state, name) {
                    cancel.check()?;
                    return created(&code, root);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Invalid(
                        "GitHub did not create the app in time. Click Connect GitHub again.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Answers one request: the start page posts the manifest, the redirect brings the code.
fn answer(mut stream: TcpStream, port: u16, state: &str, name: &str) -> Option<Secret> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = zeroize::Zeroizing::new(vec![0; 4096]);
    let mut read = 0;
    while read < buffer.len() && !buffer[..read].windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer[read..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => read += count,
        }
    }
    let request = std::str::from_utf8(&buffer[..read]).unwrap_or_default();
    let target = request
        .strip_prefix("GET ")
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_default();
    let (body, code) = if target == "/" {
        (start_page(port, state, name), None)
    } else if let Some(query) = target.strip_prefix("/created?") {
        let code = redirect_code(query, state);
        let text = if code.is_some() {
            "Horizon is creating the connection. You can close this page and return to Horizon."
        } else {
            "Horizon did not expect this request."
        };
        (text.to_owned(), code)
    } else {
        ("Horizon did not expect this request.".to_owned(), None)
    };
    let kind = if target == "/" { "text/html" } else { "text/plain" };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    code
}

pub(super) fn redirect_code(query: &str, state: &str) -> Option<Secret> {
    let mut code = None;
    let mut matched = false;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", value))
                if (1..=100).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric()) =>
            {
                code = Some(Secret::new(value.to_owned()));
            }
            Some(("state", value)) => matched = value == state,
            _ => {}
        }
    }
    code.filter(|_| matched)
}

/// A page that posts the manifest to GitHub as soon as it loads.
pub(super) fn start_page(port: u16, state: &str, name: &str) -> String {
    let manifest = horizon_cloud::github::manifest(
        name,
        DESCRIPTION,
        HOMEPAGE,
        &format!("http://127.0.0.1:{port}/created"),
        "http://127.0.0.1/callback",
    );
    let escaped = html_escape(&manifest.to_string());
    format!(
        "<!doctype html><meta charset=utf-8><title>Connect GitHub</title>\
         <body style=\"font:15px system-ui;background:#0d1117;color:#e6edf3;padding:40px\">\
         <p>Opening GitHub to create your Horizon app…</p>\
         <form id=f method=post action=\"https://github.com/settings/apps/new?state={state}\">\
         <input type=hidden name=manifest value=\"{escaped}\">\
         <button type=submit>Continue to GitHub</button></form>\
         <script>document.getElementById('f').submit()</script>"
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn created(code: &Secret, root: &Path) -> Result<Settings> {
    let app = Client::new()
        .convert_manifest(code)
        .map_err(|error| github_error(&error))?;
    // Named by the app's immutable ID, which no later app reuses.
    let path = root.join(SECRET_DIRECTORY).join(format!("github-app-{}", app.id));
    write_private(&path, app.client_secret.expose())?;
    let settings = Settings {
        app_id: app.id,
        slug: app.slug,
        client_id: app.client_id,
        client_secret_file: path,
        mode: Mode::Ask,
    };
    settings.validate()?;
    Ok(settings)
}

/// Removes the client secret of an app whose settings were not saved. A save can fail
/// after the settings file in `root` already names the app; then, and whenever that
/// file cannot be read, the secret stays, so saved settings never name a missing file.
pub fn discard(root: &Path, settings: &Settings) {
    // Only the path matters here, so settings that fail validation still count.
    let saved = match std::fs::read(root.join("settings.json")) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).map_or(true, |value| {
            value
                .pointer("/github/client_secret_file")
                .and_then(serde_json::Value::as_str)
                == settings.client_secret_file.to_str()
        }),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    };
    if !saved {
        let _ = std::fs::remove_file(&settings.client_secret_file);
    }
}

fn write_private(path: &Path, value: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        // Private before the secret goes in, whatever the umask: nobody else may replace or
        // redirect an entry in it. A link in its place is refused, not followed.
        if std::fs::symlink_metadata(parent)?.file_type().is_symlink() {
            return Err(Error::Invalid("The GitHub secret directory must not be a link"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut options = std::fs::OpenOptions::new();
    // Always a new file: an existing entry, a link included, is refused rather than followed
    // or overwritten, so no saved app's secret is ever truncated.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    // The new entry in its directory must survive a crash too, as the settings that name it.
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn github_error(error: &GitHubError) -> Error {
    match error {
        GitHubError::Transport => Error::Invalid("GitHub could not be reached. Check the network and try again."),
        GitHubError::Refused(_) => Error::Invalid("GitHub refused the new app. Click Connect GitHub again."),
        _ => Error::Invalid("GitHub sent an answer Horizon does not understand."),
    }
}

/// Whether the app permits the device sign-in that [`Mode::Ask`] needs. Starting a
/// device sign-in is the only way to ask; the code it returns is not used.
/// # Errors
/// GitHub could not be reached or refused the app.
pub fn device_flow_enabled(settings: &Settings) -> std::result::Result<bool, GitHubError> {
    match Client::new().start_device(&settings.client_id) {
        Ok(_) => Ok(true),
        Err(GitHubError::DeviceFlowDisabled) => Ok(false),
        Err(error) => Err(error),
    }
}

/// The app's settings page, where the person turns on **Enable Device Flow**.
#[must_use]
pub fn settings_url(settings: &Settings) -> String {
    format!("https://github.com/settings/apps/{}", settings.slug)
}

#[cfg(all(test, unix))]
mod tests {
    use super::write_private;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_secret_directory_is_private_whatever_the_umask_and_never_a_link() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("credentials");
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777)).unwrap();
        write_private(&directory.join("github-app-x"), "synthetic").unwrap();
        assert!(
            write_private(&directory.join("github-app-x"), "other").is_err(),
            "an existing secret is never overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("github-app-x")).unwrap(),
            "synthetic"
        );
        let target = root.path().join("target");
        std::fs::write(&target, "kept").unwrap();
        std::os::unix::fs::symlink(&target, directory.join("github-app-link")).unwrap();
        assert!(write_private(&directory.join("github-app-link"), "synthetic").is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "kept",
            "a link is not followed"
        );
        let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&directory.join("github-app-x")), 0o600);
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let link = root.path().join("linked");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(write_private(&link.join("github-app-x"), "synthetic").is_err());
        assert!(!elsewhere.join("github-app-x").exists());
    }
}
