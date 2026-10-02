//! Direct orientation commands use the durable MCP plan runner.
use horizon_browser::remote::RemoteOrientation;
use serde_json::json;
use std::{ffi::OsString, path::PathBuf, process::ExitCode, time::Duration};

pub(super) struct Options {
    pub panel_id: String,
    pub orientation: RemoteOrientation,
    pub timeout_millis: u64,
    pub output: Option<PathBuf>,
}
impl Options {
    pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let panel_id = args
            .next()
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.is_empty() && !value.starts_with('-'))
            .ok_or("orientation requires a panel ID")?;
        let orientation = args
            .next()
            .and_then(|value| value.into_string().ok())
            .and_then(|value| match value.as_str() {
                "portrait" => Some(RemoteOrientation::Portrait),
                "landscape" => Some(RemoteOrientation::Landscape),
                _ => None,
            })
            .ok_or("orientation requires portrait or landscape")?;
        let mut timeout_millis = None;
        let mut output = None;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--timeout-millis") if timeout_millis.is_none() => {
                    let value = args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| (1..=60_000).contains(value))
                        .ok_or("--timeout-millis requires 1..60000")?;
                    timeout_millis = Some(value);
                }
                Some("--output" | "-o") if output.is_none() => {
                    output = Some(PathBuf::from(args.next().ok_or("--output requires a path")?));
                }
                _ => return Err("unexpected or repeated orientation option".into()),
            }
        }
        Ok(Self {
            panel_id,
            orientation,
            timeout_millis: timeout_millis.unwrap_or(RemoteOrientation::DEFAULT_TIMEOUT_MILLIS),
            output,
        })
    }
    fn plan(&self) -> serde_json::Value {
        json!({"version":1,"steps":[{"id":"orientation","tool":"browser_orientation","arguments":{"panel_id":self.panel_id,"orientation":self.orientation,"timeout_millis":self.timeout_millis}}]})
    }
    pub(super) async fn run(self) -> ExitCode {
        let prepared = (|| -> std::io::Result<tempfile::NamedTempFile> {
            let mut file = tempfile::NamedTempFile::new()?;
            serde_json::to_writer(&mut file, &self.plan())?;
            Ok(file)
        })();
        match prepared {
            Ok(file) => {
                super::run(
                    file.path().to_path_buf(),
                    self.output.as_deref(),
                    Duration::from_secs(super::DEFAULT_RUN_TIMEOUT_SECONDS),
                )
                .await
            }
            Err(error) => {
                eprintln!("error: could not prepare orientation plan: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(OsString::from))
    }
    #[test]
    fn direct_command_preserves_mcp_contract_and_deadline() {
        let options = parse(&["panel-1", "landscape", "--timeout-millis", "700", "-o", "-"]).unwrap();
        let plan: horizon_browser_cli::Plan = serde_json::from_value(options.plan()).unwrap();
        assert_eq!(plan.steps[0].tool, "browser_orientation");
        assert_eq!(plan.steps[0].arguments["panel_id"], "panel-1");
        assert_eq!(plan.steps[0].arguments["orientation"], "landscape");
        assert_eq!(plan.steps[0].arguments["timeout_millis"], 700);
        assert_eq!(options.output, Some(PathBuf::from("-")));
        assert_eq!(
            parse(&["panel-1", "portrait"]).unwrap().timeout_millis,
            RemoteOrientation::DEFAULT_TIMEOUT_MILLIS
        );
    }
    #[test]
    fn malformed_commands_do_not_create_a_plan() {
        for args in [
            vec![],
            vec!["p"],
            vec!["p", "sideways"],
            vec!["p", "portrait", "--timeout-millis", "0"],
            vec!["p", "portrait", "--timeout-millis", "60001"],
            vec!["p", "portrait", "--output"],
            vec!["p", "portrait", "--output", "-", "--output", "-"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
}
