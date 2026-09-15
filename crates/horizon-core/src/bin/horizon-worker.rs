//! Structured, durable controller for a dedicated development worker.
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
#[path = "../worker_cli/mod.rs"]
mod worker_cli;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    match worker_cli::run() {
        Ok(value) => {
            println!("{value}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", serde_json::json!({"error": error.to_string()}));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("horizon-worker currently requires Linux");
    std::process::ExitCode::FAILURE
}
