//! The program of one conversation: the specification that the worker builds for a
//! start, the folder that it runs in, and the codec of its lines.

use horizon_core::agent_bridge::{BridgeIdentity, ClaudeCodec, ProgramSpec, SessionChoice};

/// The program of one conversation: Claude Code, or the program that the
/// environment names in its place, with the same arguments and the identity of
/// the conversation that `local_id` names.
///
/// The identity is the one of the active host: its actor, its host instance, its
/// home and its executable, plus the plugin directory of Claude Code, so the
/// program has the Horizon MCP server and the Horizon skills and draws cards in
/// its own workspace. A local id that the actor form does not accept gives no
/// identity, and the program then draws no card.
///
/// The front desk under [`super::FRONT_DESK`] has an identity of its own and the
/// program of a front desk ([`horizon_core::agent_bridge::claude_for_front_desk`]).
/// That program has no built-in tool, so the conversation writes the words of the
/// person to the work items and does no work itself.
///
/// The frame never calls this function: the identity reads the file system, so
/// the job of [`super::worker`] builds the specification of a start.
pub(in super::super) fn program_spec(
    program: &std::path::Path,
    cwd: &std::path::Path,
    session: SessionChoice,
    local_id: &str,
) -> ProgramSpec {
    let mut spec = if local_id == super::FRONT_DESK {
        let identity = BridgeIdentity::front_desk();
        horizon_core::agent_bridge::claude_for_front_desk(cwd, session, &identity)
    } else if let Some(identity) = BridgeIdentity::for_workspace(local_id) {
        horizon_core::agent_bridge::claude_for_workspace(cwd, session, &identity)
    } else {
        tracing::warn!(local_id, "the workspace of this conversation has no orchestrator actor");
        horizon_core::agent_bridge::claude(cwd, session)
    };
    spec.program = program.to_path_buf();
    spec
}

/// Makes the folder that the program of `local_id` runs in, which the front desk
/// keeps under the Horizon home. The worker of the dock calls this before a start,
/// so no frame reads a directory or makes one. A folder that cannot be made leaves
/// the program in the directory that the job names, and its start names why.
pub(in super::super) fn make_directory(directory: &std::path::Path, local_id: &str) {
    if let Err(error) = std::fs::create_dir_all(directory) {
        tracing::warn!(%error, %local_id, "the folder of this conversation is not there");
    }
}

/// The events of a program that the dock runs, as the line format of Claude
/// Code reads them.
pub(in super::super) fn codec() -> Box<dyn horizon_core::agent_bridge::LineCodec> {
    Box::new(ClaudeCodec::new())
}
