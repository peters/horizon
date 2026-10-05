#![forbid(unsafe_code)]

fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--guard"] {
        eprintln!("app_process_invalid");
        std::process::exit(2);
    }
    if let Err(error) = horizon_app_process::run_guard() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
