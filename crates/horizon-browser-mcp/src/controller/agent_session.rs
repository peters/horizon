//! One instruction typed into a cloud session's agent terminal, and one read of that
//! terminal, both answered by the running Horizon through the cloud's recorded SSH route.
use super::BrowserController;
use horizon_browser_control::manifest::cloud_agent::{
    CloudAgentMessageRequest, CloudAgentReadRequest, DEFAULT_READ_LINES,
};
use horizon_browser_control::manifest::provider_usage;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Why a call was refused before the host read it.
const INVALID: &str = "cloud_agent_invalid_request: one instruction of printable text, or one read of 20 to 600 lines, for one cloud and session from cloud_list";
/// Why no answer came. The host may have typed the instruction before it stopped answering,
/// so a caller must read the session instead of assuming that nothing arrived.
const TIMED_OUT: &str = "cloud_agent_timed_out: Horizon did not answer; read the session with cloud_agent_read before sending the instruction again";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudAgentMessageInput {
    /// The cloud's ID from `cloud_list`.
    pub cloud: String,
    /// The panel ID of the session, from `cloud_list`'s `sessions`.
    pub session: String,
    /// One instruction, as printable text on one line.
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudAgentReadInput {
    /// The cloud's ID from `cloud_list`.
    pub cloud: String,
    /// The panel ID of the session, from `cloud_list`'s `sessions`.
    pub session: String,
    /// How many lines back from the visible screen to start, 20 to 600; 120 when omitted.
    #[serde(default = "default_lines")]
    pub lines: u32,
}

fn default_lines() -> u32 {
    DEFAULT_READ_LINES
}

impl BrowserController {
    pub(crate) async fn cloud_agent_message(&self, input: CloudAgentMessageInput) -> Result<Value, String> {
        let request = CloudAgentMessageRequest {
            cloud: input.cloud,
            session: input.session,
            text: input.text,
        };
        if !request.valid() {
            return Err(INVALID.into());
        }
        self.await_cloud_agent(request.answer_millis(), CloudAgentCall::Message(request))
            .await
    }

    pub(crate) async fn cloud_agent_read(&self, input: CloudAgentReadInput) -> Result<Value, String> {
        let request = CloudAgentReadRequest {
            cloud: input.cloud,
            session: input.session,
            lines: input.lines,
        };
        if !request.valid() {
            return Err(INVALID.into());
        }
        self.await_cloud_agent(request.answer_millis(), CloudAgentCall::Read(request))
            .await
    }

    /// Queues one call, then waits for the host's answer. The host runs the call on its own
    /// thread and answers the queue once, so a lost answer leaves no second write behind.
    async fn await_cloud_agent(&self, answer_millis: i64, call: CloudAgentCall) -> Result<Value, String> {
        let invalid = || INVALID.to_string();
        let id = match &call {
            CloudAgentCall::Message(request) => {
                provider_usage::enqueue_cloud_agent_message(self.identity(), request.clone())
            }
            CloudAgentCall::Read(request) => provider_usage::enqueue_cloud_agent_read(self.identity(), request.clone()),
        }
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                "cloud_agent_unavailable: requires a Horizon agent panel".to_string()
            }
            std::io::ErrorKind::InvalidInput => invalid(),
            _ => "cloud_agent_unavailable: could not queue the request".to_string(),
        })?;
        let wait = Duration::from_millis(u64::try_from(answer_millis.saturating_add(1_000)).unwrap_or(u64::MAX));
        let started = Instant::now();
        loop {
            if let Some(result) = self
                .take_host_answer(&id)
                .map_err(|_| "cloud_agent_result_unavailable: Horizon's answer could not be read".to_string())?
            {
                if let Some(error) = result.error {
                    return Err(error);
                }
                return match &call {
                    CloudAgentCall::Message(_) => result.cloud_agent_message,
                    CloudAgentCall::Read(_) => result.cloud_agent_read,
                }
                .ok_or_else(|| "cloud_agent_result_unavailable: Horizon's answer had no result".to_string());
            }
            if started.elapsed() >= wait {
                return Err(TIMED_OUT.into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Which of the two tools a call came from, so its own answer field is read.
enum CloudAgentCall {
    Message(CloudAgentMessageRequest),
    Read(CloudAgentReadRequest),
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser_control::manifest::cloud_agent::{MAX_MESSAGE_BYTES, MAX_READ_LINES, MIN_READ_LINES};
    use horizon_browser_control::manifest::provider_usage::ANSWER_HEADROOM_MILLIS;
    use serde_json::json;

    #[test]
    fn one_instruction_for_one_session_is_the_only_input_of_a_write() {
        let input = serde_json::from_value::<CloudAgentMessageInput>(json!({
            "cloud": "cloud-1", "session": "panel-1", "text": "read #1774 and execute PR 1",
        }))
        .unwrap();
        assert_eq!((input.cloud.as_str(), input.session.as_str()), ("cloud-1", "panel-1"));
        assert!(
            CloudAgentMessageRequest {
                cloud: input.cloud,
                session: input.session,
                text: input.text,
            }
            .valid()
        );
        for field in ["command", "tmux", "path", "user", "attach"] {
            let mut value = json!({"cloud": "c", "session": "p", "text": "go"});
            value[field] = json!("x");
            assert!(
                serde_json::from_value::<CloudAgentMessageInput>(value).is_err(),
                "{field} must not decode"
            );
        }
    }

    #[test]
    fn a_read_defaults_to_its_line_count_and_refuses_more_than_it_carries() {
        let input = serde_json::from_value::<CloudAgentReadInput>(json!({"cloud": "c", "session": "p"})).unwrap();
        assert_eq!(input.lines, DEFAULT_READ_LINES);
        for lines in [MIN_READ_LINES, MAX_READ_LINES] {
            assert!(
                CloudAgentReadRequest {
                    cloud: "c".into(),
                    session: "p".into(),
                    lines,
                }
                .valid()
            );
        }
        for lines in [0, MIN_READ_LINES - 1, MAX_READ_LINES + 1] {
            assert!(
                !CloudAgentReadRequest {
                    cloud: "c".into(),
                    session: "p".into(),
                    lines,
                }
                .valid(),
                "{lines}"
            );
        }
    }

    #[test]
    fn a_lost_answer_asks_the_caller_to_read_before_writing_again() {
        assert!(TIMED_OUT.starts_with("cloud_agent_timed_out"));
        assert!(
            TIMED_OUT.contains("cloud_agent_read") && TIMED_OUT.contains("again"),
            "{TIMED_OUT}"
        );
        let answer = CloudAgentMessageRequest {
            cloud: "c".into(),
            session: "p".into(),
            text: "go".into(),
        }
        .answer_millis();
        assert_eq!(
            answer,
            i64::try_from(horizon_browser_control::manifest::cloud_agent::TIMEOUT_MILLIS).unwrap()
                + ANSWER_HEADROOM_MILLIS
        );
        assert_eq!(MAX_MESSAGE_BYTES, 4 * 1024);
    }
}
