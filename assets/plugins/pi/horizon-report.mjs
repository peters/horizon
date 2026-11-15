/**
 * The state report that Horizon gives a pi panel at its launch.
 *
 * Horizon starts a pi panel with `-e <this file>` and the five
 * `HORIZON_WORK_*` variables of that one process. This extension translates the
 * lifecycle events of pi into the lifecycle events that Horizon already records
 * for an agent panel, and sends each one as a single call of
 * `<HORIZON_WORK_EXECUTABLE> --agent-work-hook` with one small JSON document on
 * its standard input. The entry keeps the record, and the sidebar reads it.
 *
 * What this file never does:
 *
 * - it sends no text of the person, no text of the model and no tool input: the
 *   document holds the launch identity, a turn id and an event name;
 * - it writes nothing into the configuration of pi and answers no question of
 *   the person: `project_trust` is not answered here, because the person decides
 *   whether pi trusts the folder;
 * - it waits for nothing. The report is a detached process that pi never joins,
 *   and every error of it ends in this file without a message, so a missing
 *   Horizon, a missing variable or a failed entry cannot slow, print or stop pi;
 * - without the five variables it does nothing at all.
 *
 * A terminal panel in which a person starts `pi` by hand receives no `-e` and
 * therefore no report. That panel keeps the state of its screen.
 */

import { spawn } from "node:child_process";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

/** The argument that makes Horizon record one lifecycle event. */
const ENTRY_ARGUMENT = "--agent-work-hook";

/** The variables that a launch with a report sets, and nothing else may fake. */
const LAUNCH_VARIABLES = [
  "HORIZON_WORK_ROOT",
  "HORIZON_WORK_PANEL",
  "HORIZON_WORK_OWNER",
  "HORIZON_WORK_KIND",
  "HORIZON_WORK_EXECUTABLE",
];

/**
 * One launch of a pi panel: the identity that Horizon registered before this
 * process started, the conversation of pi and the turn of one busy span.
 */
class Launch {
  constructor(executable) {
    this.executable = executable;
    // pi names no conversation to Horizon at the launch, so the identity of the
    // record is this process. It stays the same across a reload of the
    // extensions, and a later process of the same panel holds another one.
    this.session = `pi-process-${process.pid}`;
    this.turns = 0;
    this.promptId = null;
    this.promptSpan = null;
  }

  /** Opens the turn of one busy span, and answers its id. */
  openTurn() {
    this.turns += 1;
    this.promptId = `turn-${process.pid}-${this.turns}`;
    this.promptSpan = null;
    return this.promptId;
  }

  /** Ends the turn of this launch after the agent settled or the session ended. */
  closeTurn() {
    this.promptId = null;
    this.promptSpan = null;
  }

  /** The directory of the panel, which the record keeps as metadata. */
  directory() {
    try {
      return process.cwd();
    } catch {
      return homedir();
    }
  }

  /**
   * The file of the conversation, for the record only: Horizon reads no
   * transcript of pi, and pi keeps no file for a session that was never saved.
   */
  transcript(context) {
    try {
      const manager = context?.sessionManager;
      const file = manager?.getSessionFile?.();
      if (typeof file === "string" && file) return file;
      const directory = manager?.getSessionDir?.() ?? tmpdir();
      return join(directory, `${this.session}.jsonl`);
    } catch {
      return join(tmpdir(), `${this.session}.jsonl`);
    }
  }
}

/**
 * Sends one lifecycle event to the entry of this Horizon. The call is detached
 * and never awaited: pi measures a handler that throws or waits, and this file
 * must neither stop a run nor delay one.
 */
function report(launch, document) {
  try {
    const child = spawn(launch.executable, [ENTRY_ARGUMENT], {
      detached: true,
      stdio: ["pipe", "ignore", "ignore"],
      windowsHide: true,
    });
    child.on("error", () => {});
    if (!child.stdin) return;
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify(document));
    child.unref();
  } catch {
    // A report that cannot be sent changes nothing that the person sees.
  }
}

/**
 * The document of one call: the identity of the launch, the turn and the event
 * name. No prompt, no answer, no tool input.
 */
function document(launch, event, fields, context) {
  return {
    session_id: launch.session,
    prompt_id: fields.promptId ?? null,
    hook_event_name: event,
    reason: fields.reason,
    tool_use_id: fields.toolUseId,
    cwd: launch.directory(),
    transcript_path: launch.transcript(context),
  };
}

/** The five variables of a launch with a report, or `null` for any other launch. */
function launchVariables(environment = process.env) {
  const values = {};
  for (const name of LAUNCH_VARIABLES) {
    const value = environment[name];
    if (!value) return null;
    values[name] = value;
  }
  return values;
}

export default function activate(pi) {
  const variables = launchVariables();
  if (variables === null) return;
  const launch = new Launch(variables.HORIZON_WORK_EXECUTABLE);

  /**
   * The turn of this busy span, opened when the agent starts to work without
   * one. Every closing event names the id that opened its turn, so a late or
   * repeated event cannot open or close another turn of this launch.
   */
  const ensureTurn = (context) => {
    if (launch.promptId !== null) return launch.promptId;
    const promptId = launch.openTurn();
    report(launch, document(launch, "UserPromptSubmit", { promptId }, context));
    return promptId;
  };

  /** The agent works: the first event of a busy span opens its turn. */
  const working = (_payload, context) => {
    ensureTurn(context);
  };
  pi.on("input", working);
  pi.on("before_agent_start", working);
  pi.on("turn_start", working);
  pi.on("tool_execution_start", working);

  // A dialog of an extension waits for the person, and its end answers it. pi
  // never asks for a permission, so this is the only exact wait of a pi panel.
  pi.on("ui_prompt_start", (_payload, context) => {
    const promptId = ensureTurn(context);
    const toolUseId = `ui-prompt-${process.pid}-${launch.turns}`;
    launch.promptSpan = toolUseId;
    report(launch, document(launch, "PermissionRequest", { promptId, toolUseId }, context));
  });
  pi.on("ui_prompt_end", (_payload, context) => {
    if (launch.promptSpan === null || launch.promptId === null) return;
    const promptId = launch.promptId;
    const toolUseId = launch.promptSpan;
    launch.promptSpan = null;
    report(launch, document(launch, "PostToolUse", { promptId, toolUseId }, context));
  });

  // `agent_settled` is the one event that means that the agent stopped working
  // for the message of the person. `agent_end` is not: a retry, a recovery, a
  // compaction or a queued message starts another run.
  pi.on("agent_settled", (event, context) => {
    if (launch.promptId === null) return;
    const promptId = launch.promptId;
    launch.closeTurn();
    const name = event?.aborted === true ? "Interrupt" : "Stop";
    report(launch, document(launch, name, { promptId }, context));
  });

  // The session of this process ends, or pi replaces it. Neither is a claim
  // about work, so the report only records that the session ended.
  pi.on("session_shutdown", (event, context) => {
    launch.closeTurn();
    const reason = event?.reason === "quit" ? "quit" : "other";
    report(launch, document(launch, "SessionEnd", { reason }, context));
  });

  // `project_trust` is not answered here: it tells no end of its wait, so this
  // file cannot report it as a wait, and the screen decides.
}
