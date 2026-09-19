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

**No exec primitive splits its stdout.** `exec-handler` publishes at most one
event per execution: it parses stdout as a single JSON value and, when that
fails, wraps the whole thing as `{"output": "..."}`. `exec-source` publishes at
most one stdout event, holding all output in one `stdout` string (plus a stderr
event when stderr is non-blank, and always an exit event). So this, which appears in a lot of plausible-looking
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
# The poll prints ONE JSON array. `--shell sh` because the command is a pipe.
[[sources]]
name = "list-inbox"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--interval", "300000", "--shell", "sh", "--command",
  '''find /srv/inbox -name '*.pdf' | jq -R -s -c 'split("\n") | map(select(length > 0) | {path: .})' ''']
publishes = ["inbox.listed", "inbox.list-failed", "inbox.list-done"]

[[handlers]]
name = "split-invoices"
path = "~/.local/share/emergent/primitives/bin/stream-runner"
args = ["--load-topic", "inbox.listed", "--publish-as", "invoice.detected",
        "--ack-topic", "invoice.settled", "--end-topic", "inbox.drained"]
unwrap_stdout = true
subscribes = ["inbox.listed", "invoice.settled"]
publishes = ["invoice.detected", "inbox.drained"]
```

The load must be a bare JSON array, or an object holding one under
`--items-key` (default `items`). An `exec-source` payload is neither: it is
`{command, stdout, exit_code}`. `unwrap_stdout = true` has the SDK parse
`.stdout` before `stream-runner` sees the message, so the array arrives bare and
no shaping handler is needed. A load of the wrong shape is dropped, and the
warning is visible only with `RUST_LOG=warn` in the primitive's `env`.

It holds each item until the ack topic fires, so exactly one item is in flight
at a time. That is a feature: in-flight work is bounded, and a slow downstream
stage applies backpressure to the whole batch rather than building a queue.
Items and the end event carry the load's `correlation_id` and a `causation_id`
pointing at the load. The end payload is `{"count": N}`, and an empty array
publishes it immediately.

Three constraints to design around:

- **One collection at a time.** A load arriving while a stream is still running
  is dropped (again, a warning you only see with `RUST_LOG=warn`). With an
  interval poller that is usually what you want, because no item is ever in
  flight twice. Size the interval so a batch normally drains first.
- **A missing ack stalls it until the engine restarts.** There is no timeout.
  The ack topic must be something the downstream path always publishes, on the
  success path and on every failure path. Route each `--error-as` topic to an
  event that ends in the ack; never leave one on `exec.error`.
- **Acks are not matched to items.** Any message on the ack topic releases the
  next item. Keep that topic exclusive to this stream.

`--ack-topic` takes one topic. A flow with several exits (filed, escalated,
rejected) needs a fan-in handler that turns each exit into the one ack, like
`settle` in the worked example.

**Where the ack goes decides what a stall and a crash cost.** Two placements,
and the choice is yours to state:

- **Ack at the end of the flow.** The source stays a durable queue: if the
  engine dies mid-item, the next poll finds the item again. The price is
  head-of-line blocking. One item sleeping in a retry backoff holds every item
  behind it, so keep delays short or bound the attempts tightly.
- **Ack at the claim.** The first handler takes ownership of the item (below)
  and its success event is the ack. Nothing blocks and a backoff costs only the
  item that is backing off. The price is recovery: an item claimed when the
  engine dies has no event in flight, so plan how a stranded claim re-enters.

**A poll needs a seen-set, or every poll re-detects everything.** Put it in the
world, not in a primitive. For an API, poll with a query the topology's own exits
falsify (`no:label`, `status=new`). For a directory, claim the file: rename it
out of the inbox, which is atomic, and announce the new path.

```toml
# `act && shape`: one act (`mv`), then the announcement. A second claim of the
# same file fails the `mv`, which is a non-zero exit, which is the error topic.
[[handlers]]
name = "claim-invoice"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "invoice.detected", "--publish-as", "invoice.claimed",
        "--error-as", "invoice.claim-failed", "--", "bash", "-c",
  '''p=$(cat); dst="/srv/claimed/$(jq -r '.path | split("/") | last' <<< "$p")"; \
     mv -n "$(jq -r .path <<< "$p")" "$dst" && jq -c --arg path "$dst" '.path = $path' <<< "$p" ''']
subscribes = ["invoice.detected"]
publishes = ["invoice.claimed", "invoice.claim-failed"]
```

`exec-source` cannot watch a directory for you. It waits for its command to exit
and then publishes, so a command that never exits (`inotifywait -m`, `tail -f`)
never publishes anything. Poll on `--interval` instead. Runs never overlap: a
run that outlasts the interval is followed immediately by the next. Directories
like `/srv/claimed` are deployment prerequisites. Create them where you install
the config (a systemd `ExecStartPre`, a Taskfile step), not with a `mkdir`
inside a primitive, where it would be a second act on every message.

`worked-example.md` runs this shape end to end, including the failure paths.

### 2. HTTP fan-out (when items must be concurrent)

A shell step POSTs once per item into an `http-source`. Each POST is an
independent event and no custom code is needed. Reach for it when items being
in flight together is the actual requirement.

Splitting alone does not make items overlap. Every `exec-handler` and
`exec-sink` runs **one command at a time, in arrival order**, unless you raise
`--max-concurrent`. So add `"--max-concurrent", "N"` to each per-item handler
downstream, and expect output order to follow completion rather than arrival.

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
  '''set -o pipefail; jq -c '.stdout | fromjson | .[]' | while IFS= read -r item; do \
       curl -sf -X POST -H 'Content-Type: application/json' -d "$item" http://127.0.0.1:8090; \
     done''']
subscribes = ["inbox.listed"]
```

The body arrives nested, so downstream handlers read `.body.path`.

Note the `while` loop. It is the one loop the ladder in `SKILL.md` permits: its
body performs no work, it only *emits*. A loop that publishes one event per item
is a splitter. A loop that processes items is the Internal Loop anti-pattern.
Prefer `stream-runner` anyway unless you need the overlap, because it is a
marketplace primitive used as itself and this is a shell body you have to own.

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
| Many items genuinely in flight at once | HTTP fan-out, plus `--max-concurrent N` on each per-item handler |
| Clean causation, high volume, willing to write ten lines | SDK splitter |
| One item per poll anyway | Nothing. The source already emits one event. |

Note that concurrency across *handlers* is unaffected by this choice. Whichever
splitter you use, every subscriber to an item's event still runs in parallel
with the others. What is at stake here is how many items are in flight, and
within one handler that is also capped by its `--max-concurrent` (default 1).

## Fan-out: concurrency

One event, several independent subscribers. This is how Emergent expresses
"do these at the same time." The engine delivers to all matching subscribers
simultaneously, so the work runs concurrently in separate processes, each
supervised, observable, and retryable on its own.

```toml
[[handlers]]
name = "score-severity"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.scored", "-t", "60000", "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); \
     jq -r '"Rate this issue as JSON {severity, confidence}.\n\(.title)\n\(.body)"' <<< "$p" \
     | claude -p --tools "" --output-format text \
     | jq -c --argjson orig "$p" '$orig + {severity, confidence}' ''']
subscribes = ["issue.found"]
publishes = ["issue.scored"]

[[handlers]]
name = "detect-duplicates"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.dupe-checked", "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); \
     gh issue list --state open --search "$(jq -r .title <<< "$p")" --json number \
     | jq -c --argjson orig "$p" '{number: $orig.number, duplicate_of: (map(.number) - [$orig.number] | first)}' ''']
subscribes = ["issue.found"]
publishes = ["issue.dupe-checked"]
```

Each body is one act between two shapes: capture the payload, call one thing,
merge the identity back. Neither is a script file, and neither needs one.
`set -o pipefail` is what makes the act's failure the body's failure: without it
the trailing `jq` exits 0 on empty input and a dead `claude` or `gh` publishes
nothing at all. And the merge names the fields it takes from the act
(`{severity, confidence}`), so a model reply cannot overwrite `number`.

Siblings are independent only while they leave each other's world alone. If one
subscriber renames a file or edits a record that another one reads, the event
order no longer tells you what the second one saw. Give each sibling what it
needs in the payload, or put the mutation downstream of the reader.

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
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "metric.cpu", "-s", "metric.memory", "-s", "metric.disk",
        "--publish-as", "monitor.metric", "--",
        "jq", "-c", "{metric: env.EMERGENT_MESSAGE_TYPE, reading: .}"]
subscribes = ["metric.cpu", "metric.memory", "metric.disk"]
publishes = ["monitor.metric"]
```

`exec-handler` exports the inbound message's type, id, source, correlation and
causation ids to the command as `EMERGENT_MESSAGE_TYPE` and friends, so a
converging handler can tell its inputs apart without a shell.

Use one converging handler when the inputs get the same treatment. When each
input needs its own words (three escalation reasons, say), write one small
handler per input, each publishing the same type with a literal reason, as the
worked example does. A reader of the log gets prose instead of a type name.

The two shapes of fan-in worth distinguishing:

- **Different types converging** (above): free, stateless, no coordination.
- **The same type from multiple publishers**: also free. Two primitives that both
  publish `phase.request` are a legitimate way to have a seeder and a feedback
  edge feed the same stage.

## The join: waiting for N

When you need *all* results before proceeding, accumulate by a key. A join is
three small things, and keeping them separate is what stops it turning into a
script: an accumulator that publishes its bucket on every arrival, a router
that recognizes a full bucket, and a sink that clears it.

```toml
# The accumulator. It owns the bucket and nothing else, and it publishes every
# state change, so a partial join is visible in the log as it fills.
[[handlers]]
name = "collect-analyses"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "analysis.done", "--publish-as", "analysis.collected", "--", "bash", "-c",
  '''p=$(cat); f="/tmp/join-$(jq -r .invoice_id <<< "$p").jsonl"; \
     jq -c . <<< "$p" >> "$f"; \
     jq -s -c '{invoice_id: .[0].invoice_id, results: .}' "$f" ''']
subscribes = ["analysis.done"]
publishes = ["analysis.collected"]

# The completeness test is a predicate in the config, not an `if` in a shell.
# `==` rather than `>=`, so a late fourth arrival cannot fire it twice.
[[handlers]]
name = "route-complete"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "analysis.collected", "--publish-as", "analysis.complete", "--",
        "jq", "-c", "select(.results | length == 3)"]
subscribes = ["analysis.collected"]
publishes = ["analysis.complete"]

[[sinks]]
name = "clear-joined"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "analysis.complete", "--", "bash", "-c",
  '''rm -f "/tmp/join-$(jq -r .invoice_id).jsonl"''']
subscribes = ["analysis.complete"]
```

The accumulator is safe without a lock because `exec-handler` runs one command
at a time by default. Do not raise `--max-concurrent` on it.

Do not collapse this into `[ $(wc -l < $f) -ge 3 ] && jq ...`. It reads as
silent-until-full and is not: a false `[ ]` exits 1, and since primitives 0.10.0
every non-zero exit publishes an error event. Two of three arrivals would land
on `exec.error`.

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

`correlation_id` remains the right thing for tracing a run in the event store.
It is just not an item key:

```bash
sqlite3 ~/.local/share/emergent/<engine.name>/events.db \
  "SELECT message_type, source, payload_json FROM events
   WHERE correlation_id = 'cor_...' ORDER BY timestamp_ms"
```

The count is hardcoded above. When the expected count varies, carry it in the
payload and compare against it (`select(.results | length == .[0].expected)`),
or use a small stateful SDK handler. A handler
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
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.triaged", "--",
        "jq", "-c", "select(.confidence >= 0.8)"]
subscribes = ["issue.scored"]
publishes = ["issue.triaged"]

[[handlers]]
name = "route-uncertain"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.uncertain", "--",
        "jq", "-c", "select(.confidence >= 0.8 | not)"]
subscribes = ["issue.scored"]
publishes = ["issue.uncertain"]
```

Why this beats a script with a branch: each route is separately observable in
the log, separately swappable, and a new route is a new block rather than an
edit. The predicates are also visible in the config, so the routing logic is
readable without opening any code.

Keep predicates mutually exclusive and exhaustive. Overlapping predicates mean an
event takes two paths, which is occasionally what you want but never what you
want by accident. Write the last arm as the negation of the others, as above,
rather than as its own comparison. jq compares across types (`null` sorts below
every number, every string above), so a missing or malformed `confidence` lands
somewhere surprising, and the moment you tighten the first arm a hand-written
opposite stops being its opposite. The negation cannot drift. An event that
matches no router vanishes without a trace.

## The silent filter

`jq select()` prints nothing and exits 0 when the predicate is false, and
`exec-handler` publishes nothing on exit 0 with empty stdout. Filtering therefore
needs no code and no explicit "discard" path.

That is the *only* silence. Every non-zero exit publishes the error type, so a
shell test such as `[ -n "$x" ] && cmd` is not a filter: its false case exits 1
and lands on `exec.error`. When a command's non-zero exit really is a normal
outcome (`jq -e` exits 4 on no output, `grep` exits 1 on no match), declare it
with `"--silent-exit-codes", "4"` before the `--`.

```toml
args = ["-s", "raw.event", "--", "jq", "-c",
        "select(.type == \"message\" and (.bot_id // null) == null)"]
```

## Feedback loops

An event re-entering an earlier stage. This is the mechanism that separates a
system from a pipeline, and nothing genuinely emergent happens without one.

The shape: a downstream handler publishes an event that an upstream stage
subscribes to, with something changed in the payload.

```toml
[[handlers]]
name = "enrich-context"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.uncertain", "--publish-as", "issue.enriched", "--",
        "jq", "-c", ".context += [\"related issues attached\"] | .attempt += 1"]
subscribes = ["issue.uncertain"]
publishes = ["issue.enriched"]

# The upstream stage listens for the edge as well as the ingress event.
[[handlers]]
name = "score-severity"
# ...
subscribes = ["issue.found", "issue.enriched"]
```

An uncertain issue goes back to the judge with more context than it had. Nothing
coordinates this. How many passes an item takes is a consequence of the data
meeting the thresholds, which is precisely the behavior nobody wrote.

Name the edge for what became true (`issue.enriched`) and subscribe the upstream
stage to it. The tempting shortcut is to republish the ingress type
(`issue.found`), and it has a cost: *every* subscriber of the ingress type runs
again on each pass, including the ones that had nothing to do with the loop. In
the worked example that shortcut re-ran the duplicate check and re-applied its
label once per attempt.

Always pair a feedback edge with a depth guard.

## Depth guards: terminating a loop

A loop counter lives in the payload, and the guard is one arm of the branch that
feeds the loop. It must *divert*, not merely observe:

```toml
[[handlers]]
name = "route-uncertain"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.uncertain", "--",
        "jq", "-c", "select((.confidence >= 0.8 | not) and .attempt < 3)"]
subscribes = ["issue.scored"]
publishes = ["issue.uncertain"]

[[handlers]]
name = "depth-guard"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.escalated", "--",
        "jq", "-c", "select((.confidence >= 0.8 | not) and .attempt >= 3) | {number, attempt}"]
subscribes = ["issue.scored"]
publishes = ["issue.escalated"]
```

The two predicates split the uncertain case between them, so a third-attempt
issue matches the guard and nothing else. A guard that only subscribes alongside
the normal path (a bare `select(.attempt >= 3)` next to an unguarded loop)
raises the alarm while the item keeps circulating. An unbounded feedback loop is
the one failure mode of this architecture that will fill a disk, so check that
some predicate on the loop's own path goes false.

## Failure as data

Errors are events, not exceptions. Publish `<domain>.failed` and let a separate
subscriber own the response, which means the failure path is as observable and
extensible as the success path.

`exec-handler` does this for you. Any non-zero exit, a timeout, or a failure to
spawn the command publishes on the `--error-as` topic (default `exec.error`).
The payload is the inbound payload with one reserved key added:

```
{...inbound fields, "error": {"exit_code": 22, "stderr": "...", "command": "..."}}
```

A timeout reports `exit_code: null` and `stderr: "process timed out"`, after the
whole process group gets SIGTERM and then, `--kill-grace-ms` later, SIGKILL. The
error event keeps the inbound `correlation_id` and `causation_id`, so the failure
sits in the same trace as the attempt. Because the original fields survive, a
retry handler can republish the item by dropping `.error`.

Give failures a domain-specific name when the response differs by domain:

```toml
args = ["-s", "issue.found", "--publish-as", "issue.scored",
        "--error-as", "issue.score-failed", "--",
        "curl", "-sf", "-X", "POST", "--data-binary", "@-", "http://127.0.0.1:11434/score"]
```

That bare form fits a call whose response is the whole result. It has two
limits. The response replaces the payload, so the item's identity is gone unless
the service echoes it. And `-f` folds "the service is down" and "the service
said no" into one failure, so a retry policy hung on it will retry a 422 that
can never succeed. When the difference matters, make the status data and let
routers read it:

```toml
# Transport failure (refused, DNS, timeout) is a non-zero exit: the error topic,
# worth retrying. Any HTTP answer is a success event carrying its status.
[[handlers]]
name = "submit-invoice"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "invoice.extracted", "--publish-as", "invoice.submitted",
        "--error-as", "invoice.submit-failed", "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); \
     curl -s -o /dev/null -w '{"status": %{http_code}}' -X POST -H 'Content-Type: application/json' \
          --data-binary @- http://127.0.0.1:8700/invoices <<< "$p" \
     | jq -c --argjson orig "$p" '$orig + {status}' ''']
subscribes = ["invoice.extracted"]
publishes = ["invoice.submitted", "invoice.submit-failed"]

[[handlers]]
name = "route-filed"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "invoice.submitted", "--publish-as", "invoice.filed", "--",
        "jq", "-c", "select(.status >= 200 and .status < 300)"]
subscribes = ["invoice.submitted"]
publishes = ["invoice.filed"]

[[handlers]]
name = "route-refused"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "invoice.submitted", "--publish-as", "invoice.refused", "--",
        "jq", "-c", "select(.status >= 400 and .status < 500)"]
subscribes = ["invoice.submitted"]
publishes = ["invoice.refused"]

# The last arm is the negation of the others, so no status falls between them.
[[handlers]]
name = "route-unavailable"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "invoice.submitted", "--publish-as", "invoice.api-unavailable", "--",
        "jq", "-c", "select((.status >= 200 and .status < 500) | not)"]
subscribes = ["invoice.submitted"]
publishes = ["invoice.api-unavailable"]
```

`invoice.refused` goes to a person. `invoice.api-unavailable` and
`invoice.submit-failed` go to the retry policy below. Printing the status also
closes a silence: a 2xx with an empty body would otherwise be exit 0 with empty
stdout, which publishes nothing.

Which error topics need a name? Every handler that touches the world gets its
own `--error-as`, and behind a `stream-runner` that topic is routed to an event
that ends in the ack, because the world fails routinely. A pure `jq` router,
projection, or delay may stay on the default `exec.error`, provided one sink
subscribes to `exec.error`. A `jq` program that parses fails only on a payload
shape nobody expected, which is a bug to fix rather than a condition to route.
Behind a `stream-runner` that bug stalls the batch, and the `exec.error` sink is
where you will read why.

## Retry with backoff

Failure event, guard, delay, then an event the failed stage subscribes to. The
attempt count rides in the payload. Two exclusive routers on the failure event
decide between another try and giving up, and only then does anything wait.

```toml
[[handlers]]
name = "route-rescorable"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.score-failed", "--publish-as", "issue.score-retryable", "--",
        "jq", "-c", "select(.attempt < 3)"]
subscribes = ["issue.score-failed"]
publishes = ["issue.score-retryable"]

[[handlers]]
name = "escalate-unscorable"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.score-failed", "--publish-as", "issue.escalated", "--",
        "jq", "-c", "select(.attempt < 3 | not) | {number, attempt, reason: \"scoring failed\"}"]
subscribes = ["issue.score-failed"]
publishes = ["issue.escalated"]

# The delay. Its one act is the pause; the jq after it is the announcement.
[[handlers]]
name = "delay-rescore"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.score-retryable", "--publish-as", "issue.backoff-elapsed",
        "-t", "20000", "--max-concurrent", "8", "--", "bash", "-c",
  '''p=$(cat); sleep $((2 ** $(jq -r .attempt <<< "$p"))); \
     jq -c 'del(.error) | .attempt += 1' <<< "$p" ''']
subscribes = ["issue.score-retryable"]
publishes = ["issue.backoff-elapsed"]
```

`score-severity` adds `-s issue.backoff-elapsed` and the loop is closed.

Four details carry the weight. The guard runs before the delay, so an item that
is out of attempts escalates at once instead of sleeping its longest sleep first,
and the decision is a router you can read rather than a `select` hidden behind a
pause. The backoff reads the attempt from the payload, because nothing else
knows it. `-t` must exceed the longest sleep, or the delay itself times out and
publishes an error. And a sleeping handler holds its slot, so raise
`--max-concurrent` or one slow retry queues every other one behind it. Behind a
`stream-runner` with an end-of-flow ack only one item is in flight, so the
default of 1 is already enough there.

Retry only acts that are safe to repeat. A timeout does not mean the act did not
happen: a POST that landed and then timed out will land again. Send an
idempotency key the service dedupes on (the item's own identity works), or send
timeouts to a person instead of the retry loop.

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
  '''f=$(jq -r .path); \
     claude -p "Review $f against our conventions. Print exactly one JSON object \
       on stdout and nothing else, either {\"verdict\":\"approved\",\"path\":\"$f\"} \
       or {\"verdict\":\"needs_work\",\"path\":\"$f\",\"reasons\":[...]}. \
       Do not edit files, do not comment, do not act further." \
       --allowedTools Read Grep''']
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

The shell body is a TOML *literal* string (`'''`), and that matters. In a basic
`"""` string TOML decodes `\"` to a bare `"` before the shell sees it, which
closes the shell's own quote: the model receives `{verdict:approved,...}` with
the JSON quotes stripped, and a path containing a space splits into extra
arguments. In a literal string the shell receives exactly what is written.

Adding a third outcome (`review.blocked`, say) is one more router and one more
line in the prompt. Nothing else in the topology moves.

Four properties make this the default:

- **The trace survives the model call.** `exec-handler` inherits the inbound
  `correlation_id` and stamps `causation_id` from the triggering message, so the
  verdict is causally linked to the file change that provoked it. Shape B
  severs both.
- **Failure is already an event.** Any non-zero exit, a timeout, or a spawn
  failure publishes the error type (`-e`, default `exec.error`). Route it like
  anything else.
- **Malformed output is still an event.** `exec-handler` parses stdout as JSON
  and falls back to `{"output": "..."}`, so an agent that ignores the format and
  narrates lands at `route-invalid` rather than vanishing.
- **No listening port**, so there is no actuator to defend and no untrusted
  network input.

The one hole: a clean exit with empty stdout publishes nothing. If that is
plausible for your agent, add the reaper below.

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
  '''f=$(jq -r .path); r="$EMERGENT_CORRELATION_ID"; \
     claude -p "Review $f against our conventions. POST one verdict per issue \
       you find to localhost:8091 with curl, as JSON: \
       {\"verdict\":\"needs_work\",\"path\":\"$f\",\"run\":\"$r\",\"reason\":\"...\"}. \
       If the file is clean, POST exactly one \
       {\"verdict\":\"approved\",\"path\":\"$f\",\"run\":\"$r\"} instead. \
       Do not edit files, do not comment, do not act further." \
       --allowedTools Bash Read''']
subscribes = ["file.changed"]

# The pending marker is its own act, so it is its own primitive. Both sinks see
# the same event; neither waits for the other.
[[sinks]]
name = "mark-pending"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "file.changed", "--", "bash", "-c",
  '''touch "/tmp/pending-review-$(basename "$(jq -r .path)")"''']
subscribes = ["file.changed"]

[[sinks]]
name = "clear-pending"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "review.approved", "-s", "review.needs-work", "--", "bash", "-c",
  '''rm -f "/tmp/pending-review-$(basename "$(jq -r .path)")"''']
subscribes = ["review.approved", "review.needs-work"]
```

`EMERGENT_CORRELATION_ID` is set only when the inbound message carries a
correlation id, which requires an upstream `exec-source --correlate` (or
`--correlation-id`). Otherwise the variable is unset and `$r` is empty.

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
# One execution, at most one stdout event, so the sweep prints ONE JSON value
# holding every stale marker.
[[sources]]
name = "review-reaper"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--interval", "60000", "--shell", "sh", "--command",
  '''find /tmp -maxdepth 1 -name 'pending-review-*' -mmin +5 | jq -R -s -c '{files: (split("\n") | map(select(length > 0) | sub("^.*/pending-review-"; "")))}' ''']
publishes = ["reviews.swept", "reviews.sweep-failed", "reviews.sweep-done"]

[[handlers]]
name = "raise-stalled"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "reviews.swept", "--publish-as", "reviews.stalled", "--",
        "jq", "-c", ".stdout | fromjson | select(.files | length > 0)"]
subscribes = ["reviews.swept"]
publishes = ["reviews.stalled"]
```

`reviews.stalled` carries every stale file from one sweep. Put a `stream-runner`
behind it when each needs its own response.

`reviews.stalled` is then just another event: retry it, escalate it, or route it
to a human. The non-determinism is contained because its failure mode has a
name.

### What breaks across the re-entry boundary

Everything here is specific to Shape B except the last row, which applies to any
model call. This table is the argument for defaulting to Shape A: it collapses
to one row there.

| Concern | What happens | What to do |
|---|---|---|
| Causation | `http-source` sets no `causation_id`. The chain severs. | Accept it, or write a small SDK source that stamps it from the body. |
| Correlation | `http-source` sets no `correlation_id` either. | Have the agent echo `EMERGENT_CORRELATION_ID` into the body and use it as the trace key downstream. The sink gets it in env only when the inbound message carries one, so the run must start from an `exec-source --correlate`. |
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

**After engine 0.10.10 a terminal wildcard routes.** `system.error.*` reaches
every primitive's failure event, including primitives added later. On 0.10.10
and earlier it was accepted and delivered nothing, so name each type when the
topology has to run on an older engine:

```toml
[[handlers]]
name = "watchdog"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "system.error.score-severity", "-s", "system.error.poll-issues",
        "--publish-as", "triage.degraded", "--", "jq", "-c", "{name, error}"]
subscribes = ["system.error.score-severity", "system.error.poll-issues"]
publishes = ["triage.degraded"]
```

This makes the system reflexive: the topology can respond to its own health the
same way it responds to domain data. Remember what the engine does not do by
default. It restarts a primitive only when that primitive's block sets `restart`
(after 0.10.10), so without one `triage.degraded` is a page, not a self-heal.
With one, subscribe to `system.restarted.<name>` as well, and treat
`"Restarts exhausted"` on `system.error.<name>` as the page.

## Self-seeding

A topology that starts itself, rather than waiting for a manual poke. Subscribe
the seeding step to `system.started.<name>` of whatever must be ready first.

```toml
[[sinks]]
name = "seeder"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "system.started.webhook", "--",
        "curl", "-s", "--retry", "5", "--retry-connrefused", "--retry-delay", "1",
        "-X", "POST", "-H", "Content-Type: application/json",
        "-d", '{"count": 0}', "http://127.0.0.1:8088"]
subscribes = ["system.started.webhook"]
```

`system.started.<name>` fires when the engine has *spawned* the process, not
when the process is ready, so the port may not be bound yet. `--retry-connrefused`
covers the gap.

Combined with a feedback edge, this is the ouroboros: the loop seeds itself on
startup and then sustains itself. Startup order (Sinks, then Handlers, then
Sources) spawns the consumers before anything is published, with a fixed 50 ms
pause between primitives and no readiness handshake. A slow-starting consumer
can therefore miss a source's very first event.

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
args = ["--host", "127.0.0.1", "--port", "8099", "--path", "/inject"]
publishes = ["http.request"]

[[handlers]]
name = "inject-issue"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "http.request", "--publish-as", "issue.found", "--", "jq", "-c", ".body"]
subscribes = ["http.request"]
publishes = ["issue.found"]
```

`--host` defaults to `0.0.0.0`. An injection endpoint is an actuator, so bind it
to loopback. The payload is `{method, path, headers, body, remote_addr}`; the
handler above unwraps `.body` into whatever type you want to simulate.

This is also how you replay. The engine has no replay command, so replaying an
event means reading its `payload_json` from the event store and POSTing it here.
The replayed event gets new ids and no causation link to the original.
A topology you can poke is a topology you can debug.
