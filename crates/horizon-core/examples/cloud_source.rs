//! Export a committed source fixture through the production transfer implementation.
#![forbid(unsafe_code)]
use horizon_core::cloud_runtime::{Cancellation, Error, Result, command::Runner, repository};
use std::{path::PathBuf, process::ExitCode};
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(Error::Invalid(
            "Usage: cloud_source REPOSITORY REVISION NEW_OUTPUT_DIRECTORY",
        ));
    }
    let source = PathBuf::from(&args[0]).canonicalize()?;
    let output = PathBuf::from(&args[2]);
    std::fs::create_dir(&output)?;
    let revision = repository::resolve(&source, &args[1])?;
    let cancel = Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    repository::validate_tree(&source, &revision, &runner)?;
    repository::pack(&source, &revision, &output.join("source.pack"), &runner)?;
    repository::auxiliary(&source, &revision, &output, &runner)?;
    repository::snapshot(&source, &revision, &output, &runner)?;
    std::fs::write(output.join("revision"), revision)?;
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
