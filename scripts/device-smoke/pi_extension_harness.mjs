#!/usr/bin/env node
/**
 * Fires the lifecycle events of pi into the state report that Horizon gives a pi
 * panel, so a test and a person can see what the report sends.
 *
 * The report is the module that Horizon writes into its own home and that a pi
 * panel loads with `-e`. This harness is the pi side of it without pi: it builds
 * the small `pi` object that an extension factory receives, imports the report,
 * and fires one event for each line that it reads.
 *
 *   node pi_extension_harness.mjs --extension <file> [--session-dir <directory>]
 *
 * One word for each line, and the events that each word fires:
 *
 *   start   input, before_agent_start, turn_start   a message starts work
 *   ask     ui_prompt_start                        a dialog waits for the person
 *   answer  ui_prompt_end                          the person answered
 *   stop    agent_settled {aborted:false}           the run ended
 *   esc     agent_settled {aborted:true}            the run was aborted
 *   quit    session_shutdown {reason:"quit"}         the session ends
 *
 * The harness prints one line for each word and ends after `quit`. It needs the
 * report only, so it runs with the Node that pi itself runs on. Standard
 * library only.
 */

import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { tmpdir } from "node:os";

/** Reads `--name value` from the arguments of this run. */
function option(argv, name) {
  const index = argv.indexOf(name);
  return index >= 0 && index + 1 < argv.length ? argv[index + 1] : undefined;
}

/** The `pi` object that an extension factory receives, with the events it takes. */
function fakePi() {
  const handlers = new Map();
  return {
    handlers,
    pi: {
      on(event, handler) {
        const list = handlers.get(event) ?? [];
        list.push(handler);
        handlers.set(event, list);
      },
      registerTool() {},
      registerCommand() {},
      registerShortcut() {},
      notify() {},
    },
  };
}

/** The session context of a handler, as pi hands it to an extension. */
function context(sessionDir) {
  return {
    sessionManager: {
      getSessionFile: () => null,
      getSessionDir: () => sessionDir,
      getSessionId: () => "harness-session",
      getCwd: () => process.cwd(),
    },
  };
}

async function fire(handlers, event, payload, ctx) {
  for (const handler of handlers.get(event) ?? []) {
    const answer = handler(payload, ctx);
    if (answer && typeof answer.then === "function") await answer;
  }
}

/** The events of one word of the person, in the order that pi fires them. */
const WORDS = {
  start: [
    ["input", { text: "the message of the person is not sent", source: "interactive" }],
    ["before_agent_start", { prompt: "the message of the person is not sent", messageType: "user" }],
    ["turn_start", { turnIndex: 1, timestamp: Date.now() }],
  ],
  ask: [["ui_prompt_start", { reason: "ui_prompt", kind: "confirm", title: "Continue?" }]],
  answer: [["ui_prompt_end", { reason: "ui_prompt" }]],
  stop: [["agent_settled", { aborted: false }]],
  esc: [["agent_settled", { aborted: true }]],
  quit: [["session_shutdown", { reason: "quit" }]],
};

async function main() {
  const argv = process.argv.slice(2);
  const extension = option(argv, "--extension");
  if (!extension) {
    console.log("usage: pi_extension_harness.mjs --extension <file> [--session-dir <directory>]");
    return 2;
  }
  const sessionDir = option(argv, "--session-dir") ?? process.env.TMPDIR ?? tmpdir();
  const { handlers, pi } = fakePi();
  const activate = (await import(pathToFileURL(extension).href)).default;
  activate(pi);
  const ctx = context(sessionDir);
  console.log(`harness: ${handlers.size} events of the report are subscribed; type ${Object.keys(WORDS).join(", ")}`);
  for await (const line of createInterface({ input: process.stdin })) {
    const word = line.trim().toLowerCase();
    if (!word) continue;
    const events = WORDS[word];
    if (!events) {
      console.log(`harness: unknown word ${JSON.stringify(word)}; use ${Object.keys(WORDS).join(", ")}`);
      continue;
    }
    for (const [event, payload] of events) await fire(handlers, event, payload, ctx);
    console.log(`harness: ${word} fired`);
    if (word === "quit") return 0;
  }
  return 0;
}

process.exitCode = await main();
