#![forbid(unsafe_code)]

use horizon_net::{Agent, Error, RelayConfiguration, Result};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("horizon-net: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [operation, flag, path] = arguments.as_slice() else {
        return Err(Error::InvalidConfiguration(
            "usage: horizon-net <agent|relay-config> --config PATH".into(),
        ));
    };
    if flag != "--config" {
        return Err(Error::InvalidConfiguration("expected --config PATH".into()));
    }
    match operation.as_str() {
        "agent" => {
            let agent = Agent::bind_persistent(std::path::Path::new(path)).await?;
            agent.online().await?;
            println!("{{\"ready\":true,\"key\":\"{}\"}}", agent.id());
            tokio::signal::ctrl_c().await?;
            agent.close().await;
            Ok(())
        }
        "relay-config" => {
            if tokio::fs::metadata(path).await?.len() > 1_048_576 {
                return Err(Error::MessageTooLarge);
            }
            let config = tokio::fs::read(path).await?;
            let config: RelayConfiguration = serde_json::from_slice(&config)?;
            print!("{}", config.to_toml()?);
            Ok(())
        }
        _ => Err(Error::InvalidConfiguration("unknown operation".into())),
    }
}
