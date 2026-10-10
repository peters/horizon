//! Shared headless entry point for the packaged GUI and standalone native host.

fn catalog() -> crate::Result<()> {
    use horizon_core::Config;
    use horizon_core::remote_browser_credential::{CredentialStores, KeyringCredentialStore, SessionCredentialStore};
    let path = Config::resolve_path(None).ok_or(crate::Error::Unavailable)?;
    let text = std::fs::read_to_string(path).map_err(|_| crate::Error::Unavailable)?;
    let config = Config::from_yaml(&text).map_err(|_| crate::Error::Unavailable)?;
    let profile = config
        .browser
        .remote
        .providers
        .get("browserstack")
        .ok_or(crate::Error::Unavailable)?;
    let session = SessionCredentialStore::new();
    let keyring = KeyringCredentialStore::open().map_err(|_| horizon_app_runtime::Error::CredentialsUnavailable)?;
    let account = horizon_app_runtime::account::Account::capture(
        profile,
        &CredentialStores {
            session: &session,
            os_keychain: Some(&keyring),
            environment: None,
        },
    )?;
    let provider = account.provider();
    let devices = provider.devices()?;
    let quota = provider.quota()?;
    let output = serde_json::json!({"devices":devices,"running":quota.parallel_sessions_running,"allowed":quota.parallel_sessions_max_allowed.min(quota.team_parallel_sessions_max_allowed),"queued":quota.queued_sessions});
    serde_json::to_writer(std::io::stdout().lock(), &output).map_err(|_| crate::Error::Unavailable)
}
fn run(arguments: &[String]) -> crate::Result<()> {
    match arguments {
        [argument] if argument == "--guard" => Ok(horizon_app_process::run_guard()?),
        [argument] if argument == "--tunnel-guard" => Ok(horizon_app_provider::tunnel_guard::run_guard()?),
        [argument] if argument == "--catalog" => catalog(),
        [mode, flag, path] if mode == "--run" && flag == "--client" => {
            let host = crate::bootstrap::open(std::path::Path::new(path))?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|_| crate::Error::Unavailable)?
                .block_on(crate::cli::execute(host, std::time::Duration::from_mins(30)))
        }
        [mode, flag, path] if mode == "--reconcile" && flag == "--client" => {
            let result = crate::bootstrap::reconcile(std::path::Path::new(path))?;
            serde_json::to_writer(std::io::stdout().lock(), &result).map_err(|_| crate::Error::Unavailable)
        }
        [mode, flag, path] if mode == "--mcp" && flag == "--client" => {
            let host = crate::bootstrap::open(std::path::Path::new(path))?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|_| crate::Error::Unavailable)?
                .block_on(crate::mcp::serve_stdio(host.actor, &host.evidence, &host.reports))
        }
        _ => Err(crate::Error::Unavailable),
    }
}
/// Dispatch native modes before GUI initialization. Guard children use the same packaged executable.
#[must_use]
pub fn run_if_requested() -> bool {
    let mut arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(first) = arguments.first_mut() else {
        return false;
    };
    if let Some(mode) = first.strip_prefix("--native-") {
        *first = format!("--{mode}");
    } else if !matches!(first.as_str(), "--guard" | "--tunnel-guard") {
        return false;
    }
    execute(&arguments);
    true
}
/// Standalone and packaged modes share identical ownership and error behavior.
pub fn execute(arguments: &[String]) {
    if let Err(error) = run(arguments) {
        if arguments.first().is_some_and(|argument| argument == "--run") {
            if error != crate::Error::RunFailed {
                crate::cli::report_error(&error);
            }
        } else {
            eprintln!("{error}");
        }
        std::process::exit(2);
    }
}
