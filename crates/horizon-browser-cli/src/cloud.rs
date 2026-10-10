//! Direct cloud list commands use the durable MCP plan runner, as orientation does.
use serde_json::json;
use std::{ffi::OsString, path::PathBuf, process::ExitCode, time::Duration};

/// The bound of one cloud list run. Horizon answers a request within seconds.
const TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct Options {
    pub operation: &'static str,
    pub cloud: Option<String>,
    pub output: Option<PathBuf>,
}
impl Options {
    pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let operation = match args.next().as_ref().and_then(|value| value.to_str()) {
            Some("list") => "list",
            Some("attach") => "attach",
            Some("park") => "park",
            Some("stop") => "stop",
            _ => return Err("cloud requires list, attach, park or stop".into()),
        };
        let cloud = if operation == "list" {
            None
        } else {
            Some(
                args.next()
                    .and_then(|value| value.into_string().ok())
                    .filter(|value| !value.is_empty() && !value.starts_with('-'))
                    .ok_or("cloud attach, park and stop require a cloud ID from cloud list")?,
            )
        };
        // The same shape that the request queue accepts, so a malformed ID makes no plan.
        let request = horizon_browser_control::manifest::provider_usage::CloudListRequest {
            operation: serde_json::from_value(json!(operation)).map_err(|_| "cloud operation is not valid")?,
            cloud: cloud.clone(),
        };
        if !request.valid() {
            return Err("cloud IDs are 1 to 128 letters, digits, '.', '_' or '-'".into());
        }
        let mut output = None;
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--output" | "-o") if output.is_none() => {
                    output = Some(PathBuf::from(args.next().ok_or("--output requires a path")?));
                }
                _ => return Err("unexpected or repeated cloud option".into()),
            }
        }
        Ok(Self {
            operation,
            cloud,
            output,
        })
    }
    fn plan(&self) -> serde_json::Value {
        let mut arguments = json!({"operation": self.operation});
        if let Some(cloud) = &self.cloud {
            arguments["cloud"] = json!(cloud);
        }
        json!({"version":1,"steps":[{"id":"cloud","tool":"cloud_list","arguments":arguments}]})
    }
    pub(super) async fn run(self) -> ExitCode {
        let prepared = (|| -> std::io::Result<tempfile::NamedTempFile> {
            let mut file = tempfile::NamedTempFile::new()?;
            serde_json::to_writer(&mut file, &self.plan())?;
            Ok(file)
        })();
        match prepared {
            Ok(file) => super::run(file.path().to_path_buf(), self.output.as_deref(), TIMEOUT).await,
            Err(error) => {
                eprintln!("error: could not prepare cloud plan: {error}");
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
    fn direct_commands_call_the_cloud_list_tool() {
        let options = parse(&["stop", "cloud-1", "-o", "-"]).unwrap();
        let plan: horizon_browser_cli::Plan = serde_json::from_value(options.plan()).unwrap();
        assert_eq!(plan.steps[0].tool, "cloud_list");
        assert_eq!(
            json!(plan.steps[0].arguments),
            json!({"operation": "stop", "cloud": "cloud-1"})
        );
        assert_eq!(options.output, Some(PathBuf::from("-")));
        let list: horizon_browser_cli::Plan = serde_json::from_value(parse(&["list"]).unwrap().plan()).unwrap();
        assert_eq!(json!(list.steps[0].arguments), json!({"operation": "list"}));
    }
    #[test]
    fn malformed_commands_do_not_create_a_plan() {
        let long = "c".repeat(129);
        for args in [
            vec![],
            vec!["delete", "c"],
            vec!["attach"],
            vec!["park", "--output", "-"],
            vec!["list", "c"],
            vec!["stop", "c", "--output"],
            vec!["list", "-o", "-", "-o", "-"],
            vec!["stop", "a/b"],
            vec!["park", &long],
        ] {
            assert!(parse(&args).is_err());
        }
    }
}
