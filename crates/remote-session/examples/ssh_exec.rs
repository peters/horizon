use std::{
    io::{Read, Write},
    process::ExitCode,
    time::Duration,
};

use remote_session::{Completion, Credentials, Error, Event, ExecRequest, Exit, HostKeyPin, Limits, execute};
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

struct Output {
    completion: Completion,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run() -> Result<Output, Error> {
    let args: Vec<_> = std::env::args().skip(1).take(6).collect();
    let [endpoint, username, key_file, pin_file, command] = args.as_slice() else {
        eprintln!("Usage: ssh_exec IP:PORT USER PRIVATE_KEY PINNED_PUBLIC_HOST_KEY 'COMMAND'");
        return Err(Error::InvalidOptions);
    };
    let key = bounded_file(key_file)?;
    let pin = bounded_file(pin_file)?;
    let credentials = Credentials::from_openssh(&key, None)?;
    let limits = Limits::default();
    let request = ExecRequest::new(
        endpoint.parse().map_err(|_| Error::InvalidOptions)?,
        username,
        command,
        HostKeyPin::from_openssh(&pin)?,
        Instant::now() + Duration::from_secs(30),
    )?
    .with_limits(limits)?;
    let mut session = execute(request, credentials)?;
    let Some(input) = session.take_input() else {
        let _ = session.shutdown().await;
        return Err(Error::Finished);
    };
    if let Err(error) = input.eof().await {
        let _ = session.shutdown().await;
        return Err(error);
    }
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    while let Some(event) = session.next_event().await {
        let (buffer, bytes, limit) = match event {
            Event::Stdout(bytes) => (&mut stdout, bytes, limits.stdout),
            Event::Stderr(bytes) => (&mut stderr, bytes, limits.stderr),
            _ => continue,
        };
        if bytes.len() > limit.saturating_sub(buffer.len()) {
            let _ = session.shutdown().await;
            return Err(Error::DataLimit);
        }
        buffer.extend(bytes);
    }
    Ok(Output {
        completion: session.completion().await?,
        stdout,
        stderr,
    })
}

// Synchronous output may wait for the caller's pipe reader. SSH has already
// retired and the runtime has been dropped before either local sink is touched.
fn forward(output: Output) -> Result<Completion, Error> {
    std::io::stdout()
        .write_all(&output.stdout)
        .map_err(|_| Error::Transport)?;
    std::io::stderr()
        .write_all(&output.stderr)
        .map_err(|_| Error::Transport)?;
    Ok(output.completion)
}

fn main() -> ExitCode {
    let result = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(run()),
        Err(_) => Err(Error::Transport),
    };
    match result.and_then(forward) {
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
