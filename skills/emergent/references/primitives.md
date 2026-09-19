# Marketplace Primitive Reference

Install with `emergent marketplace install <name>...` (accepts multiple names;
so does `remove`). Everything lands in `~/.local/share/emergent/primitives/bin/`.

These primitives exist so that most topologies need no custom code at all. Before
writing a primitive with an SDK, check whether a composition of these expresses
the step. Usually it does.

## Contents

- [exec-source](#exec-source-source)
- [exec-handler](#exec-handler-handler)
- [exec-sink](#exec-sink-sink)
- [http-source](#http-source-source)
- [websocket-handler](#websocket-handler-handler)
- [stream-runner](#stream-runner-handler)
- [jev-handler](#jev-handler-handler)
- [sse-sink](#sse-sink-sink)
- [topology-viewer](#topology-viewer-sink)
- [Envelope environment variables](#envelope-environment-variables)
- [Primitive config fields](#primitive-config-fields)

---

## exec-source (Source)

Run any shell command, emit its output as events. Handles polling, one-shot
seeding, and timers.

```toml
[[sources]]
name = "ticker"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "3000"]
publishes = ["exec.output"]
```

**Flags**

| Flag | Meaning |
|---|---|
| `-c, --command <CMD>` | Command to execute (required) |
| `-a, --args <ARGS>` | One string of arguments. Split on whitespace with no quote handling, unless `--shell` is set, in which case `<shell> -c "<command> <args>"` runs |
| `-i, --interval <MS>` | Repeat interval; omit or 0 to run once. Runs never overlap, and a run that outlasts the interval is followed immediately by the next |
| `-s, --shell <SHELL>` | Shell to run through (`sh`, `bash`) |
| `-d, --working-dir <DIR>` | Working directory |
| `--correlate` | Mint one correlation ID at startup, stamp it on every published message |
| `--correlation-id <ID>` | Adopt an existing `cor_<uuid_v7>` instead (env: `EMERGENT_CORRELATION_ID`). Wins over `--correlate`. A malformed ID exits 1 before connecting |

Every flag except `--correlate` also reads an environment variable:
`EXEC_SOURCE_COMMAND`, `_ARGS`, `_INTERVAL`, `_WORKING_DIR`, `_SHELL`. Because
`--correlation-id` reads `EMERGENT_CORRELATION_ID`, a value already present in
the engine's own environment is adopted silently, even without `--correlate`.

Without `--shell`, `--command` is executed directly, so a pipe or a redirect in
it fails with "No such file or directory". Pass `--shell sh` for anything that
is more than a program name.

The command has to exit. `exec-source` collects all output and publishes after
the process ends, so a streaming command (`inotifywait -m`, `tail -f`) never
publishes. Poll on `--interval`, or write a small SDK source for a true stream.

**Publishes:** `exec.output` (stdout), `exec.error` (stderr), `exec.exit`

The three topics come from the `publishes` array in order, so
`publishes = ["invoice.listed", "invoice.list-failed", "invoice.list-done"]`
renames all three. There is no `--publish-as` flag; the config *is* the mapping.
Give each source its own topic names rather than letting several sources share
`exec.output`, which otherwise forces downstream handlers to disambiguate by
inspecting the payload.

**Payloads**, one shape per topic:

| Topic (position in `publishes`) | Published when | Payload |
|---|---|---|
| first, stdout | stdout is non-blank | `{"command", "stdout", "exit_code"}` |
| second, stderr | stderr is non-blank | `{"command", "stderr", "exit_code"}` |
| third, exit | always | `{"command", "exit_code"}` |

`command` is the bare `--command` value without `--args`. `exit_code` is `-1`
when the process was killed by a signal. A command that prints nothing publishes
only the exit event, so subscribe to the third topic when "it ran" matters.

**One execution publishes at most one stdout event.** `stdout` is a single
string holding all output, however many lines it has. Printing ten JSON objects
does not publish ten events. See "Turning a collection into events" in
`patterns.md`, because assuming otherwise is the most common way an otherwise
sound topology fails on its first run.

Set `unwrap_stdout = true` on a downstream handler or sink and the SDK extracts
and parses `.stdout` for you, removing the dedicated jq unwrap step.

`--correlate` is how a run gets an identity. Every `exec-handler` downstream
carries the ID forward, so the whole flow is one query against the event store's
`correlation_id` column, and shell steps can read it from
`EMERGENT_CORRELATION_ID`.

Scope matters: the ID is resolved **once at process startup**, not per message.
A one-shot source therefore gets one ID per run, which is what you want. An
`--interval` source stamps that same ID on every poll for its entire lifetime,
so `correlation_id` groups the poller, not the item. Key per-item joins on
something from the payload instead. See `patterns.md`, "Choosing the join key".

---

## exec-handler (Handler)

Pipe an event payload through any executable's stdin and publish its stdout as a
new event. This is the workhorse of idiomatic topologies.

```toml
[[handlers]]
name = "transform"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "input.topic", "--publish-as", "output.topic", "--", "jq", "-c", ". + {processed: true}"]
subscribes = ["input.topic"]
publishes = ["output.topic"]
```

**Flags**

| Flag | Meaning |
|---|---|
| `-s, --subscribe <TOPIC>` | Message type to subscribe to (required, repeatable) |
| `--publish-as <TOPIC>` | Message type for successful output (default `exec.output`) |
| `-e, --error-as <TOPIC>` | Message type for errors (default `exec.error`) |
| `-t, --timeout <MS>` | Per-execution timeout (default 30000). On expiry the command's whole process group gets SIGTERM |
| `--kill-grace-ms <MS>` | How long a timed-out command may run after SIGTERM before SIGKILL (default 5000) |
| `--max-concurrent <N>` | Commands running at once (default 1). At 1, strict arrival order. Above 1, outputs publish in completion order |
| `--silent-exit-codes <CODES>` | Comma-separated exit codes treated as a silent filter when stderr is blank (default none) |
| `-- <cmd> [args...]` | The command to run |

**The `publishes` array overrides the topic flags, by position.** The engine
passes the config's `publishes` to the primitive, and `publishes[0]` replaces
`--publish-as` while `publishes[1]` replaces `--error-as`. Always order it
`[success, error]`. Written the other way round, the topics swap silently.

**Exit-code semantics, which are the basis of several idioms:**

| Command result | Effect |
|---|---|
| exit 0, stdout non-empty | Publishes stdout as the success message |
| exit 0, stdout empty | Silent. Nothing published. |
| non-zero exit, code listed in `--silent-exit-codes`, stderr blank | Silent filter. Nothing published. |
| any other non-zero exit | Publishes the error message |
| timeout, or the command could not be spawned | Publishes the error message |

The exit-0-empty-stdout case is load-bearing, and it is the only silence you get
by default. `jq -c 'select(...)'` prints nothing and **exits 0** when its
predicate is false, which is what makes routers and filters zero-code. Do not
write `jq -e` in a router: it exits 4 on no output, which publishes an error
event unless you also pass `--silent-exit-codes 4`.

A non-zero exit is never silent by default, so a failure cannot vanish. That
matters for shell idioms: `[ cond ] && cmd` exits 1 when the condition is false,
which is an error event, not a filter. When a false condition is a normal
outcome, declare it: `--silent-exit-codes 1`.

**Error payload.** The inbound payload's fields are spread at the top level and a
reserved `error` object is added: `{"exit_code", "stderr", "command"}`. An
inbound payload that is not a JSON object is carried under `input` instead, and
an inbound key named `error` is overwritten. A timeout gives
`exit_code: null, stderr: "process timed out"`. Because the original fields
survive, a retry handler can republish the item from the error event alone.

**One execution publishes exactly one event.** stdout is parsed as a *single*
JSON value; if that parse fails it is wrapped as `{"output": "<all stdout>"}`.
So a command printing several JSON objects produces one event containing a
blob, not one event per object. `jq -c '.[] | ...'` and
`split("\n") | .[] | fromjson` look like fan-out and are not. To turn a
collection into per-item events, see "Turning a collection into events" in
`patterns.md`.

**Topic collision to avoid:** `--error-as` defaults to `exec.error`, which is
also the topic `exec-source` uses for stderr. Left at the default, two unrelated
failure meanings land on one topic and neither is declared in `publishes`. Give
handler errors a domain name (`--error-as invoice.extract-failed`) and declare
it.

Published messages, success and error alike, inherit the inbound
`correlation_id` and set `causation_id` to the message they came from, so
tracing works without any effort on your part.

A message is pulled only when a slot is free, so a slow command backpressures
the engine rather than queueing unboundedly inside the handler. On SIGTERM,
in-flight executions drain and publish before the handler disconnects.

A long `-t` value is the standard tell for a merged step. If you are reaching
for minutes, ask what events should be flowing during that time.

---

## exec-sink (Sink)

Pipe an event payload through an executable. Nothing is published; this is
fire-and-forget egress. The command's stdout and stderr are not captured: they
pass through to the engine's terminal or journal.

```toml
[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.processed", "--", "jq", "."]
subscribes = ["data.processed"]
```

**Flags:** `-s, --subscribe <TOPIC>` (required, repeatable),
`-t, --timeout <MS>` (30000), `--kill-grace-ms <MS>` (5000),
`--max-concurrent <N>` (1), `--silent-exit-codes <CODES>`, `-- <cmd> [args...]`

A failing command publishes nothing. The sink writes
`exec-sink: <cmd>: exit code N (caused by <message id>)` to its stderr and moves
on, and codes listed in `--silent-exit-codes` are not reported at all. If a
failed delivery must be reactable, use an `exec-handler` instead and route its
error topic.

Common uses that replace dedicated primitives entirely:

```bash
exec-sink -s timer.tick   -- jq .                                   # console
exec-sink -s alert.fired  -- curl -s -X POST -d @- https://hooks…   # webhook
exec-sink -s data.done    -- tee -a /var/log/events.jsonl           # file log
```

stdin carries only the payload. When a sink needs to know which topic fired
(for example when fanning several topics into one forwarder), read
`$EMERGENT_MESSAGE_TYPE`. See "Envelope environment variables" below.

---

## http-source (Source)

Receive HTTP requests as events. Every method is accepted, not only POST.

```toml
[[sources]]
name = "webhook"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "8080"]
publishes = ["http.request"]
```

**Flags:** `-p, --port <PORT>` (8080), `--host <HOST>` (0.0.0.0; bind
`127.0.0.1` unless the caller is remote), `--path <PATH>` (`/`),
`--secret <SECRET>` for signature validation. Each reads an environment
variable: `HTTP_SOURCE_PORT`, `HTTP_SOURCE_HOST`, `HTTP_SOURCE_PATH`,
`HTTP_SOURCE_SECRET`. Prefer the variable for the secret so it stays out of
`emergent.toml`. After primitives 0.11.0 there is also `--trust-forwarded-for`
(off), described below.

`--path` is an exact axum route, not a prefix: `--path /inject` answers
`/inject` and returns `404` for `/inject/extra`. Captures use braces,
`/hook/{id}` or a final `/files/{*rest}`, and the published `path` is the
concrete requested path. After primitives 0.11.0 an invalid value (no leading
`/`, the old `:id` or `*rest` syntax, a wildcard that is not last) prints one
line naming the value and the rule and exits 1. On 0.11.0 and earlier the same
value panics at startup, which the engine reports as exit status 101. A `?` or
`#` in the literal part of the path is refused the same way after 0.11.0,
because the query string is never part of the route (it is published in the
`query` field) and a fragment never reaches the server; on 0.11.0 and earlier
such a route loads, reports `running`, and answers `404` to everything.

With a secret set, a request must carry an `X-Signature` header holding the hex
HMAC-SHA256 of the raw body, with an optional `sha256=` prefix. A missing or
wrong signature gets `401`. A published request gets `202`.

**Publishes:** the first entry of `publishes` (default `http.request`) with
`{"method", "path", "query", "headers", "body", "remote_addr"}`. `body` is the
parsed JSON, or the raw body as a string when it is not JSON.

- On primitives 0.11.0 and earlier `path` is always `"/"`, `remote_addr` is
  always `null` and there is no `query`, so do not route on any of them.
- After 0.11.0 `path` is the requested path without the query string, so
  `select(.path == "/inject")` is a stable route. `query` is the raw, undecoded
  query string, or `null`. `remote_addr` is the IP of the socket peer, with no
  port.

`remote_addr` ignores `X-Forwarded-For` by default, because a direct caller can
write anything in that header. Pass `--trust-forwarded-for` only when the source
sits behind a reverse proxy that overwrites the header; the leftmost hop is then
reported, and a malformed header falls back to the socket peer. On a directly
exposed port the flag lets every caller name itself, so leave it off.

The request body never becomes an event of its own type. It arrives nested under
`.body`, with no `correlation_id` and no `causation_id`, so the first handler
downstream is normally a `jq -c .body` unwrap.

Beyond real webhooks, an `http-source` is the standard way to make a topology
injectable: you can POST any event into it by hand, which is exactly what the
injection test in Gate 2 asks for.

---

## websocket-handler (Handler)

Bidirectional WebSocket bridge. Inert until it receives a connect message, which
means the connection itself is event-driven rather than configured.

```toml
[[handlers]]
name = "ws"
path = "~/.local/share/emergent/primitives/bin/websocket-handler"
args = ["--prefix", "ws"]
subscribes = ["ws.connect", "ws.send", "ws.disconnect"]
publishes = ["ws.connected", "ws.frame", "ws.closed", "ws.disconnected", "ws.error"]
```

**Flags:** `--prefix <PREFIX>` (default `ws`)

**Subscribes:** `{prefix}.connect` (payload `{url}`), `{prefix}.send`,
`{prefix}.disconnect`

**Publishes:**

| Topic | Payload |
|---|---|
| `{prefix}.connected` | `{url}` |
| `{prefix}.frame` | `{data}`. A text frame is JSON-parsed, falling back to the raw string. A binary frame is base64 after primitives 0.11.0; on 0.11.0 and earlier it arrived as the literal string `[object Blob]` |
| `{prefix}.closed` | The handler was asked to end the connection. Payload below |
| `{prefix}.disconnected` | The connection ended and nobody asked. After primitives 0.11.0 only. Payload below |
| `{prefix}.error` | `{url, error}`. Diagnostic, never terminal |

After primitives 0.11.0 every connection publishes exactly one terminal event,
decided by intent rather than by close code:

| Event | `cause` | Reconnect? |
|---|---|---|
| `{prefix}.closed` | `disconnect` (a disconnect message), `reconnect` (a newer connect replaced it), `shutdown` (the handler is stopping) | No |
| `{prefix}.disconnected` | `remote_close` (the peer sent a close frame), `connection_lost` (dropped with no close frame), `connect_failed` (never opened) | Yes, if you want it back |

Both carry `{url, code, reason, was_clean, cause, opened, error}`. An ending
with no close frame is always `code` 1006. `opened` is false when the
connection failed before it was established. `error` is the first socket error
seen, or null.

Reconnection is a subscriber, not a flag: a handler on `{prefix}.disconnected`
republishes `{prefix}.connect`. A dropped connection publishes `error` and then
`disconnected`, so drive the reconnect from `disconnected` alone or the flow
reconnects twice. Put a delay or a retry count in that handler, because a
refused connect publishes `disconnected` at once and the loop is otherwise
tight. A half-open connection (the network is gone with no FIN or RST) is only
noticed when the OS gives up on it.

On 0.11.0 and earlier there is no `disconnected`: `closed` carried only
`{url, code, reason}` and covered remote closes too, a handler shutdown
published nothing, and a connect while connected published `closed` with the
new URL and orphaned the new socket. Do not build reconnection on those
versions.

Topic names are resolved by suffix (`.connect`, `.send`, `.frame`, and so on)
from the config's `subscribes` and `publishes`, falling back to `--prefix`, so
the config wins over the flag. A topology that declares `slack.closed` and no
`disconnected` type gets `slack.disconnected`, which nothing routes until you
declare and subscribe to it. One connection at a time: a new connect closes the
old one. A send with no open socket publishes an error event, and a non-string
send payload is JSON-stringified. Every event of a connection carries the
connect message's id as `causation_id` but does **not** propagate
`correlation_id`, so carry a key in the payload if you need to trace across the
bridge.

---

## stream-runner (Handler)

Emit a JSON collection one item at a time, waiting for a downstream ack before
advancing. This is how a batch payload becomes a paced stream without a sleep
anywhere.

```toml
[[handlers]]
name = "streamer"
path = "~/.local/share/emergent/primitives/bin/stream-runner"
args = ["--load-topic", "batch.load", "--publish-as", "work.item",
        "--ack-topic", "work.done", "--end-topic", "batch.complete",
        "--items-key", "items"]
subscribes = ["batch.load", "work.done"]
publishes = ["work.item", "batch.complete"]
```

As with `exec-handler`, the `publishes` array overrides the topic flags by
position: keep it `[item, end]`, or after primitives 0.11.0
`[item, end, rejected, timed out]`.

**Flags**

| Flag | Default | Meaning |
|---|---|---|
| `--load-topic` | `stream.load` | Event carrying the collection |
| `--publish-as` | `stream.item` | Topic for each item |
| `--ack-topic` | `stream.ack` | Topic that advances the stream |
| `--end-topic` | `stream.end` | Published when exhausted, payload `{"count": N}` |
| `--items-key` | `items` | Object key holding the array (ignored for a bare array) |
| `--rejected-topic` | `stream.rejected` | After 0.11.0. Published when a load is dropped |
| `--timed-out-topic` | `stream.item-timed-out` | After 0.11.0. Published when an ack does not arrive in time |
| `--ack-key` | unset | After 0.11.0. Field that must agree between an item and its ack |
| `--ack-timeout-ms` | unset | After 0.11.0. How long an item may wait for its ack |
| `--on-timeout` | `skip` | After 0.11.0. `skip` the item or `end` the run; needs `--ack-timeout-ms` |

This is **the splitter**: the one marketplace primitive that turns a collection
into per-item events. It is also the pacing mechanism, because those are the
same behavior: one item in flight until the ack fires.

What to design around depends on the version, because through primitives
0.11.0 every one of these failures was silent.

- **One collection at a time.** A `load` arriving while a stream is still
  running is dropped, not queued. With an interval source, gate the poll on the
  end topic or size the interval so a batch drains first. On 0.11.0 and earlier
  the drop is only logged at `RUST_LOG=warn` or lower, so by default it is
  silent. After 0.11.0 it is published on the rejected topic with
  `reason: "busy"` and the dropped load intact under `.payload`. Do not wire
  that straight back to the load topic: the stream is still busy and the
  rejection would loop. Park it and replay it on the end event.
- **A malformed load.** A payload missing `--items-key`, or whose key is not an
  array, publishes nothing on 0.11.0 and earlier. After 0.11.0 it is published
  on the rejected topic with `reason: "bad_shape"`, and a raw `exec-source`
  envelope gets a `hint` to unwrap it first. An empty collection publishes the
  end event with `count: 0` immediately on every version.
- **Acks and items.** Without `--ack-key`, any message on the ack topic advances
  the stream, so a duplicate or late ack puts two items in flight; make sure
  exactly one ack fires per item across all paths. After 0.11.0,
  `--ack-key <field>` advances only when `ack[field]` equals the same field on
  the item in flight. The field must be present and non-null on both sides, so
  a collection of bare strings cannot use it. A mismatched ack is logged at WARN
  and not published.
- **A missing ack.** On 0.11.0 and earlier it stalls the stream until restart,
  and every later load is dropped too, so the ack topic must be published on the
  failure path as well as the success path. After 0.11.0, `--ack-timeout-ms`
  bounds the wait: the item is published on the timed-out topic under `.item`,
  and `--on-timeout` either skips it or ends the run with `incomplete: true`.
  Pair it with `--ack-key`, or an ack that arrives after its item timed out
  advances the stream a second time.

After 0.11.0 the end event is
`{"count", "total", "timed_out", "incomplete"}`; `count` and `total` differ only
when a run ended early. Route the rejected and timed-out topics with `jq`
selectors on `.reason`, the same way `jev-handler` error kinds are routed.

The ack topic is normally the downstream stage's own output, so consumption rate
sets the pace. The load message's `correlation_id` is replayed onto every item
and onto the end event, and each carries `causation_id` of the load message, because acks are separate messages and cannot be trusted
to carry it.

Use this when the consumer is the bottleneck (rate-limited APIs, LLM calls). Use
plain fan-out when it is not.

---

## jev-handler (Handler)

Ask [TypeSafe System One](https://docs.typesafe.ai) (the Jev model) a fixed set
of typed questions about every payload and publish the answers with calibrated
confidence. One API call per message, typically well under a second. This is the
judge to reach for when the judgment is a yes/no, a pick from a known set, or a
position on a rubric, and volume or latency rules out an LLM call per event.

```toml
[[handlers]]
name = "judge-message"
path = "~/.local/share/emergent/primitives/bin/jev-handler"
args = ["-s", "mail.fetched", "--questions", "./questions.toml",
        "--state-pointer", "/mail",
        "--publish-as", "mail.judged", "-e", "mail.judge-failed",
        "--max-concurrent", "4"]
subscribes = ["mail.fetched"]
publishes = ["mail.judged", "mail.judge-failed"]
```

**Flags**

| Flag | Default | Meaning |
|---|---|---|
| `-s, --subscribe` | required | Message types to judge (repeatable) |
| `--questions` | required | `.toml` or `.json` questions file, read once at startup |
| `--publish-as` | `jev.answered` | Topic for an answered message |
| `-e, --error-as` | `jev.error` | Topic for a failed message |
| `--state-pointer` | whole payload | JSON pointer to the slice of the payload the model reads |
| `--model` | `jev-latest` | Alias or pinned id. Pin it in production so thresholds do not move under you |
| `--max-concurrent` | `1` | Messages in flight. A message in retry backoff holds its slot |
| `-t, --timeout` | `120000` | Whole-message budget in ms, retries included |
| `--request-timeout` | `30000` | Single HTTP attempt in ms |
| `--max-attempts` | `4` | Attempts per message. `429`, `5xx`, and transport failures retry; any other `4xx` is fatal on the first attempt |
| `--retry-base-ms` | `500` | First backoff step, doubled per attempt, with up to 25% jitter subtracted |
| `--retry-max-delay-ms` | `30000` | Ceiling on a computed backoff |
| `--max-retry-after-ms` | `60000` | Ceiling on a server-supplied `Retry-After` |
| `--endpoint` | `https://api.typesafe.ai/v1/systemone` | Evaluation endpoint |

The API key comes from the `TYPESAFE_API_KEY` environment variable and nowhere
else. Let the primitive inherit it from the engine's environment; do not write it
into an `env` table in `emergent.toml`.

**The questions file.** The table name is the question id and becomes the answer
key, so a router can write `.answers.<id>`:

```toml
[questions.unwanted]          # noul: yes/no. criteria optional, keys "true"/"false" only
type = "noul"
instructions = "Is this message unsolicited?"

[questions.kind]              # choice: at least 2 options (the API caps it at 255). criteria required
type = "choice"
instructions = "What kind of message is this?"
criteria = { phish = "Credential theft under a false identity.", vendor_notice = "An operational notice from a service already in use.", other = "None of the above." }

[questions.pressure]          # score: ordered rubric, 2+ levels. criteria required
type = "score"
instructions = "How much time pressure does the message apply?"
criteria = ["None.", "A soft deadline.", "Act now or lose access."]
```

Ids must match `[A-Za-z_][A-Za-z0-9_]*`, unknown keys are rejected, and score
levels must be distinct. The model never sees the ids; all meaning lives in
`instructions` and `criteria`. The file is validated before the engine connection, so a typo stops
the process at startup rather than producing a handler that looks healthy.
Always give a choice an explicit none-of-the-above option, or the model is
forced to pick a wrong one with confidence.

**Payloads**

```json
{"input": {"...": "the inbound payload, nested"},
 "answers": {
   "unwanted": {"type": "noul", "noul": 0.93},
   "kind": {"type": "choice", "choice": "phish", "confidence": 0.99,
            "probabilities": {"phish": 0.99, "vendor_notice": 0.01, "other": 0.0}},
   "pressure": {"type": "score", "score": 2.0, "confidence": 1.0,
                "legend": {"0": "None."}, "probabilities": {"2": 1.0}}},
 "usage": {"input_tokens": 391, "output_tokens": 34},
 "model": "jev-1.13.0", "request_id": "req_..."}
```

A noul carries no `confidence`: the probability is the confidence. Do not reuse a
noul threshold on a choice; they are calibrated differently.

As with `exec-handler`, the `publishes` array overrides the topic flags by
position (`[answered, error]`), and both message types inherit the inbound
`correlation_id` with `causation_id` set to the inbound message. `input` is the
whole inbound payload even when `--state-pointer` narrows what the model reads,
and `usage` may be `null`.

A failure spreads the inbound fields (a non-object payload goes under `input`)
and adds a reserved `error` object:
`{kind, status, attempts, message, endpoint, model, request_id, body, detail}`.
Fields that do not apply are `null`, and `body` is truncated to 2 KiB.
`error.kind` is one of `invalid_request`, `state_not_found` (the item or the
questions are at fault, quarantine); `auth`, `billing`, `bad_response`,
`answer_contract` (the item is fine and only a human can unblock it, hold it);
`rate_limited`, `server_error`, `transport`, `timeout` (transient, requeue).

**It judges; it never routes.** It publishes one verdict type, exactly like the
`exec-handler` judge in SKILL.md's non-deterministic routing section. Confidence
bands become message types through exclusive, exhaustive routers on the verdict:

```toml
[[handlers]]
name = "route-settled"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "mail.judged", "--publish-as", "mail.settled", "--",
        "jq", "-c", "select(.answers.kind.confidence >= 0.9)"]
subscribes = ["mail.judged"]
publishes = ["mail.settled"]

[[handlers]]
name = "route-uncertain"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "mail.judged", "--publish-as", "mail.uncertain", "--",
        "jq", "-c", "select(.answers.kind.confidence < 0.9)"]
subscribes = ["mail.judged"]
publishes = ["mail.uncertain"]
```

Combining questions is a router predicate too
(`.answers.unwanted.noul >= 0.5 or .answers.deceptive.noul >= 0.5`), never a
reason to fork the primitive. Route the error topic the same way, with the last
router matching by negation so a kind added later cannot vanish.

Design around the model's limits:

- **It reads literally and does no arithmetic.** It will not count, compare
  dates, or sum. Compute those upstream and put the result in the state.
- **Irrelevant state costs accuracy and money.** Use `--state-pointer` and an
  upstream `jq` projection to send only what the questions need.
- **It is not robust to prompt injection.** Treat the state as untrusted input
  and never let a verdict alone authorize a destructive or trust-raising action.
- **Criteria grounded in real examples beat generic labels** by a wide margin.
  Describe each option by what actually belongs in it.

---

## sse-sink (Sink)

Push events to browsers over Server-Sent Events.

```toml
[[sinks]]
name = "dashboard"
path = "~/.local/share/emergent/primitives/bin/sse-sink"
args = ["--port", "8081"]
subscribes = ["monitor.metric"]
```

**Flags:** `-p, --port <PORT>` (8080), `--host <HOST>` (`127.0.0.1`).
**Endpoints:** `GET /events`, and `GET /health` returning `{ok, clients}`.

After primitives 0.11.0 the sink listens on loopback only. A browser on another
machine needs `--host 0.0.0.0` (or a reverse proxy), and the stream has no
authentication, so expose it deliberately. An unknown flag or a malformed
`--port` is an error, and a busy or invalid address prints one line and exits 1.
On 0.11.0 and earlier it bound every interface, had no `--host`, and ignored
flags it did not know.

Each event is sent as an unnamed `data:` line holding
`{id, type, source, timestamp, payload}`, with CORS open to `*`. With no
`subscribes` in the config it subscribes to `*`.

---

## topology-viewer (Sink)

Live D3 force-directed view of the running pipeline.

```toml
[[sinks]]
name = "topology"
path = "~/.local/share/emergent/primitives/bin/topology-viewer"
args = ["--port", "8009"]
subscribes = ["system.started.*", "system.stopped.*", "system.error.*"]
```

**Flags:** `-p, --port <PORT>` (8080), `--host <HOST>` (`127.0.0.1`). Open `/`
in a browser. The loopback default, `--host 0.0.0.0` for remote browsers, and
the strict flag parsing are the same as for sse-sink above, with the same
version boundary.

| Route | Returns |
|---|---|
| `GET /api/topology` | `{nodes, edges, health}` as the viewer currently holds it |
| `POST /api/refresh` | After primitives 0.11.0. One re-read of the engine topology, then the same body. `200` when the engine answered, `502` with the held state and the reason when it did not, `405` for other methods |
| `GET /events` | The SSE stream the page uses: `topology:full`, `node:updated`, `edges:updated`, `health:updated` |

After primitives 0.11.0 the viewer reads the engine's own
`GET /api/topology` (on `EMERGENT_API_PORT`, which the engine sets) when it
starts and every 5 seconds, so the graph is complete whatever the viewer missed
while starting. Its wildcard subscriptions carry the live updates in between;
they need an engine after 0.10.10 and a viewer built on an SDK that sends
wildcards, and without them the graph is still right within 5 seconds. `health`
says whether the graph can be trusted: `ok`, `pending` (no answer yet), `empty`
(the engine reported nothing but itself) or `degraded` (the engine could not be
read; `detail` says why). The page shows a banner for anything but `ok`, so
check `health` before believing a sparse graph.

**On 0.11.0 and earlier (Govcraft/emergent-primitives#5)** the viewer asks for
three `system.*.*` wildcards, which no engine release delivers, so the graph
shows the engine node and nothing else, and the refresh button first calls a
hard-coded `localhost:8892` that nothing serves. On those versions read the
graph from the engine instead:

```bash
curl -s 127.0.0.1:<api_port>/api/topology | jq '.primitives[] | {name, kind, publishes, subscribes}'
```

On engine 0.10.10 and earlier, ignore `state` and `pid` in that response:
managed primitives always report `"configured"` and `null`, even while running
(Govcraft/emergent#40). After 0.10.10 both are live, so adding `state` and `pid`
to that filter tells you what is actually running.

Worth checking during design. Seeing the graph makes an under-decomposed topology
obvious at a glance, because it renders as a short chain instead of a web.

---

## Envelope environment variables

Exec primitives pipe only the message *payload* to stdin, so the envelope is
invisible to a jq filter. It arrives in the environment instead:

| Variable | Source |
|---|---|
| `EMERGENT_MESSAGE_ID` | `message.id` |
| `EMERGENT_MESSAGE_TYPE` | `message.message_type` |
| `EMERGENT_MESSAGE_SOURCE` | `message.source` |
| `EMERGENT_CORRELATION_ID` | `message.correlation_id` |
| `EMERGENT_CAUSATION_ID` | `message.causation_id` |

`exec-handler` and `exec-sink` set all five. `exec-source` sets only
`EMERGENT_CORRELATION_ID`, having no inbound message. A field the message does
not carry is *removed* from the child's environment rather than left alone, so
an ambient value cannot leak in and mislabel output.

The engine also sets `EMERGENT_SOCKET`, `EMERGENT_NAME`, `EMERGENT_API_PORT`,
`EMERGENT_PUBLISHES`, and `EMERGENT_SUBSCRIBES` for every primitive. The last
two are how the config's arrays reach the primitive, which is why `publishes`
order overrides the topic flags.

---

## Primitive config fields

| Field | Sources | Handlers | Sinks | Notes |
|---|---|---|---|---|
| `name` | required | required | required | Unique across sources, handlers, and sinks combined |
| `path` | required | required | required | Leading `~/` expands; a bare name with no `/` resolves via PATH. A missing path on an enabled primitive is a load error |
| `args` | optional | optional | optional | Argument array |
| `enabled` | optional | optional | optional | Default true |
| `publishes` | optional | optional | n/a | Declared output types. Drives the topology view **and** overrides the primitive's topic flags by position |
| `subscribes` | n/a | optional | optional | Types to receive. Defaults to `[]`, which parses but receives nothing, so set it |
| `env` | optional | optional | optional | Environment map. Do not put secrets here; primitives inherit the engine's environment |
| `unwrap_stdout` | n/a | optional | optional | Auto-extract and parse `.stdout` from the exec envelope |

Language paths:

```toml
# Rust or Go (compiled binary)
path = "./target/release/my_handler"

# Python (via uv)
path = "uv"
args = ["run", "--with", "emergent-client", "h.py"]

# TypeScript (via Deno)
path = "deno"
args = ["run", "--allow-env", "--allow-net=unix", "h.ts"]
```

### Quoting long commands

A shell one-liner rarely fits on one line, and single-line TOML strings cannot
span lines. Use a multi-line **literal** string (`'''...'''`) with a trailing
backslash for continuation. TOML leaves a literal string untouched, so the shell
receives exactly what you typed, backslash-newline included:

```toml
args = ["-s", "issue.found", "--publish-as", "issue.scored", "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); gh issue view "$(jq -r .number <<< "$p")" --json body \
     | jq -c --argjson orig "$p" '{number: $orig.number, body: .body}' ''']
```

Open every body that contains a pipe with `set -o pipefail`. A pipe reports its
last command's status, so without it a failed `gh` leaves `jq` to exit 0 on empty
input and the handler publishes nothing, not even an error.

Do not use a multi-line basic string (`"""..."""`) for a shell body. TOML
processes escapes in it first: `\"` becomes a bare `"` and closes the shell's
quote, `\n` becomes a real newline, and `\(` is a parse error. Each of those
breaks a prompt or a jq program in a way that still loads and still runs. The
only thing a literal string cannot hold is `'''` itself. End the body with a
space before the closing `'''` when its last character is a single quote.

Leave `[engine] wire_format` unset. The key parses (`"messagepack"` or
`"json"`) but selects nothing: IPC is always MessagePack. On engine 0.10.10 and
earlier it was silently inert; after 0.10.10 setting it earns a warning at
startup. For human-readable inspection, read the event store's JSON logs.
