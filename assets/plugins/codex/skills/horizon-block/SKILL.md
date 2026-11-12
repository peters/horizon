---
name: horizon-block
description: Draw a card of plain text in the calling agent's Horizon workspace with the block_panel MCP tool, and inspect, update, hide or close it. A person's click on a button or a selectable row reaches the agent that owns the card through next_click, which can wait on every card of the agent at once. A link on a card opens a page in a browser panel of Horizon, and it sends no click. Browser pages use horizon-browser and native desktops use horizon-device.
---

# Horizon cards

A card is a small surface of plain text that an agent composes in the person's
Horizon workspace. Use a card to show the state of the work that the agent
does: a machine that it holds, a plan, a list of results, a figure that it
computed. A card is not a terminal, a browser page or a native desktop, and it
takes no text input. A link on a card opens a page in a browser panel of Horizon
when the person presses it.

The `block_panel` MCP tool manages cards. Every operation is scoped to the
agent panel that Horizon launched and to its workspace. A caller without that
panel gets `caller_unavailable`.

`block_panel` operations are `create`, `update`, `list`, `inspect`,
`next_click`, `visibility`, `close`, `open_terminal`, `open_panel`,
`machine_set`, `machine_delete`, `machines`, `machine_key`, `machine_keys` and
`delete_machine_key`.

## Create a card

The card is drawn in the caller's workspace. It does not take keyboard focus.

```json
{"operation": "create", "title": "cloud-1",
 "document": {"components": [
   {"type": "heading", "text": "cloud-1", "subtitle": "Hetzner · Helsinki",
    "badge": {"text": "Running", "tone": "ok"}},
   {"type": "badge", "text": "running", "tone": "ok"},
   {"type": "kv", "rows": [
     {"label": "Address", "value": "203.0.113.10", "mono": true},
     {"label": "Price", "value": "0.42 EUR/h"}]},
   {"type": "text", "text": "Billed from 14:02 UTC."},
   {"type": "progress", "label": "Hour 3 of 24", "percent": 12, "detail": "Saving"},
   {"type": "list", "selection": "single", "rows": [
     {"id": "size.cx22", "title": "CX22 - 2 vCPU", "subtitle": "4 GB memory"},
     {"id": "size.cx32", "title": "CX32 - 4 vCPU",
      "subtitle": "gcloud is not installed", "tone": "warn"}]},
   {"type": "link", "label": "Open session", "url": "https://cloud.example.test/sessions/7"},
   {"type": "buttons", "items": [
     {"id": "connect", "label": "Connect", "tone": "primary"},
     {"id": "delete", "label": "Delete", "tone": "danger"}]}]}}
```

Keep the returned `panel_id`. The answer is `created` with `panel_id` and
`revision`: the first revision is 1.

An agent owns at most eight cards in one workspace. A ninth card is refused
with `too_many_cards`; close a card that the person does not need.

## Update a card

`update` replaces the whole document. It does not merge with the old one. Read
the returned `revision`; it increases with every new document.

```json
{"operation": "update", "panel_id": "<panel_id>",
 "document": {"components": [{"type": "text", "text": "Hour 4 of 24."}]}}
```

Only the agent that created a card may update it, hide it, show it or close it.
Another agent of the same workspace gets `not_owner`.

## List and inspect cards

`list` answers every card of the calling workspace, with `panel_id`, `title`,
`revision`, `owned_by_caller`, `pending_clicks` and `listening`.

```json
{"operation": "list"}
```

`inspect` answers one card with its document, its revision, `visible`,
`agent_present`, `owned_by_caller`, `pending_clicks` and `listening`.
`agent_present` is false when no agent panel of the owner is in the workspace
any more: the person then keeps the last document with no agent behind it, and
the buttons and rows of the card are disabled. `pending_clicks` counts the
clicks that wait, and `listening` is true while a `next_click` of the owner
waits on that card, alone or on every card of the owner.

```json
{"operation": "inspect", "panel_id": "<panel_id>"}
```

List and inspect work for every agent of the workspace. A card of another
workspace answers `panel_unavailable`, and a panel that is not a card answers
`not_block_panel`.

## Hide, show and close a card

`visibility` hides or shows a card that the caller owns. A hidden card keeps its
document and its revision.

```json
{"operation": "visibility", "panel_id": "<panel_id>", "visible": false}
```

`close` removes a card that the caller owns. Its document is lost. Read what you
need from `inspect` before you close it.

```json
{"operation": "close", "panel_id": "<panel_id>"}
```

## Connect an SSH terminal

`open_terminal` opens an SSH terminal panel for the person. The panel goes in the
calling workspace, beside your agent panel, and it does not take keyboard focus.
Use it when the person needs a shell on a machine that you set up. Name either a
route with `ssh` or a machine of this Horizon with `machine`, never both and never
neither: the other order is `invalid_request`, and the message names the field.

```json
{"operation": "open_terminal",
 "ssh": {"host": "lab-1", "user": "deploy", "port": 2222},
 "remote_command": "tmux new -A -s build", "title": "lab-1 build"}
```

The answer is `opened` with the `panel_id` of the terminal.

Give the person a session that survives a disconnect: pass a remote command that
attaches or creates a terminal multiplexer session. Horizon adds `-t`, so the
program gets a terminal:

```
ssh lab-1 -t tmux new -A -s build
```

- `ssh.host` (required) and `ssh.user` take letters, digits, `.`, `_` and `-`,
  and `:` in the host for an IPv6 address. A label cannot start with `-` and
  holds no space and no control character.
- `ssh.port` is 1 to 65535. Omit it to use the SSH configuration of the person.
- `remote_command` is one line of letters, digits, spaces and the characters
  `.`, `_`, `:`, `/`, `,`, `=`, `@`, `+` and `-`, at most 512 characters. It
  cannot start with `-`. Do not use a quote, `;`, `&`, `|`, `$`, a backtick,
  `<`, `>`, `(`, `)`, a backslash or a line break.
- `title` is one line of at most 80 characters. It defaults to `user@host`.

You can pass no other field. Horizon builds the `ssh` command line itself, so
you cannot add an ssh option, an identity file, an argument or an environment.
With `machine`, the record holds the route, the key and the tmux session, so
pass no `ssh`, `key` or `remote_command`; each of them is `invalid_request`.

Tell the person on a card what the terminal is for, and what to do in it. The
titlebar of the panel carries the mark `opened by <agent>`, so the person knows
that you opened it. The panel is an ordinary SSH terminal after that: the person
types in it and closes it. You cannot read it, write to it or close it with this
tool.

## Open a panel of a preset

`open_panel` opens a panel of one preset of the person's settings in one folder
of this computer. Use it when a task of the person runs in its own folder, such
as a git worktree of a repository:

```json
{"operation": "open_panel", "preset": "<a preset of the settings>",
 "directory": "/abs/path/of/the/worktree", "title": "fix login"}
```

- `preset` is the name or the alias of a preset of the settings, exactly. Horizon
  takes the command, the arguments and the kind from that preset, so you send no
  command, no argument, no environment and no prompt. A name that the settings do
  not hold is `unknown_preset`, and the message lists the names of the presets
  and no command.
- `directory` is an absolute path of at most 1 024 bytes with no control
  character. It must be a directory of this computer, and it becomes the working
  directory of the panel. A folder that is not there is `directory_missing`.
- `title` is one line of at most 80 characters. It defaults to the name of the
  preset.
- A preset of a kind that opens no program of this computer, such as a browser
  page, a native viewer, a card or an SSH terminal, is `invalid_request`. Ask for
  a shell, a command or an agent preset.
- One agent panel opens at most eight panels in ten minutes. The ninth answer is
  `rate_limited` with the seconds to wait.

The answer is `opened` with the `panel_id`. The panel goes in your workspace,
beside your agent panel, it does not take keyboard focus, and its titlebar
carries the mark `opened by <agent>`. The panel is the person's afterwards: this
tool cannot read it, write to it or close it.

For the worktrees of a repository, use the `horizon-worktree` skill: it names
the branch, the folder, the card and the rules of a merge and of a remove.

## Register a machine

A machine record lets the person connect to a host that you set up with one
click in the sidebar, when your agent panel is gone. The list belongs to the
person's Horizon and not to a workspace or a session: every agent panel of this
Horizon sees it, it survives a restart, and Horizon draws it below the
workspaces.

```json
{"operation": "machine_set",
 "name": "build-1",
 "ssh": {"host": "203.0.113.10", "user": "root"},
 "key": "build-1",
 "session": "build-1",
 "provider": "Hetzner, hel1"}
```

The answer is `machine_set` with `name` and `created`, which is false when the
name was already there and the record replaced the old one.

- `name` follows the machine key rule: 1 to 40 characters of `a-z`, `0-9` and
  `-`, with no `-` at either end. A name that breaks the rule is
  `invalid_machine_name`.
- `ssh` holds `host` and optional `user` and `port`, as `open_terminal` takes
  them, and nothing else. A route that breaks a rule is `invalid_ssh_route`.
- `key` is the name of a machine key of this Horizon, as `machine_key` reports
  it, and it must be there. A key name that breaks the rule is
  `invalid_key_name`.
- `session` is a tmux session name: 1 to 40 characters of `A-Z`, `a-z`, `0-9`,
  `_` and `-`, and no `-` in front. The terminal of the machine runs
  `tmux new -A -s <session>`, so the work survives the terminal. A name that
  breaks the rule is `invalid_session_name`.
- `provider` is one line of at most 60 characters, such as `Hetzner, hel1`. It
  is your claim, not verified Horizon state, and Horizon draws it dimmed under
  the machine name.
- The record holds no command line. Horizon builds the `ssh` argv and the tmux
  command from the record when a panel opens, so a record can never run a
  command that you saved in it.
- One Horizon keeps at most 64 machines. The 65th name is `too_many_machines`;
  ask the person to forget one.

```json
{"operation": "machines"}
```

`machines` answers every record in name order, each with `name`, `host`,
`user`, `port`, `key`, `session`, `provider`, `added_at_millis`,
`terminal_open` and, when one waits, `pending`. `terminal_open` says whether a
terminal of that machine is open in this Horizon now. It is not a reachability
check, and Horizon never polls a machine. `pending: true` says that the server
of the person has not decided the last change to that machine yet; it settles by
itself. The answer also carries `server`: `off` when this Horizon keeps its
machine list on this computer alone, or `connecting`, `in_step`, `behind` or
`away`.

A Horizon that follows a server shares its machine list with the other computers
of the person. A machine that another computer added appears in the Machines
section here too, and it may have no key on this computer: a click on it opens
nothing until that computer has the key, and Horizon says so.

```json
{"operation": "machine_delete", "name": "build-1"}
```

`machine_delete` forgets one record and answers `name`. It never deletes the
key: call `delete_machine_key` for that. An unknown name is
`machine_unavailable`. A terminal that the machine opened keeps its own saved
connection and still works after the record is gone.

Connect a registered machine with `open_terminal` and `machine`, and pass no
`ssh`, `key` or `remote_command`, because the record holds them. The panel that
opens is the panel that a click on the machine row opens, so a second click
focuses it instead of opening a second terminal.

```json
{"operation": "open_terminal", "machine": "build-1"}
```

## Keep a key for a machine

A machine that you rent must not hold the person's own key, and the person's ssh
agent must not offer a long list of keys to it. Horizon keeps one key pair per
machine name in its own home, and a terminal that names the key connects with
that key alone.

Ask for a key, put its public line on the machine at rent time, then open the
terminal with it:

The request:

```json
{"operation": "machine_key", "name": "build-1"}
```

The answer:

```json
{"status": "machine_key", "name": "build-1",
 "public_key": "ssh-ed25519 AAAA... horizon-machine-build-1",
 "private_key_path": "<Horizon home>/machine-keys/build-1",
 "known_hosts_path": "<Horizon home>/machine-keys/build-1.known_hosts",
 "created": true}
```

1. Call `machine_key` with a name such as `build-1`.
2. Put `public_key` in the `authorized_keys` of the machine, at rent time, from
   the console or the cloud-init of the provider.
3. Run the few commands that the machine needs with your own `ssh`, as
   [Your own commands on the machine](#your-own-commands-on-the-machine) says.
   Wait until SSH answers before you show the person a Connect button.
4. Call `open_terminal` with `"key": "build-1"`:

   ```json
   {"operation": "open_terminal", "key": "build-1",
    "ssh": {"host": "203.0.113.10", "user": "root"},
    "remote_command": "tmux new -A -s build"}
   ```

5. Call `delete_machine_key` after you delete the machine.

```json
{"operation": "machine_keys"}
```

```json
{"operation": "delete_machine_key", "name": "build-1"}
```

- `name` is 1 to 40 characters of `a-z`, `0-9` and `-`, and cannot start or end
  with `-`. A name that breaks the rule is `invalid_key_name`.
- One Horizon home holds at most 64 keys. The 65th name is `too_many_keys`;
  delete a key that you no longer need.
- Horizon makes an ed25519 pair with no passphrase, under its own home. The
  answer holds the public line and the absolute paths `private_key_path` and
  `known_hosts_path`; it never holds the text of the private key. `created` is
  false when the pair was already there, and the same public key comes back.
- You pass no path, no passphrase, no key file and no ssh option. Horizon adds
  `-i`, `IdentitiesOnly=yes`, `IdentityAgent=none`, its own known-hosts file for
  that key, and `StrictHostKeyChecking=accept-new` to the command line that it
  builds. The machine sees that one key, and the ssh agent of the person is not
  offered.
- The keys belong to the Horizon home and not to a workspace, so every agent
  panel of this Horizon can use any of them by name: an agent that starts later,
  in any workspace, reaches a machine that you rented.
- `machine_keys` answers `keys`, each with `name`, `public_key`,
  `private_key_path`, `known_hosts_path` and `created_at_millis`.
- `delete_machine_key` removes the pair and its known-hosts file, and answers
  `name`. An unknown or deleted key is `key_unavailable`. Call it after you
  delete the machine.
- The text of the private key never appears in an answer, a log line or a
  card. The path does, because you run as the same local user as Horizon and can
  read the file anyway.

The key is trust on first use: the first login with a key pins the host key in
the known-hosts file of that key, and a later change refuses the login. Horizon
does not verify the machine.

## Your own commands on the machine

A rent leaves a few things to do on the machine: wait until SSH answers, check
that a program is there, install one. Run those commands yourself, with the
private key that `machine_key` named. Use this form for every connection of your
own:

```
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> '<command>'
```

- `IdentitiesOnly=yes` and `IdentityAgent=none` offer that one key only. A long
  ssh agent of the person exhausts `MaxAuthTries` and the login fails without
  them.
- `UserKnownHostsFile` and `StrictHostKeyChecking=accept-new` use the known-hosts
  file of that key, so the first host key is pinned there and the person's own
  file is untouched.
- `BatchMode=yes` fails at once instead of asking for a password, and
  `ConnectTimeout=10` keeps a dead address from hanging.
- Pass `<command>` as one argument. Never put a credential, a token or the
  private key text in a command line, a log or a card.
- A connection that fails is normal in the first minute: retry the same form
  every 10 seconds, for at most 3 minutes, and tell the person what you wait for
  on the card with a `progress` component. Change nothing about the machine's
  own ssh configuration.

The person's own ssh never needs this file: `open_terminal` with `key` builds
the same options itself, and the person types in that panel.

## The document

A document holds components in draw order, and only these components:

- `heading`: `text`, optional `subtitle` and optional `badge` with `text` and
  optional `tone`. The badge is drawn on the title row, on the right.
- `text`: `text`. Body text; the only text that may hold line breaks.
- `kv`: `rows` with `label`, `value` and optional `mono`. A `mono` value is
  drawn in the monospace family: use it for an address, a command or a hash.
- `badge`: `text` and optional `tone`. A badge on a row of its own.
- `progress`: `label`, `percent` and optional `detail`, drawn on the right of
  the label row.
- `divider`: no fields.
- `list`: `selection` (`none` or `single`) and `rows` with `id`, `title`,
  optional `subtitle`, optional `tone` for the colour of the subtitle and
  optional `selected`. A row with `selected: true` is the row that the list
  starts with: a click reports it until the person selects another row. Only a
  list with `selection: single` may hold a selected row, and at most one row of
  a list.
- `buttons`: `items` with `id`, `label` and optional `tone`.
- `link`: `label` and `url`. The label is one line of 1 to 80 characters. The
  URL is an absolute `http` or `https` URL with a host, 1 to 2048 characters,
  with no credentials (`user:pass@`), no space and no control character. The card
  draws the label as a link and the host of the URL under it, so the person sees
  where it goes. A press opens the page in a browser panel of the card's
  workspace, beside the card, and it sends no click to the agent. Use a link for
  a page the person should open, and never put a token or a signed URL in it.

The limits are:

- 32 KiB for the whole serialized document.
- 64 components.
- 24 rows in one `kv` and 32 rows in one `list`.
- 8 buttons in one `buttons`, and 8 `link` components in one card.
- 512 characters in one text, including `subtitle`, the `badge` text and
  `detail`.
- Ids from `[a-z0-9_.-]`, at most 64 characters, unique in the whole document.
- A `tone` from `neutral`, `info`, `ok`, `warn`, `danger` or `primary`.
- A `percent` from 0 to 100.
- A `selected` row only in a list with `selection: single`, at most one row of
  a list.

Only the first `primary` button of a card is drawn as the primary action; a
later one is drawn as a neutral button. A button with tone `danger` is drawn at
the right edge of its row. Horizon draws every badge in the neutral tone when
the owner of the card is gone, with the prefix `Last seen: `.

An unknown field, component or field value is refused. There are no inputs,
images or tables, and Horizon fetches nothing and opens nothing until the person
presses a link.

A card is plain text that Horizon draws. It is not verified Horizon state: the
person reads the agent's claim. Do not show a credential, a token or a private
key on a card.

## Read a click

A person can press a button, and select a row in a list that has
`selection: single`. A selection is Horizon's own state: it is not saved, and a
new document drops a selection whose row is gone. A press on a selectable row
changes the selection and is also a click. A list whose row has `selected: true`
starts with that row selected, until the person selects another row.

`next_click` waits for the next click on a card that the caller owns. Leave out
`panel_id` and it waits on every card that the caller owns, oldest click first:
that is the normal listen loop, because the person presses whichever card they
see. It answers as soon as a click waits, or with `timed_out` when its wait ends.

```json
{"operation": "next_click", "panel_id": "<panel_id>", "timeout_millis": 300000}
```

```json
{"operation": "next_click", "timeout_millis": 300000}
```

The wait is 1000 to 3600000 milliseconds; the default is 300000. A click names
the button or row that the person hit and the document revision that they saw:

```json
{"status": "clicked", "click": {
  "event_id": 4, "panel_id": "<panel_id>", "component_id": "continue",
  "revision": 7, "selected": ["size.cx32"],
  "at_millis": 1700000000000, "stale": false}}
```

- `component_id` is the button id or list row id that the person hit.
- `revision` is the document revision that the person saw, not the current one.
- `selected` holds the row ids that were selected at that moment, in document
  order. It holds the row that the document marked `selected` until the person
  selects another one. This is how a button such as Continue tells the agent
  which option the person chose.
- `event_id` counts the clicks of one card from 1. Two clicks on the same
  component have different ids.
- `stale` is true when the document changed after the person pressed. Confirm
  the intent before you act on a stale click.

A click reaches an agent at most once. Keep a listen loop:

1. Create or update the card with the choice that the person must make.
2. Call `next_click` without `panel_id`, with these exact field names and no
   other field:

   ```json
   {"operation": "next_click", "timeout_millis": 300000}
   ```

3. Act on the click, and send the result with `update`.
4. Call `next_click` again.
5. On `timed_out`, call `next_click` again while the card matters, or stop and
   tell the person in the terminal.

One `next_click` waits per card, and one without `panel_id` waits per agent: a
later wait of the same form answers the older one with `timed_out` first. A wait
that names a card and a wait on every card of its agent can run together; a
click goes to the wait that names its card first, and to the any-card wait
otherwise. Closing a card answers a `next_click` that names it with
`panel_unavailable`, and an any-card wait whose agent owns no card left answers
the same. A wait on the card of another agent answers `not_owner`.

A click that no `next_click` waits for is kept for five minutes, at most 32 per
card. A click on a full queue is dropped and the card says that too many clicks
wait; an older click is never dropped for a newer one.

## The wake line

When a click waits and no `next_click` listens, Horizon types one fixed line
into the owner's agent terminal:

```
A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it.
```

The line never carries text from the card, so a card cannot write into a
terminal. It arrives only when the owner's agent panel is idle and holds no
keyboard focus, at most once per streak of clicks and at most every 30 seconds
per agent panel. Do not wait for it when you are already in `next_click`: while
it listens, the click is delivered at once and no line is typed.

An agent that ignores the line never reads the click. `listening` and
`pending_clicks` in `list` and `inspect` say whether a click waits.

## Errors

Every refusal is a `failed` outcome with a stable `code` and a message.

- `wrong_host`: the request belongs to another Horizon host. Send it from the
  agent panel that Horizon launched.
- `caller_unavailable`: the calling agent panel is not in this host, or the
  server started without the identity that Horizon injects. Start the agent
  from Horizon.
- `panel_unavailable`: the panel id is not in the calling workspace. Call
  `list` to see the cards that the caller can reach.
- `not_block_panel`: the panel id is a panel that is not a card.
- `not_owner`: another agent created this card. Read it with `inspect`; only its
  owner changes it.
- `invalid_document`: the document breaks a rule. The message names the
  offending place, such as `components[2].items[0].id` or
  `components[1].rows`. Correct that place and send the document again.
- `document_too_large`: the serialized document is above 32 KiB. Send less
  text, or more than one card.
- `too_many_cards`: the agent owns eight cards in this workspace. Close one.
- `no_cards`: an agent without a card in this workspace asked `next_click`
  without `panel_id`. Call `create` first.
- `invalid_ssh_route`: the host, the user or the port of `open_terminal` breaks a
  rule. The message names the field.
- `invalid_remote_command`: the remote command breaks a rule. The message names
  the field.
- `too_many_terminals`: the agent panel opened eight terminals in this Horizon
  run. Closing a terminal does not free its place; ask the person to open the
  next one.
- `terminal_unavailable`: Horizon could not open the terminal panel.
- `invalid_request`: `open_terminal` named both `ssh` and `machine`, neither, or
  one of `ssh`, `key` and `remote_command` together with `machine`. Pass one
  target and let the record hold the rest.
- `invalid_machine_name`: the machine name breaks the key name rule. The message
  names it.
- `invalid_session_name`: the tmux session name breaks its rule. The message
  names it.
- `too_many_machines`: this Horizon keeps the 64 machines that it may keep. Ask
  the person to forget one in the sidebar.
- `machine_unavailable`: this Horizon has no machine of that name, or the record
  is gone. Call `machines` to list the ones it has.
- `invalid_key_name`: the key name breaks a rule. The message names it.
- `too_many_keys`: this Horizon holds the 64 keys that it may hold. Delete one
  with `delete_machine_key`.
- `key_unavailable`: this Horizon home has no key of that name, the pair is
  incomplete, `ssh-keygen` is missing or failed, or Horizon could not read or
  delete it.
- `request_expired`: Horizon did not apply the request in 10 seconds, or in the
  10 seconds and the wait that a `next_click` asks for. Call `list` before you
  retry, because the change may have completed.
- `host_timeout`: Horizon did not answer within 15 seconds, or within the wait
  of a `next_click` plus 5 seconds. Call `list` before you retry a mutation;
  older Horizon hosts do not support cards.

## Limits of this version

- A card holds buttons, selectable rows and links. It has no text input, no
  image and no table. A link is the only field that Horizon opens, and only a
  press opens it: it carries no click, so an agent cannot wait on it with
  `next_click`.
- The selection of a row is Horizon state, not a click of its own: it reaches
  the agent with the next click, and it starts at the row the document marks
  `selected`.
- A wait without `panel_id` covers every card that the agent owns now, also one
  that it creates while the wait runs.
- A click reaches an agent at most once. There is no replay, and the queue of a
  card is not saved.
- A card is not saved as a file; its document lives in the panel and comes back
  after a restart of Horizon.
- Only the agent that created a card can change it.
- A terminal that you open with `open_terminal` is not a card. It has no
  document, no click and no `next_click`, and this tool cannot change it after
  it is open.
- The key name is saved with the terminal. A key that you delete after you
  opened a terminal leaves that terminal with ssh's own error when it restores.
- A local agent that runs as the same user as Horizon can read the private key
  file from disk, and its answer names the path. The key is not a secret that
  Horizon can keep from the person or from a process of the person. What Horizon
  keeps is the fixed command line of the panel that it opens, and that the
  person's own keys and ssh agent are not offered to the machine.
