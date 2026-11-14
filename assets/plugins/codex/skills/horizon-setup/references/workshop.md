# The whole workshop in one run

The person wants the whole workshop: the server, a browser that stays signed in,
and machines for the repositories that they work on. This goal is one run of the
three parts, in this order, with one card that changes. It adds no second way to
do a part: each part is the flow of its own goal in this skill.

### 1. The first card: the parts and their state on this computer

Read the state of each part with the commands that exist, and draw one card:

- The server: `{"operation": "server_status"}` answers the entry of this computer
  and the state of its link, `off` when this computer reaches no server.
- The browser: the browser host entry of the settings and the profile that it
  names. The entry serves the browser panels that a create makes after the
  person's press, and `horizon-browser` holds its contract. When the settings
  hold no entry, the part is `Not set up`.
- The machines: `{"operation": "machines"}` answers the machine list of this
  Horizon, and `key_missing` on a record says that this computer holds no key of
  that machine. A repository of the person counts as ready for machines when its
  `AGENTS.md` holds the section `## Horizon machine`, which
  `references/repository-machine.md` of `horizon-machines` reads.

Each part is one row, with the state in its badge: `Ready` with tone `ok`,
`Not set up` with tone `neutral`, and `Needs you` with tone `warn` when the part
cannot go on without the person, for example a link that is `away`.

```json
{"components": [
  {"type": "heading", "text": "Set up my workshop",
   "subtitle": "Three parts, in this order: the server, the browser, the machines.",
   "badge": {"text": "Step 1"}},
  {"type": "list", "selection": "single", "rows": [
    {"id": "part.server", "title": "The Horizon server",
     "subtitle": "Machines, keys and settings follow you to every computer.",
     "badge": {"text": "Ready", "tone": "ok"}},
    {"id": "part.browser", "title": "A browser that stays signed in",
     "subtitle": "It runs on a machine that stays on.",
     "badge": {"text": "Not set up", "tone": "neutral"}},
    {"id": "part.machines", "title": "Machines for your repositories",
     "subtitle": "One machine that is ready for the repository that you name.",
     "badge": {"text": "Not set up", "tone": "neutral"}}]},
  {"type": "caption", "text": "Nothing is rented and no setting changes before your press."},
  {"type": "buttons", "items": [
    {"id": "all_ready", "label": "Set up the parts that are not ready", "tone": "primary"},
    {"id": "part_server", "label": "The server only"},
    {"id": "part_browser", "label": "The browser only"},
    {"id": "part_machines", "label": "The machines only"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

The person chooses the parts with the press: `all_ready` runs every part whose
row is not `Ready`, in the order of the parts, and a `part_*` button runs that one
part alone. A part whose row is `Ready` is said as `Already set up` in the card of
the next part, and its flow is not run again.

### 2. Between two parts

One card says what is done and what comes next, and it asks for nothing that the
part before it did not ask:

```json
{"components": [
  {"type": "heading", "text": "The workshop, part 2 of 3",
   "subtitle": "The browser", "badge": {"text": "Step 2"}},
  {"type": "steps", "steps": [
    {"label": "The server", "state": "done", "at": "done"},
    {"label": "The browser", "state": "now", "at": "now"},
    {"label": "The machines"}]},
  {"type": "kv", "rows": [
    {"label": "Server", "value": "helsinki-1, linked", "mono": true},
    {"label": "Next", "value": "The browser on helsinki-1"}]},
  {"type": "text", "text": "Everything that you answered for the server stays. The next part asks its own questions."},
  {"type": "buttons", "items": [
    {"id": "continue", "label": "Continue with the browser", "tone": "primary"},
    {"id": "stop", "label": "Stop here", "tone": "danger"}]}
]}
```

### 3. Money is asked once for each part that rents

Two parts can rent: the browser, when its place is a new machine, and the
machines of the repositories. A part that rents states its own price on its own
card, with the price of each hour that the provider answers in this run, as the
cards of Goal `rent` and of `references/repository-machine.md` do. One price for
two machines is the price of both together. A part that rents nothing has no
price row and asks nothing about money.

### 4. The end card: what the workshop holds now

When the chosen parts are done, or when the person stops, the card says what the
workshop holds, what it costs for each hour while it is on, and the three things
that a person does next:

```json
{"components": [
  {"type": "heading", "text": "Your workshop is ready",
   "subtitle": "Two of three parts are set up.", "badge": {"text": "Done", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Server", "value": "helsinki-1, linked", "mono": true},
    {"label": "Browser", "value": "helsinki-1, Chromium, profile work", "mono": true},
    {"label": "Machines", "value": "build-1 for example-repo, ready"},
    {"label": "Cost", "value": "one machine, 0.0107 EUR / hour while it is on", "mono": true}]},
  {"type": "text", "text": "What you do next: join a second computer from Settings, Server; start a plan from a task that you describe or from an issue; delete a machine from the Machines section of the sidebar when its work is done."},
  {"type": "buttons", "items": [
    {"id": "close", "label": "Close"}]}
]}
```

- The `Server` row names the machine or the computer and the state of its link.
- The `Browser` row names the machine, the engine and the profile.
- One row for each machine of the repositories, with its repository and its
  state.
- The `Cost` row is the price for each hour of every machine that the run rented
  and that still runs, read in this run. A workshop that rents nothing says
  `nothing is rented` in that row.

### 5. When the goal stops

A part that fails keeps its own failure card, as `When a step fails` says, and the
workshop card says which part it was and what the parts after it need. The parts
that are done stay done. `Stop here` ends the run and draws the end card with what
the workshop holds at that moment.

