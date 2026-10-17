use std::{
    io::{Read, Write},
    process::ExitCode,
    time::Duration,
};

use remote_session::{Completion, Credentials, Error, Event, ExecRequest, Exit, HostKeyPin, execute};
use tokio::time::Instant;
use zeroize::Zeroizing;

fn bounded_file(path: &str) -> Result<Zeroizing<String>, Error> {
    let file = std::fs::File::open(path).map_err(|_| Error::InvalidKey)?;
    let mut text = Zeroizing::new(String::new());
    file.take(64 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| Error::InvalidKey)?;
    if text.len() > 64 * 1024 {
        return Err(Error::InvalidKey);
    }
    Ok(text)
}

async fn run() -> Result<Completion, Error> {
    let args: Vec<_> = std::env::args().skip(1).take(6).collect();
    let [endpoint, username, key_file, pin_file, command] = args.as_slice() else {
        eprintln!("Usage: ssh_exec IP:PORT USER PRIVATE_KEY PINNED_PUBLIC_HOST_KEY 'COMMAND'");
        return Err(Error::InvalidOptions);
    };
    let key = bounded_file(key_file)?;
    let pin = bounded_file(pin_file)?;
    let credentials = Credentials::from_openssh(&key, None)?;
    let request = ExecRequest::new(
        endpoint.parse().map_err(|_| Error::InvalidOptions)?,
        username,
        command,
        HostKeyPin::from_openssh(&pin)?,
        Instant::now() + Duration::from_secs(30),
    )?;
    let mut session = execute(request, credentials)?;
    session.take_input().ok_or(Error::Finished)?.eof().await?;
    while let Some(event) = session.next_event().await {
        match event {
            Event::Stdout(bytes) => std::io::stdout().write_all(&bytes).map_err(|_| Error::Transport)?,
            Event::Stderr(bytes) => std::io::stderr().write_all(&bytes).map_err(|_| Error::Transport)?,
            _ => {}
        }
    }
    session.completion().await
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(Completion {
            exit: Exit::Status(status),
            ..
        }) => ExitCode::from(u8::try_from(status).unwrap_or(255)),
        Ok(_) => ExitCode::from(255),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
