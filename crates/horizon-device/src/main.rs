#![forbid(unsafe_code)]

#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    horizon_device::cli::run().await
}
