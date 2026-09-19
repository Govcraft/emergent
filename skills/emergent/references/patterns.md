# Topology Patterns

The mechanical vocabulary for expressing behavior as structure. Each pattern
replaces something you would otherwise write inside a primitive, which is why
knowing them makes decomposition cheaper than merging rather than more
expensive.

## Contents

- [Fan-out: concurrency](#fan-out-concurrency)
- [Fan-in: convergence](#fan-in-convergence)
- [The join: waiting for N](#the-join-waiting-for-n)
- [Predicate routing: branching](#predicate-routing-branching)
- [The silent filter](#the-silent-filter)
- [Non-deterministic routing](#non-deterministic-routing-the-model-picks-the-next-event)
- [Feedback loops](#feedback-loops)
- [Depth guards: terminating a loop](#depth-guards-terminating-a-loop)
- [Failure as data](#failure-as-data)
- [Retry with backoff](#retry-with-backoff)
- [Paced streaming](#paced-streaming)
- [System events as triggers](#system-events-as-triggers)
- [Self-seeding](#self-seeding)
- [Stateful accumulators](#stateful-accumulators)
- [Making a topology injectable](#making-a-topology-injectable)

---

## Turning a collection into events

Read this before designing anything that processes "each" of something, because
the obvious approach silently does not work.

**Every exec primitive publishes exactly one event per execution.** `exec-source`
puts all stdout into one `stdout` string. `exec-handler` parses stdout as a
single JSON value and, when that fails, wraps the whole thing as
`{"output": "..."}`. So this, which appears in a lot of plausible-looking
configs, is wrong:

```toml
# WRONG. Emits N JSON objects on stdout, which fail to parse as one value and
# collapse into ONE event carrying a stringified blob. Downstream jq then reads
# null from every field.
args = ["-s", "exec.output", "--publish-as", "invoice.detected", "--",
        "jq", "-c", ".stdout | split(\"\\n\") | .[] | fromjson"]
```

Nothing in the engine splits output into messages. If you want N events, some
primitive has to publish N times. There are three ways.

First, separate two ideas that are easy to conflate:

- **Splitting** turns one collection into N events of the same type. This
  section is about that.
- **Fan-out** delivers one event to N subscribers doing different work. That is
  the concurrency mechanism, and it applies to each item once it exists.

They compose rather than compete. Split the batch, then let each item fan out.

### 1. stream-runner (the splitter)

This is the marketplace primitive built for the job, and the first thing to
reach for. Hand it a collection, it emits one item at a time.

```toml
[[handlers]]
name = "split-invoices"
path = "~/.local/share/emergent/primitives/bin/stream-runner"
args = ["--load-topic", "inbox.listed", "--publish-as", "invoice.detected",
        "--ack-topic", "invoice.settled", "--end-topic", "inbox.drained",
        "--items-key", "items"]
subscribes = ["inbox.listed", "invoice.settled"]
publishes = ["invoice.detected", "inbox.drained"]
```

It holds each item until the ack topic fires, so exactly one item is in flight
at a time. That is a feature: in-flight work is bounded, and a slow downstream
stage applies backpressure to the whole batch rather than building a queue.

Two constraints to design around:

- **One collection at a time.** A `load` arriving while a stream is still
  running is logged and dropped. With an interval poller, size the interval so a
  batch drains before the next arrives, or gate polling on the end topic.
- **A missing ack stalls it forever.** The ack topic must be something the
  downstream path always publishes, on both the success and failure paths.

### 2. HTTP fan-out (when items must be concurrent)

A shell step POSTs once per item into an `http-source`. Each POST is an
independent event, they do not wait on each other, and no custom code is needed.
This is the default when items are independent.

```toml
[[sources]]
name = "invoice-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8090"]
publishes = ["invoice.detected"]

[[sinks]]
name = "split-inbox"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "inbox.listed", "--", "bash", "-c",
  """jq -r '.stdout' | while IFS= read -r line; do \
       [ -n "$line" ] || continue; \
       jq -n --arg p "$line" '{path: $p}' \
       | curl -sf -X POST -H 'Content-Type: application/json' -d @- \
           http://127.0.0.1:8090; \
     done"""]
subscribes = ["inbox.listed"]
```

The body arrives nested, so downstream handlers read `.body.path`.

Note the `while` loop. That is not the Internal Loop anti-pattern: it performs
no work per item, it only *emits*. A loop that publishes one event per item is
the fan-out mechanism itself. A loop that processes items is what you are
avoiding.

### 3. A small SDK splitter

Every SDK has `publish_all` / `publish_stream` for exactly this. A splitter is
about ten lines, publishes N events with no ack gating, and keeps causation
intact.

```python
from emergent import run_handler, create_message

async def split(msg, handler):
    for item in msg.payload_as(dict)["items"]:
        await handler.publish(
            create_message("invoice.detected").caused_by(msg.id).payload(item)
        )

import asyncio
asyncio.run(run_handler("split-inbox", ["inbox.listed"], split))
```

### Choosing

| Need | Use |
|---|---|
| Split a collection, marketplace primitives only | `stream-runner`. The default. |
| Bounded in-flight work, backpressure, or ordering | `stream-runner`. That is what the ack buys. |
| Many items genuinely in flight at once | HTTP fan-out |
| Clean causation, high volume, willing to write ten lines | SDK splitter |
| One item per poll anyway | Nothing. The source already emits one event. |

Note that concurrency across *handlers* is unaffected by this choice. Whichever
splitter you use, every subscriber to an item's event still runs in parallel.
The only thing at stake here is how many items are in flight simultaneously.

## Fan-out: concurrency

One event, several independent subscribers. This is how Emergent expresses
"do these at the same time." The engine delivers to all matching subscribers
simultaneously, so the work runs concurrently in separate processes, each
supervised, observable, and retryable on its own.

```toml
[[handlers]]
name = "score-severity"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.scored", "--", "./score.sh"]
subscribes = ["issue.found"]
publishes = ["issue.scored"]

[[handlers]]
name = "detect-duplicates"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.dupe-checked", "--", "./dupes.sh"]
subscribes = ["issue.found"]
publishes = ["issue.dupe-checked"]
```

Neither knows the other exists. Adding a third analysis is one more block and
zero edits, which is the whole point.

Contrast with `xargs -P` or `asyncio.gather` inside one primitive: same
parallelism, but the engine cannot see it, nothing can subscribe to an
individual result, a partial failure is invisible, and adding a fourth analysis
means editing a script.

## Fan-in: convergence

Several publishers, one subscriber. No merge node is required because
subscription is by message type, so any number of primitives can feed one.

```toml
[[handlers]]
name = "formatter"
subscribes = ["metric.cpu", "metric.memory", "metric.disk"]
publishes = ["monitor.metric"]
```

The two shapes of fan-in worth distinguishing:

- **Different types converging** (above): free, stateless, no coordination.
- **The same type from multiple publishers**: also free. Two primitives that both
  publish `phase.request` are a legitimate way to have a seeder and a feedback
  edge feed the same stage.

## The join: waiting for N

When you need *all* results before proceeding, accumulate by a key.
`exec-handler` publishes nothing on exit 0 with empty stdout, so the joiner stays
silent until the last arrival.

```toml
[[handlers]]
name = "joiner"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "analysis.done", "--publish-as", "analysis.complete", "--", "sh", "-c",
  """p=$(cat); k=$(jq -r .invoice_id <<< "$p"); f=/tmp/join-$k.jsonl; \
     echo "$p" >> $f; \
     [ $(wc -l < $f) -ge 3 ] && jq -s -c --arg k "$k" '{invoice_id: $k, results: .}' $f && rm -f $f"""]
subscribes = ["analysis.done"]
publishes = ["analysis.complete"]
```

### Choosing the join key

This is the detail that quietly breaks joins, so get it right up front.

`exec-source --correlate` resolves a correlation ID **once at process startup**
and stamps that same ID on every message the process ever publishes. With
`--interval`, that means every poll and every item produced across the whole
lifetime of the source shares one ID. Every `exec-handler` then propagates it
downstream unchanged.

So `correlation_id` identifies **a run, not an item**:

| Situation | Right join key |
|---|---|
| One-shot engine, launched per run, exits when done | `correlation_id` is exactly the run identity. Use it. |
| Interval poller emitting many items | A natural key from the payload (invoice path, issue number, deploy sha). `correlation_id` would collide every item into one bucket. |
| Work spanning two engines | `--correlation-id` to adopt the parent's ID, deliberately grouping both. |

`correlation_id` remains the right thing for tracing a run in the event store and
for `bin/audit.sh`-style queries. It is just not an item key.

The count is hardcoded above. When the expected count varies, carry it in the
payload and compare against it, or use a small stateful SDK handler. A handler
holding a join table is one of the few genuinely good reasons to write custom
code, because the accumulator is irreducible.

Note also that this file-based joiner is not crash-safe and does not expire
partial joins. For anything where a lost result would matter, a stateful SDK
handler with a real timeout is the honest answer.

## Predicate routing: branching

An `if`/`else` becomes N subscribers on the same event with mutually exclusive
predicates. Nothing decides between them. All of them look; one matches.

```toml
[[handlers]]
name = "route-confident"
args = ["-s", "issue.scored", "--publish-as", "issue.triaged", "--",
        "jq", "-c", "select(.confidence >= 0.8)"]
subscribes = ["issue.scored"]
publishes = ["issue.triaged"]

[[handlers]]
name = "route-uncertain"
args = ["-s", "issue.scored", "--publish-as", "issue.uncertain", "--",
        "jq", "-c", "select(.confidence < 0.8)"]
subscribes = ["issue.scored"]
publishes = ["issue.uncertain"]
```

Why this beats a script with a branch: each route is separately observable in
the log, separately swappable, and a new route is a new block rather than an
edit. The predicates are also visible in the config, so the routing logic is
readable without opening any code.

Keep predicates mutually exclusive and exhaustive. Overlapping predicates mean an
event takes two paths, which is occasionally what you want but never what you
want by accident.

## The silent filter

`jq select()` exits non-zero with empty stderr when the predicate is false, and
`exec-handler` treats that as a silent drop. Filtering therefore needs no code
and no explicit "discard" path.

```toml
args = ["-s", "raw.event", "--", "jq", "-c",
        "select(.type == \"message\" and (.bot_id // null) == null)"]
```

## Feedback loops

An event re-entering an earlier stage. This is the mechanism that separates a
system from a pipeline, and nothing genuinely emergent happens without one.

The shape: a downstream handler republishes a type that an upstream stage already
subscribes to, usually with something changed in the payload.

```toml
[[handlers]]
name = "enrich-context"
args = ["-s", "issue.uncertain", "--publish-as", "issue.found", "--", "sh", "-c",
        "jq -c '.context += [\"related issues attached\"] | .attempt += 1'"]
subscribes = ["issue.uncertain"]
publishes = ["issue.found"]
```

`issue.found` is where triage began, so an uncertain issue re-enters triage with
more context than it had. Nothing coordinates this. How many passes an item takes
is a consequence of the data meeting the thresholds, which is precisely the
behavior nobody wrote.

Always pair a feedback edge with a depth guard.

## Depth guards: terminating a loop

A loop counter lives in the payload; the guard is a predicate.

```toml
[[handlers]]
name = "depth-guard"
args = ["-s", "issue.found", "--publish-as", "issue.escalated", "--",
        "jq", "-c", "select(.attempt >= 3)"]
subscribes = ["issue.found"]
publishes = ["issue.escalated"]
```

Note that this runs *alongside* normal processing rather than gating it. If you
want the guard to actually divert rather than duplicate, make the normal path's
predicate exclusive too (`select(.attempt < 3)`). Being explicit about both sides
is worth the extra block, because an unbounded feedback loop is the one failure
mode of this architecture that will fill a disk.

## Failure as data

Errors are events, not exceptions. Publish `<domain>.failed` and let a separate
subscriber own the response, which means the failure path is as observable and
extensible as the success path.

`exec-handler` does this for you: a non-zero exit with stderr publishes on the
`--error-as` topic (default `exec.error`). Give failures a domain-specific name
when the response differs by domain:

```toml
args = ["-s", "issue.found", "--publish-as", "issue.scored",
        "--error-as", "issue.score-failed", "--", "./score.sh"]
```

## Retry with backoff

Failure event, delay, republish the original type. The attempt count rides in the
payload and a guard ends it.

```toml
[[handlers]]
name = "retry-scoring"
args = ["-s", "issue.score-failed", "--publish-as", "issue.found", "--", "sh", "-c",
        "sleep $(( 2 ** ${ATTEMPT:-1} )); jq -c '.attempt += 1'"]
subscribes = ["issue.score-failed"]
publishes = ["issue.found"]
```

The retry is a first-class part of the topology, so you can see every attempt in
the log and change the policy without touching the thing being retried.

## Paced streaming

`stream-runner` is both the splitter (see above) and the pacing mechanism, and
those are the same behavior seen from two angles: it emits one item, waits for
an ack, then emits the next. One item is in flight at a time.

Usually that is what you want. It bounds in-flight work, applies backpressure
from a slow stage back to the source rather than growing a queue, and preserves
order. Note what it does *not* cost you: every handler subscribing to an item's
event still runs concurrently. Only the number of simultaneous *items* is
capped.

Reach for a different splitter when items being simultaneous is the actual
requirement:

| Situation | Use |
|---|---|
| Default, and anywhere backpressure or order matters | `stream-runner` |
| Rate-limited API, quota, single-seat licence | `stream-runner`, paced by the downstream ack |
| Throughput bound by slow per-item work you want overlapped | HTTP fan-out or an SDK splitter |
| A batch must fully drain before the next arrives | `stream-runner`, gating the poll on the end topic |

The ack topic should be the downstream stage's own output, so consumption sets
the pace with no sleep and no guesswork. Make sure something publishes it on the
failure path too, or one bad item stalls the whole batch.

```toml
[[handlers]]
name = "pace-items"
path = "~/.local/share/emergent/primitives/bin/stream-runner"
args = ["--load-topic", "batch.ready", "--publish-as", "item.next",
        "--ack-topic", "item.processed", "--end-topic", "batch.done",
        "--items-key", "records"]
subscribes = ["batch.ready", "item.processed"]
publishes = ["item.next", "batch.done"]
```

Use plain fan-out instead when the consumer is not the bottleneck. Pacing you do
not need is just latency.

## Non-deterministic routing: the model picks the next event

Every other pattern here routes on a rule. This one routes on a judgment, which
is what you reach for when no predicate can express the decision: is this diff
ready to merge, is this ticket a duplicate, is this alert worth waking someone
for.

The structure has two steps, and they are worth naming separately because only
the first has a design choice in it:

1. **A judgment is produced**, as one event of one type.
2. **The judgment becomes a transition**, by exclusive routers turning that one
   type into the domain vocabulary.

Step 2 is identical in every shape below. It is also what makes the pattern
work: a judging primitive publishes one fixed type, so alone it can report a
verdict but never choose between flows. Routers are what turn a judgment into a
transition, and once they exist, downstream handlers cannot tell a model was
involved. That is the property worth protecting, because it means the
non-deterministic node is swappable for a rule later without disturbing
anything.

Step 1 has two mechanisms. Start with the cheaper one.

### Shape A: an `exec-handler` judges (default)

The agent prints one JSON verdict on stdout, and `--publish-as` makes that an
event. No port, no re-entry, no orphaned trace.

```toml
[[handlers]]
name = "judge-file"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "file.changed", "--publish-as", "review.submitted",
        "-e", "review.judge-failed", "-t", "180000", "--", "bash", "-c",
  """f=$(jq -r .path); \
     claude -p "Review $f against our conventions. Print exactly one JSON object \
       on stdout and nothing else, either {\"verdict\":\"approved\",\"path\":\"$f\"} \
       or {\"verdict\":\"needs_work\",\"path\":\"$f\",\"reasons\":[...]}. \
       Do not edit files, do not comment, do not act further." \
       --allowedTools Read Grep"""]
subscribes = ["file.changed"]
publishes = ["review.submitted", "review.judge-failed"]

# The vocabulary lives here. Each outcome is one router.
[[handlers]]
name = "route-approved"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "review.submitted", "--publish-as", "review.approved", "--",
        "jq", "-c", "select(.verdict == \"approved\")"]
subscribes = ["review.submitted"]
publishes = ["review.approved"]

[[handlers]]
name = "route-needs-work"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "review.submitted", "--publish-as", "review.needs-work", "--",
        "jq", "-c", "select(.verdict == \"needs_work\")"]
subscribes = ["review.submitted"]
publishes = ["review.needs-work"]

# Exhaustiveness. Model output is untrusted input, so an unrecognized verdict
# must become an event rather than a silent drop.
[[handlers]]
name = "route-invalid"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "review.submitted", "--publish-as", "review.invalid-verdict", "--",
        "jq", "-c", "select((.verdict // \"\") | IN(\"approved\", \"needs_work\") | not)"]
subscribes = ["review.submitted"]
publishes = ["review.invalid-verdict"]
```

Adding a third outcome (`review.blocked`, say) is one more router and one more
line in the prompt. Nothing else in the topology moves.

Four properties make this the default:

- **The trace survives the model call.** `exec-handler` inherits the inbound
  `correlation_id` and stamps `causation_id` from the triggering message, so the
  verdict is causally linked to the file change that provoked it. Shape B
  severs both.
- **Failure is already an event.** A non-zero exit with stderr, or a timeout,
  publishes the error type (`-e`, default `exec.error`). Route it like anything
  else.
- **Malformed output is still an event.** `exec-handler` parses stdout as JSON
  and falls back to `{"output": "..."}`, so an agent that ignores the format and
  narrates lands at `route-invalid` rather than vanishing.
- **No listening port**, so there is no actuator to defend and no untrusted
  network input.

The one hole: a clean exit with empty stdout publishes nothing, and so does a
non-zero exit with empty stderr. If either is plausible for your agent, add the
reaper below.

### Shape B: the agent re-enters through an `http-source`

An `exec-sink` dispatches, and the agent POSTs its verdict back in. Its tool
call is the publish. This buys expressiveness that Shape A structurally cannot
offer, and costs the trace.

Reach for it when one execution producing exactly one event is what blocks you:

- the decision yields **zero, one, or many** events rather than exactly one
- the agent should emit **while** it works rather than once at the end, so
  downstream can start on early findings
- the decider is **out of band**: another machine, a queue, a human clicking a
  link in a notification

`http-source` takes its message type from the first entry of
`EMERGENT_PUBLISHES`, so one source emits exactly one type no matter what
arrives. That is not a limit to work around, it is the shape to build on: **one
source receives a single input, and the routers downstream publish as many
distinct events as you like.** The agent's interface stays one stable URL as the
vocabulary grows, and validation lives in one place.

```toml
[[sources]]
name = "verdict-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8091"]
publishes = ["review.submitted"]

[[sinks]]
name = "dispatch-review"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "file.changed", "-t", "180000", "--", "bash", "-c",
  """p=$(cat); f=$(jq -r .path <<< "$p"); r="$EMERGENT_CORRELATION_ID"; \
     touch "/tmp/pending-review-$(basename "$f")"; \
     claude -p "Review $f against our conventions. POST one verdict per issue \
       you find to localhost:8091 with curl, as JSON: \
       {\"verdict\":\"needs_work\",\"path\":\"$f\",\"run\":\"$r\",\"reason\":\"...\"}. \
       If the file is clean, POST exactly one \
       {\"verdict\":\"approved\",\"path\":\"$f\",\"run\":\"$r\"} instead. \
       Do not edit files, do not comment, do not act further." \
       --allowedTools Bash Read"""]
subscribes = ["file.changed"]

[[handlers]]
name = "clear-pending"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "review.approved", "-s", "review.needs-work",
        "--publish-as", "review.settled", "--", "bash", "-c",
  """p=$(cat); f=$(jq -r .path <<< "$p"); \
     rm -f "/tmp/pending-review-$(basename "$f")"; \
     echo "$p\""""]
subscribes = ["review.approved", "review.needs-work"]
publishes = ["review.settled"]
```

The routers from Shape A apply unchanged except that the body arrives nested, so
each predicate reads `.body.verdict` and emits `.body`.

### Shape B2: one endpoint per event

A variant of B for when you want the event type structurally guaranteed rather
than parsed out of model output. Give each outcome its own source on its own
port. The agent's set of URLs is then literally its action space, and no router
or validator is needed because an unknown URL simply fails to connect.

```toml
[[sources]]
name = "approved-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8091"]
publishes = ["review.approved"]

[[sources]]
name = "needs-work-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8092"]
publishes = ["review.needs-work"]
```

The cost is that every new outcome needs a port, a process, and a prompt change,
and there is no single place to validate. Prefer plain B unless the guarantee
matters more than the extensibility.

### The reaper: handling silence

Mandatory under Shape B, optional under Shape A. An `exec-sink` discards output,
so a model that refuses, crashes, or emits nothing produces no event at all:
nothing downstream fires and nothing complains. Pair the dispatch with a pending
marker (above) and scan for stale ones:

```toml
[[sources]]
name = "review-reaper"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--interval", "60000", "--shell", "bash", "--command",
  """find /tmp -maxdepth 1 -name 'pending-review-*' -mmin +5 \
     -exec basename {} \\; | sed 's/^pending-review-//' \
     | jq -R -c '{file: ., reason: "agent emitted no verdict"}'"""]
publishes = ["exec.output"]

[[handlers]]
name = "raise-stalled"
args = ["-s", "exec.output", "--publish-as", "review.stalled", "--",
        "jq", "-c", ".stdout | select(length > 0) | fromjson"]
subscribes = ["exec.output"]
publishes = ["review.stalled"]
```

`review.stalled` is then just another event: retry it, escalate it, or route it
to a human. The non-determinism is contained because its failure mode has a
name.

### What breaks across the re-entry boundary

Everything here is specific to Shape B except the last row, which applies to any
model call. This table is the argument for defaulting to Shape A: it collapses
to one row there.

| Concern | What happens | What to do |
|---|---|---|
| Causation | `http-source` sets no `causation_id`. The chain severs. | Accept it, or write a small SDK source that stamps it from the body. |
| Correlation | `http-source` sets no `correlation_id` either. | Have the agent echo `EMERGENT_CORRELATION_ID` (the sink gets it in env) into the body; use it as the trace key downstream. |
| Payload shape | Everything the agent POSTs arrives nested under `.body`. | Unwrap with `.body` in the first handler, as the routers above do. |
| Trust | The endpoint is an actuator: whatever can POST there drives the system. | Bind `--host 127.0.0.1`, consider `--secret` for HMAC, and always validate the vocabulary. |
| Cost and latency | Every decision is a model call. | Filter hard *before* the judging primitive so the agent only judges what genuinely needs judgment. |

### Keeping it honest

The value of this pattern comes from the agent's action space being exactly your
event vocabulary. It decides one transition and announces it. The moment you
hand it tools to act on the world directly, or prompt it to "handle" something
rather than classify it, you have built The LLM Conductor: the whole workflow
back inside one opaque call, now non-deterministic as well as hidden.

The test is simple. If the agent emitted the wrong event, could the rest of the
system still behave sensibly? If yes, the boundary is right.

## System events as triggers

The engine publishes lifecycle events into the same fabric as everything else, so
they are subscribable like any other type.

| Event | When |
|---|---|
| `system.started.<name>` | Primitive started |
| `system.stopped.<name>` | Primitive stopped gracefully |
| `system.error.<name>` | Primitive failed |
| `system.shutdown` | Engine shutting down (SDKs handle internally) |
| `system.shutdown.requested` | Shutdown signal received, before drain |

Wildcards match: `system.started.*`, `system.error.*`.

This makes the system reflexive. A watchdog that reacts to `system.error.*` by
publishing an alert event is three lines of TOML, and the topology can respond to
its own health the same way it responds to domain data.

## Self-seeding

A topology that starts itself, rather than waiting for a manual poke. Subscribe
the seeding step to `system.started.<name>` of whatever must be ready first.

```toml
[[sinks]]
name = "seeder"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "system.started.webhook", "--", "sh", "-c",
        "curl -s -X POST -d '{\"count\":0}' http://localhost:8088"]
subscribes = ["system.started.webhook"]
```

Combined with a feedback edge, this is the ouroboros: the loop seeds itself on
startup and then sustains itself. Startup order (Sinks, then Handlers, then
Sources) guarantees the consumers exist before anything is published.

## Stateful accumulators

When something genuinely must accumulate (a simulation grid, a rolling window, a
join table), one small SDK handler owns that accumulator and nothing else.

```python
from emergent import run_handler, create_message

window = []

async def process(msg, handler):
    window.append(msg.payload_as(dict)["value"])
    del window[:-100]
    avg = sum(window) / len(window)
    await handler.publish(
        create_message("metric.rolling-avg").caused_by(msg.id).payload({"avg": avg})
    )

import asyncio
asyncio.run(run_handler("rolling-avg", ["metric.raw"], process))
```

The discipline that keeps this from becoming private state: **publish every
state change**. If the accumulator changes and no event says so, the rest of the
system is blind to it and nothing can react. State that is not in the stream
might as well be in another process on another machine.

## Making a topology injectable

Add an `http-source` and you can POST any event into the middle of the system by
hand. This is how you satisfy the injection test, how you test a stage in
isolation, and how a human intervenes in a running system without restarting it.

```toml
[[sources]]
name = "inject"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "8099", "--path", "/inject"]
publishes = ["http.request"]
```

Route the body to whatever type you want to simulate with a one-line jq handler.
A topology you can poke is a topology you can debug.
