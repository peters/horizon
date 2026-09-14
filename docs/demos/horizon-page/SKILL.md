---
name: horizon-page
description: Page a Horizon user such as @peters with a short investigation request. Use when you need their machine or workspace context. Does not require Horizon.
---

# Horizon Page

Send a one-shot `PRIVMSG` to a Horizon nick. You do **not** pick the workspace
or panel. The recipient routes the page onto a local or remote workspace, then
either pastes it into an existing agent or creates a new one.

Keep the body to a few sentences. Include the failing system, what you already
tried, and what you need them to look at.

## Message shape

```json
{"cmd":"PRIVMSG","from":"<your nick>","to":"peters","text":"<short request>"}
```

`to` is a nick, not a workspace. Default nick is `peters` unless
`HORIZON_PAGE_NICK` is set.

## How to send

Pick the first method that works.

### 1. HTTP inbox (`HORIZON_PAGE_URL`)

```sh
curl -sS -X POST "${HORIZON_PAGE_URL%/}/msg" \
  -H 'Content-Type: application/json' \
  -d "{\"cmd\":\"PRIVMSG\",\"from\":\"${HORIZON_PAGE_FROM:-devops}\",\"to\":\"${HORIZON_PAGE_NICK:-peters}\",\"text\":\"<short request>\"}"
```

The inbox is the tiny `relay.py` from this folder (local or copied to a
reachable host). There is no auth beyond reaching the URL.

### 2. File drop (`HORIZON_INBOX`)

Write the same JSON to a new file:

```sh
inbox="${HORIZON_INBOX%/}"
mkdir -p "$inbox/incoming"
id="$(date +%s)-$$"
printf '%s\n' "{\"cmd\":\"PRIVMSG\",\"from\":\"${HORIZON_PAGE_FROM:-devops}\",\"to\":\"${HORIZON_PAGE_NICK:-peters}\",\"text\":\"<short request>\"}" \
  > "$inbox/incoming/$id.json"
```

Use this when you share a directory with the recipient (SSHFS, Tailscale,
sync folder). Horizon's product inbox would watch `~/.horizon/inbox/incoming/`.

### 3. Neither is set

Print the JSON object and the `curl` from method 1 so a human can copy it.
Do not pretend the page was delivered.

## Do not

- Guess a workspace, panel, or agent id.
- Send secrets, tokens, or private keys in `text`.
- Retry in a loop. One `PRIVMSG` is the page.
- Use this skill to chat. It is a pager, not IRC.
