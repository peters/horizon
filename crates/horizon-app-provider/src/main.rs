#![forbid(unsafe_code)]
fn main() {
    if std::env::args().nth(1).as_deref() != Some("--tunnel-guard") {
        std::process::exit(2);
    }
    if horizon_app_provider::tunnel_guard::run_guard().is_err() {
        std::process::exit(1);
    }
}
