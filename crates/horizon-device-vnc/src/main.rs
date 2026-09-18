#![forbid(unsafe_code)]
mod backend;
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    horizon_device::cli::run_with_resize(backend::connect).await
}
