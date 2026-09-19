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
| `-a, --args <ARGS>` | Command arguments |
| `-i, --interval <MS>` | Repeat interval; omit or 0 to run once |
| `-s, --shell <SHELL>` | Shell to run through (`sh`, `bash`) |
| `-w, --working-dir <DIR>` | Working directory |
| `--correlate` | Mint one correlation ID at startup, stamp it on every published message |
| `--correlation-id <ID>` | Adopt an existing `cor_<uuid_v7>` instead (env: `EMERGENT_CORRELATION_ID`) |

**Publishes:** `exec.output` (stdout), `exec.error` (stderr), `exec.exit`

The three topics come from the `publishes` array in order, so
`publishes = ["invoice.listed", "invoice.list-failed", "invoice.list-done"]`
renames all three. There is no `--publish-as` flag; the config *is* the mapping.
Give each source its own topic names rather than letting several sources share
`exec.output`, which otherwise forces downstream handlers to disambiguate by
inspecting the payload.

**Payload:** `{"command": "...", "stdout": "...", "exit_code": 0}`

**One execution publishes exactly one event.** `stdout` is a single string
holding all output, however many lines it has. Printing ten JSON objects does
not publish ten events. See "Turning a collection into events" in
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
| `-s, --subscribe <TOPIC>` | Message type to subscribe to (repeatable) |
| `--publish-as <TOPIC>` | Message type for successful output (default `exec.output`) |
| `-e, --error-as <TOPIC>` | Message type for errors (default `exec.error`) |
| `-t, --timeout <MS>` | Per-execution timeout (default 30000) |
| `-- <cmd> [args...]` | The command to run |

**Exit-code semantics, which are the basis of several idioms:**

| Command result | Effect |
|---|---|
| exit 0, stdout non-empty | Publishes stdout as the success message |
| exit 0, stdout empty | Silent. Nothing published. |
| non-zero exit, stderr empty | Silent filter. Nothing published. |
| non-zero exit, stderr non-empty | Publishes the error message |

The two silent cases are load-bearing. `jq select()` exits non-zero when its
predicate is false, which makes filtering zero-code. The empty-stdout case is
what makes the accumulate-until-complete join work.

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

Published messages inherit the inbound `correlation_id` and set `causation_id`
to the message they came from, so tracing works without any effort on your part.

A long `-t` value is the standard tell for a merged step. If you are reaching
for minutes, ask what events should be flowing during that time.

---

## exec-sink (Sink)

Pipe an event payload through an executable. Output is discarded; this is
fire-and-forget egress.

```toml
[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.processed", "--", "jq", "."]
subscribes = ["data.processed"]
```

**Flags:** `-s, --subscribe <TOPIC>` (repeatable), `-t, --timeout <MS>`,
`-- <cmd> [args...]`

Common uses that replace dedicated primitives entirely:

```bash
exec-sink -s timer.tick   -- jq .                                   # console
exec-sink -s alert.fired  -- curl -s -X POST -d @- https://hooks…   # webhook
exec-sink -s data.done    -- tee -a /var/log/events.jsonl           # file log
```

Note that the child receives only the payload, with no topic metadata. When a
sink needs to know which topic fired (for example when fanning several topics
into one forwarder), pass the topic as a literal argument and use one block per
topic.

---

## http-source (Source)

Receive HTTP POSTs as events.

```toml
[[sources]]
name = "webhook"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "8080"]
publishes = ["http.request"]
```

**Flags:** `-p, --port <PORT>` (8080), `-H, --host <HOST>` (0.0.0.0),
`--path <PATH>` (`/`), `-s, --secret <SECRET>` for HMAC-SHA256 signature
validation (env `HTTP_WEBHOOK_SECRET`)

**Publishes:** `http.request` with
`{"method", "path", "headers", "body", "remote_addr"}`

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
subscribes = ["ws.connect", "ws.send"]
publishes = ["ws.connected", "ws.frame", "ws.closed", "ws.error"]
```

**Flags:** `--prefix <PREFIX>` (default `ws`)

**Subscribes:** `{prefix}.connect` (payload `{url}`), `{prefix}.send`,
`{prefix}.disconnect`
**Publishes:** `{prefix}.connected`, `{prefix}.frame` (payload `{data}`),
`{prefix}.closed`, `{prefix}.error`

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

**Flags**

| Flag | Default | Meaning |
|---|---|---|
| `--load-topic` | `stream.load` | Event carrying the collection |
| `--publish-as` | `stream.item` | Topic for each item |
| `--ack-topic` | `stream.ack` | Topic that advances the stream |
| `--end-topic` | `stream.end` | Published when exhausted, payload `{"count": N}` |
| `--items-key` | `items` | Object key holding the array (ignored for a bare array) |

This is **the splitter**: the one marketplace primitive that turns a collection
into per-item events. It is also the pacing mechanism, because those are the
same behavior: one item in flight until the ack fires.

Two constraints to design around:

- **One collection at a time.** A `load` arriving while a stream is still
  running is logged and dropped, not queued. With an interval source, gate the
  poll on the end topic or size the interval so a batch drains first.
- **A missing ack stalls the stream permanently.** Whatever you name as the ack
  topic must be published on the failure path as well as the success path.

The ack topic is normally the downstream stage's own output, so consumption rate
sets the pace. The load message's `correlation_id` is replayed onto every item
and onto the end event, because acks are separate messages and cannot be trusted
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
| `--max-attempts` | `4` | Attempts per message. Only `429` and `5xx` retry |

The API key comes from the `TYPESAFE_API_KEY` environment variable and nowhere
else. Let the primitive inherit it from the engine's environment; do not write it
into an `env` table in `emergent.toml`.

**The questions file.** The table name is the question id and becomes the answer
key, so a router can write `.answers.<id>`:

```toml
[questions.unwanted]          # noul: yes/no. criteria optional, keys "true"/"false" only
type = "noul"
instructions = "Is this message unsolicited?"

[questions.kind]              # choice: one of 2..=255 options. criteria required
type = "choice"
instructions = "What kind of message is this?"
criteria = { phish = "Credential theft under a false identity.", vendor_notice = "An operational notice from a service already in use.", other = "None of the above." }

[questions.pressure]          # score: ordered rubric, 2+ levels. criteria required
type = "score"
instructions = "How much time pressure does the message apply?"
criteria = ["None.", "A soft deadline.", "Act now or lose access."]
```

The model never sees the ids; all meaning lives in `instructions` and
`criteria`. The file is validated before the engine connection, so a typo stops
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

A failure spreads the inbound fields and adds a reserved `error` object:
`{kind, status, attempts, message, endpoint, model, request_id, body, detail}`.
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

**Flags:** `--port <PORT>` (8080). **Endpoints:** `GET /events`, `GET /health`

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

Worth adding during design. Seeing the graph makes an under-decomposed topology
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

The engine also sets `EMERGENT_SOCKET`, `EMERGENT_NAME`, `EMERGENT_PUBLISHES`,
and `EMERGENT_SUBSCRIBES` for every primitive.

---

## Primitive config fields

| Field | Sources | Handlers | Sinks | Notes |
|---|---|---|---|---|
| `name` | required | required | required | Unique identifier |
| `path` | required | required | required | Supports `~`, bare commands via PATH |
| `args` | optional | optional | optional | Argument array |
| `enabled` | optional | optional | optional | Default true |
| `publishes` | optional | optional | n/a | Declared output types (drives topology view) |
| `subscribes` | n/a | required | required | Types to receive |
| `env` | optional | optional | optional | Environment map |
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

A shell one-liner rarely fits on one line, and TOML basic strings (`"..."`)
cannot span lines. Use a multi-line basic string (`"""..."""`) with a trailing
backslash for continuation. Inner double quotes then need no escaping, which is
worth it on its own:

```toml
args = ["-s", "issue.found", "--publish-as", "issue.scored", "--", "bash", "-c",
  """p=$(cat); n=$(jq -r .number <<< "$p"); \
     gh issue view "$n" --json body | jq -c '{number: '"$n"', body: .body}'"""]
```

Writing `"... \` with a single-quote basic string is a parse error, and it is
the most common way a generated config fails to load.

There is no `wire_format` option to set. IPC is always MessagePack. For
human-readable inspection, read the event store's JSON logs.
