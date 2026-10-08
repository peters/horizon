//! Signs in to a GitHub App with the device flow, renews the chain once without a
//! client secret, and checks the new access token. It prints the shape of each
//! answer, never a token. Used by `docs/testing/procedures/github-app-tokens.md`.
//!
//! `HORIZON_GITHUB_CLIENT_ID=<client id> cargo run -p horizon-cloud --example github_token_smoke`
use horizon_cloud::github::{Chain, Client, Poll};
use std::time::{Duration, SystemTime};

fn hours(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::now()).unwrap_or_default().as_secs() / 3600
}

fn describe(label: &str, chain: &Chain) {
    println!(
        "{label}: access token for {} h, refresh token for {} days",
        hours(chain.access_expires_at),
        hours(chain.refresh_expires_at) / 24
    );
}

fn user(token: &str) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    agent
        .get("https://api.github.com/user")
        .header("Authorization", &format!("Bearer {token}"))
        .header("User-Agent", "horizon")
        .call()
        .map_or(0, |response| response.status().as_u16())
}

fn main() {
    let Ok(client_id) = std::env::var("HORIZON_GITHUB_CLIENT_ID") else {
        eprintln!("Set HORIZON_GITHUB_CLIENT_ID to the client ID of the test app.");
        std::process::exit(2);
    };
    let client = Client::new();
    let mut code = match client.start_device(&client_id) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("start: {error}");
            std::process::exit(1);
        }
    };
    println!("Open {} and enter {}", code.verification_uri, code.user_code);
    let chain = loop {
        std::thread::sleep(code.interval);
        match client.poll_device(&client_id, &mut code) {
            Ok(Poll::Granted(chain)) => break chain,
            Ok(Poll::Pending) => {}
            Ok(Poll::SlowDown(interval)) => println!("slow down: now every {} s", interval.as_secs()),
            Err(error) => {
                println!("sign-in ended: {error}");
                std::process::exit(1);
            }
        }
    };
    describe("signed in", &chain);
    println!("first access token: HTTP {}", user(chain.access_token.expose()));
    let renewed = match client.refresh(&client_id, None, &chain.refresh_token) {
        Ok(renewed) => renewed,
        Err(error) => {
            println!("renewal without a secret failed: {error}");
            std::process::exit(1);
        }
    };
    describe("renewed without a client secret", &renewed);
    println!("new access token: HTTP {}", user(renewed.access_token.expose()));
    std::thread::sleep(Duration::from_secs(1));
    println!("old access token: HTTP {}", user(chain.access_token.expose()));
    match client.refresh(&client_id, None, &chain.refresh_token) {
        Ok(_) => println!("old refresh token: still accepted"),
        Err(error) => println!("old refresh token: {error}"),
    }
}
