---
name: emergent
description: >
  Design and build systems on the Emergent event-driven workflow engine the
  idiomatic way, where behavior lives in the topology rather than inside
  primitives. Use this skill whenever Emergent, emergent.toml, Sources,
  Handlers, or Sinks come up; whenever the user wants a pipeline, workflow,
  automation, agent loop, monitor, or orchestration that will run on Emergent;
  and before writing or editing any emergent.toml or SDK primitive. Use it
  especially when the task sounds like "just write a script for this step" or
  "wire my existing tool into Emergent", because that framing is what produces
  a monolith wearing pub-sub as a costume. Also use when reviewing or
  refactoring an existing topology to judge whether it is genuinely
  event-driven.
---

# Emergent: Designing for Emergence

## What you are actually building

Emergent is not a task runner, not a DAG executor, and not a wrapper around your
script. It is a message fabric where independent processes react to events. The
engine routes, stores, and supervises. It never sequences.

That leads to the one idea this whole skill exists to protect:

> **Behavior lives in the topology, not in the primitives.**
> A primitive is a reflex. The system's intelligence is in what reacts to what.

Here is the test that matters. If your design would still work after you delete
the engine and replace it with a shell pipeline or a cron job, you have not
built an Emergent system. You have built a script with extra steps and paid for
an IPC layer you did not use.

And take the name literally. The goal is a system that does things you did not
write: escalation depth nobody coded, backpressure nobody scheduled, recovery
paths nobody enumerated, all falling out of small primitives reacting locally
plus feedback closing the loop. Conway's Game of Life is the reference model,
not a metaphor. Four rules and a seed produce gliders. Nobody implemented a
glider. Three big sequential steps, however cleanly wired, cannot surprise you
and never will.

## Why decomposition pays (read this before you argue with it)

You will feel pressure to merge steps. It always looks cheaper: one script is
testable standalone, has no event names to invent, and no routing to trust. That
instinct is why almost every Emergent system built by an agent comes out wrong.

The decisive argument is about what you are leaving behind. **A decomposed
topology is not just this workflow, it is a substrate.** Every event you publish
and every handler you write becomes available to behaviors nobody has designed
yet, and a new behavior is then a new subscriber on events that already exist.
Merge those steps and the next requirement means decomposing a script under
pressure, with state and control flow already fused together, which is the
expensive version of this work. A primitive costs a three-line TOML block. That
asymmetry is the whole reason to over-invest in boundaries early.

On top of that, every event boundary hands you four things at no cost:

| Boundary gives you | Because |
|---|---|
| **Observability** | The event store records it. You can read what happened and why, after the fact, without instrumenting anything. |
| **Extensibility** | Anyone can subscribe. New capability means a new primitive and zero edits to existing ones. |
| **Recoverability** | The event is a resume point. Retry, replay, and redrive all become possible. |
| **Substitutability** | Either side can be swapped, in any language, without the other noticing. |

Every boundary you skip forfeits all four permanently, for everything inside
that primitive. A 2-hour step that publishes one event at the end is a 2-hour
hole in your system where nothing is observable, nothing is extensible, nothing
resumes, and nothing can be swapped.

That is the trade. Merging is not "simpler," it is buying a little authoring
convenience with all four properties at once.

## Gate 1: the event catalog, before you write any file

Do not open an editor yet. Emit the catalog first, in the conversation, and get
it right. Naming events first is what makes a monolith impossible to write,
because a monolith has only one event and the emptiness of its catalog is
immediately obvious.

For every event, state four things:

```
<domain>.<action>
  fired when:  what became true in the world at this instant
  payload:     the shape, concretely
  published by: which primitive(s)
  consumed by:  which primitive(s)
```

The "fired when" line is the load-bearing one. It must describe a state change
in the world, in one sentence, with no "and". If you cannot write that sentence,
you do not have an event, you have a function call you were about to disguise as
one.

**Name events for what became true, never for what happens next.**
`issue.scored` describes the world and any number of future subscribers can use
it. `issue.ready-for-labeling` describes its consumer, and the moment a second
subscriber wants it for something else the name is a lie it has to work around.
Consumer-coupled names are how a large topology quietly stops being reusable,
which forfeits the main reason you decomposed it. If a name contains a
downstream primitive, a destination, or the word "for", rename it after the
state change instead.

**Reuse before you add.** Check the events you already have before inventing a
primitive. If something upstream already publishes what you need, subscribe to
it. New behavior arriving as a new subscriber on existing events is the system
working as intended, and it is the cheapest change you can possibly make.

Then sketch the topology and label three things explicitly:

- **Feedback edges** (an event re-entering an earlier stage). A topology with no
  cycle is a pipeline, and a pipeline cannot exhibit emergence. If you have
  none, say so and justify it.
- **Fan-out points** (one event, several independent subscribers).
- **Join points** (several events converging, and how they are matched, which is
  almost always `correlation_id`).

A cheap sanity check before you move on: count the verbs in the requirement.
"Poll the repo, triage each issue, tag it, and notify the team" has four. If
your catalog has fewer events than the requirement has verbs, you have already
merged something.

Present the catalog, then write config.

## The stopping rules: when is a primitive still too big?

Split until all five hold. They overlap on purpose, because each one catches a
different way of hiding work.

**1. One observable state change.** If you would ever want to see it in the log,
alert on it, or react to it, it is an event boundary. A primitive that changes
the world twice is two primitives.

**2. No branching, no sequencing.** A primitive containing an `if`/`case` that
decides what happens next has stolen routing from the topology. A primitive that
does A then B has stolen sequencing. Both belong outside. Routing becomes N
subscribers with mutually exclusive predicates; sequencing becomes two
primitives and an event between them.

**3. One irreducible I/O act.** One API call, one transform, one write. The unit
is the smallest thing that could fail on its own, because it is also the
smallest thing you could retry on its own.

**4. Independently resumable.** If you would want to retry, resume, or watch
progress on part of a step, that part is its own primitive. Long duration is the
loudest tell here: a multi-minute timeout on an exec-handler is nearly always a
merged step, and a multi-hour one always is.

**5. Concurrency comes from the topology, never from inside.** This is the rule
agents miss most. To do N things at once you publish one event and let N
handlers subscribe to it. You do not write a script with `&`, `xargs -P`,
`asyncio.gather`, or a thread pool. Internal parallelism is invisible to the
engine, unsupervised, unobservable, and unextendable. Fan-out is how Emergent
expresses concurrency, and fan-in is how it expresses the join.

The corollary is about *where* the concurrency lives. Once an item is in flight,
every handler subscribing to its event runs concurrently, and that breadth is
free. What is not free is having many *items* in flight at once:
`stream-runner`, the splitter, holds each item until the previous is acked. That
is deliberate, and it buys bounded in-flight work and natural backpressure.

So decide the two separately. Breadth per item is always fan-out. Items in
parallel is a choice, and if you want it you split with HTTP fan-out or an SDK
splitter instead. What you must not do is respond to "I want parallelism" by
removing the splitter, which leaves you with no way to turn a collection into
events at all.

## Translating code constructs into topology

Decomposition feels like a leap of faith only until you know the mechanical
substitutions. Reach for these; they are the idiom.

| You would normally write | Build this instead |
|---|---|
| `for item in items:` | Publish one event per item. Note that exec primitives emit exactly **one event per execution**, so printing N objects does not fan out. Use HTTP fan-out, `stream-runner`, or a small SDK splitter. See below. |
| `if x: A else: B` | Two subscribers on the same event with mutually exclusive `jq select()` predicates. Nothing decides between them; both look, one matches. |
| `try/except` | Publish `<domain>.failed` with the error in the payload. A separate subscriber owns the response. Failure is data, not control flow. |
| retry with backoff | `*.failed` to a delay handler to a republish of the original event. The retry count rides in the payload and a guard predicate ends it. |
| `parallel_map(f, items)` | Publish once, let N handlers subscribe, each publishing its own result type. The engine is the scheduler. |
| `gather()` / join | Accumulate by `correlation_id` until the expected count arrives, then publish. See the zero-code join below. |
| a state variable | An event carrying the new state. If something must accumulate, one small stateful handler owns that accumulator and publishes every change. |
| a loop counter / guard | A depth field in the payload plus a guard handler whose predicate drops or diverts past the limit. |
| `sleep()` for pacing | Ack-driven `stream-runner`, or an interval `exec-source`. Time is an event source, not a blocking call. |
| a function call | Publish an event; subscribe to the response type. |
| a config flag switching behavior | Two primitives, one enabled. Or two subscribers with predicates on the flag in the payload. |
| a judgment call no rule can express | An `exec-handler` handing the decision to a model, publishing one verdict event, with routers turning it into the vocabulary. See non-deterministic routing below. |

### One execution, one event

The single most common way a well-designed topology fails on first run: **every
exec primitive publishes exactly one event per execution.** `exec-source` puts
all stdout into one string. `exec-handler` parses stdout as one JSON value and
wraps it as `{"output": "..."}` when that fails. Printing ten JSON objects
publishes one event containing ten stringified objects, not ten events.

So `jq -c '.[] | ...'` and `split("\n") | .[] | fromjson` look exactly like
splitting and are not. To get N events, something must publish N times:

- **`stream-runner`** is the splitter. It is the marketplace primitive built for
  exactly this: hand it a collection, it emits one item at a time. Reach for it
  first.
- **HTTP fan-out**: a shell step POSTs once per item to an `http-source`. Use
  when you need the items themselves in flight simultaneously, since
  `stream-runner` holds each until the previous is acked.
- **A ten-line SDK splitter** using `publish_all`, when you want N events with
  causation intact and no ack protocol.

Keep two ideas separate here, because conflating them produces incoherent
designs. **Splitting** turns one collection into N events of one type.
**Fan-out** delivers one event to N subscribers doing different work. They
compose: `stream-runner` paces the items, and each item then fans out across
handlers. Concurrency across *handlers* is free either way.

`references/patterns.md`, "Turning a collection into events", has all three.

### The zero-code join

`exec-handler` publishes nothing when the command exits 0 with empty stdout. That
one fact gives you a join with no custom code: each arrival appends to a
per-key file and stays silent until the last one lands.

```toml
[[handlers]]
name = "joiner"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "shard.done", "--publish-as", "shards.complete", "--", "sh", "-c",
  """p=$(cat); k=$(jq -r .job_id <<< "$p"); f=/tmp/join-$k.jsonl; \
     echo "$p" >> $f; \
     [ $(wc -l < $f) -ge 3 ] && jq -s -c --arg k "$k" '{job_id: $k, results: .}' $f && rm -f $f"""]
subscribes = ["shard.done"]
publishes = ["shards.complete"]
```

**Choose the join key deliberately.** `exec-source --correlate` mints one ID per
source *process* and stamps it on everything that process publishes, and every
`exec-handler` propagates it forward. So `correlation_id` identifies **a run,
not an item**. That makes it the right key for "show me everything from this
run" queries against the event store, and the wrong key for joining per-item
work out of an interval poller, where every item of every poll would share one
ID and collide. Join on a natural key from the payload instead: an invoice path,
an issue number, a job id.

## Non-deterministic routing: letting a model choose the next event

Everything above routes deterministically: a predicate looks at a value and the
matching subscriber fires. The most interesting Emergent systems add a second
kind of node, where **a model decides which event happens next**.

The move is to give an agent an action space made of your event vocabulary, so
its judgment lands on the bus as an ordinary event. Nothing downstream can tell
a model was involved, which is the point: the non-deterministic node stays
swappable for a rule later without disturbing anything around it.

Keep two steps separate, because only the first has a choice to make.

1. **A judgment is produced**, as one event of one type.
2. **The judgment becomes a transition**, by exclusive routers turning that one
   type into the domain vocabulary.

Step 2 is the same regardless of how step 1 is built, and it is not decoration.
A judging primitive publishes one fixed type, so on its own it can report a
verdict but never choose between flows. The routers are what make a judgment a
transition.

### The default: an `exec-handler` judges

The agent prints one JSON verdict on stdout and `--publish-as` makes that an
event.

```toml
# The non-deterministic node. Its verdict is not in the config, and not
# knowable from it.
[[handlers]]
name = "judge-file"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "file.changed", "--publish-as", "review.submitted",
        "-e", "review.judge-failed", "-t", "180000", "--", "bash", "-c",
  """f=$(jq -r .path); \
     claude -p "Review $f against our conventions. Print exactly one JSON object \
       on stdout and nothing else, either {\"verdict\":\"approved\",\"path\":\"$f\"} \
       or {\"verdict\":\"needs_work\",\"path\":\"$f\",\"reasons\":[...]}. \
       Do not edit files, do not comment, take no other action." \
       --allowedTools Read Grep"""]
subscribes = ["file.changed"]
publishes = ["review.submitted", "review.judge-failed"]

# The vocabulary. Each outcome is a router; adding one is a new block.
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

Handlers on `review.approved` and `review.needs-work` take it from there, and
neither knows a model was involved. Adding a third outcome is one more router
and one more line in the prompt.

Four properties make this the default:

- **The trace survives the model call.** `exec-handler` inherits the inbound
  `correlation_id` and stamps `causation_id`, so the verdict is causally linked
  to the file change that provoked it.
- **Failure is already an event.** A crash with stderr, or a timeout, publishes
  the error type. You can route it like anything else.
- **Malformed output is still an event.** Stdout that is not JSON becomes
  `{"output": "..."}`, so an agent that ignores the format and narrates ends up
  at `route-invalid` rather than vanishing.
- **No open port**, so no listening actuator to defend.

One silence remains: a clean exit with empty stdout publishes nothing. If that
is plausible for your agent, add the reaper from `references/patterns.md`.

### The escalation: the agent re-enters through an `http-source`

An `exec-sink` dispatches and the agent POSTs its verdict to an `http-source`.
Its tool call is the publish. Reach for this when one execution producing
exactly one event is the thing in your way:

- the decision yields **zero, one, or many** events rather than exactly one
- the agent should emit **while** it works, not once at the end
- the decider is **out of band**: another machine, a queue, a human clicking a
  link

A single `http-source` on `127.0.0.1` receives everything and the routers above
still apply, reading `.body.verdict` instead of `.verdict`, because the POSTed
body arrives nested. What you give up:

- **The causal chain breaks.** `http-source` stamps no `correlation_id` and no
  `causation_id`, so the re-entering event is an orphan. Have the agent echo
  `EMERGENT_CORRELATION_ID` into the body and trace on that.
- **Silence is invisible.** A fire-and-forget sink cannot tell you the agent
  emitted nothing, so the pending marker and reaper are mandatory here rather
  than optional.
- **The endpoint is an actuator.** Bind to `127.0.0.1` and consider `--secret`.
  Anything that can POST there can drive your system.

`references/patterns.md` covers both shapes in full, the reaper, the
one-port-per-outcome variant, and vocabulary validation.

This is where the architecture earns its name. The topology declares the space
of reachable trajectories; a non-deterministic node picks the path through it.
You get behavior you did not script, inside a space you did.

## Anti-patterns, and the tell for each

Name these when you see them, in your own drafts and in code you are reviewing.

**The Wrapper.** One source, one handler that is the entire program, one sink.
*Tell:* deleting the engine loses nothing but supervision.

**The Conductor.** A primitive that knows the whole sequence and drives it.
*Tell:* its name is `runner`, `pipeline`, `controller`, `coordinator`, or
`orchestrator`, or its script contains the list of phases. This is the most
emergence-hostile pattern there is, because a system with a conductor can only
ever do what the conductor knows.

**The LLM Conductor.** An agent prompted to "review this and take whatever
action is needed," holding the whole workflow inside one model call. *Tell:* the
prompt describes outcomes rather than an event vocabulary, or the agent is given
tools that let it act on the world instead of announce a decision. This is The
Conductor with a model inside it, and it is worse than the shell-script version
because the sequence is now non-deterministic as well as hidden. The discipline
that keeps non-deterministic routing honest: **the agent announces its decision
as an event and stops; handlers own the consequences.**

**The Black Box.** A step with a long timeout that publishes exactly one event
when it finishes. *Tell:* `-t 900000` or larger. Nothing can observe or react to
anything happening inside it.

**The Hidden Branch.** Routing inside a script. *Tell:* `case`, `if`, or a
ternary that selects the next action rather than computing a value.

**The Private State.** State in a script variable, a local file, or a long-lived
process that no other primitive can see. *Tell:* nothing in the event log would
tell you the current state. If state is not in the stream, nothing can react to
it, and reaction is the entire mechanism of emergence.

**The Internal Loop.** A `for` over a collection inside a primitive. *Tell:* one
event in, one event out, but N units of work happened. Those N units were
invisible, unretryable, and unparallelizable. The distinction that matters is
work versus emission: a loop that *processes* items is the anti-pattern, while a
loop whose only body is publishing one event per item is the fan-out mechanism
and exactly what you want.

**The Internal Parallelism.** `&`, `xargs -P`, `gather`, threads, rayon. *Tell:*
concurrency that the topology cannot see. (Legitimate exception: parallelism
inside a genuinely atomic computation, like a numerical kernel that computes one
frame. The frame is still one event.)

**The Silent Step.** Real work that publishes nothing because it "just worked."
*Tell:* a gap in the log where you cannot tell whether something ran.

**The Straight Line.** No feedback edge anywhere. *Tell:* every event flows
strictly forward. Sometimes correct for pure ETL, but interrogate it, because
without a cycle the system can only transform, never adapt.

## Gate 2: review the draft before you ship it

Run these against your own topology and write down the answers. They are
falsifiable on purpose. If any fails, revise before presenting.

**The log test.** Reading only the event log, could you reconstruct what the
system did and why? Any span where nothing is published is a black box.

**The subscriber test.** Invent a plausible new requirement ("also notify Slack
on high severity", "also record timing"). Can you satisfy it by adding one
primitive and editing zero existing ones? If you would have to open a script,
that behavior is trapped inside it.

**The injection test.** Can you hand-inject any event mid-topology (POST to an
`http-source`, or replay one from the log) and get sensible behavior? If a
primitive only works when its predecessor just ran, they are coupled through
hidden state.

**The concurrency test.** If ten items arrive at once, do ten flow through
independently, or does something serialize them? Trace one item's path and name
every point where it could be waiting on a different item. Each one needs an
external constraint justifying it, because otherwise you have capped throughput
at one.

**The swap test.** Can you replace any single primitive's implementation, `jq`
for Python, `claude` for `ollama`, without touching its neighbors?

**The kill test.** Kill one primitive. Does the rest degrade sensibly, or does
everything stop? Local autonomy is what makes the system a system.

**The name test.** Can every primitive be named verb-noun with no "and"?
`extract-url`, `score-severity`, `post-comment` pass. `fetch-and-format`,
`process-request`, `handle-event` fail, and the name is telling you the truth
about the contents.

**The surprise test.** Where could this system do something you did not
explicitly write? If the honest answer is nowhere, you built a pipeline. That
may be acceptable for a pure transform, but say so out loud rather than letting
it happen by default.

## A brief conversion

Requirement: watch a repo for new issues, triage each with an LLM, label it, and
escalate anything the model is unsure about.

**The monolith an agent reaches for first.** One `exec-source` on a 60s interval
running `triage.py`, which polls the API, loops over issues, calls the LLM per
issue, parses confidence, decides whether to escalate, applies labels, and posts
to Slack. One event type, `exec.output`, carrying a summary nobody consumes.
Every stopping rule broken. Emergent contributed process supervision and nothing
else.

**The decomposition.** `poll-issues` publishes `issue.found` once per issue
(loop becomes fan-out). `score-severity` and `detect-duplicates` both subscribe
to `issue.found` and run concurrently, publishing `issue.scored` and
`issue.duplicate-checked` (fan-out is the concurrency). `label-issue` subscribes
to `issue.scored`. Two guards subscribe to `issue.scored` with opposite
predicates: `confidence >= 0.8` publishes `issue.triaged`, `confidence < 0.8`
publishes `issue.uncertain` (branch becomes topology). `enrich-context`
subscribes to `issue.uncertain`, adds related-issue context, and republishes
`issue.found` with `attempt: n+1` (the feedback edge). A `depth-guard` subscribes
to `issue.found` and diverts anything past `attempt: 3` to `issue.escalated`,
which a Slack sink consumes.

Now notice what nobody wrote: **how many times an issue re-triages**. That is a
consequence of confidence scores meeting a threshold meeting a depth guard.
Easy issues settle in one pass, ambiguous ones circulate and accumulate context
until they either resolve or escalate. That behavior is in the topology, not in
any primitive, and it is what the word emergent is pointing at.

`references/worked-example.md` develops this into full TOML.

## Reference material

Consult these as needed rather than reading them all up front.

| File | Read when |
|---|---|
| [references/primitives.md](./references/primitives.md) | Choosing a marketplace primitive or needing its exact CLI flags and payload shapes |
| [references/patterns.md](./references/patterns.md) | Implementing feedback loops, guards, joins, retries, streaming, or system-event triggers |
| [references/worked-example.md](./references/worked-example.md) | Wanting the full monolith-to-topology conversion in complete TOML |
| [references/configuration.md](./references/configuration.md) | Writing emergent.toml: fields, paths, secrets, event store |
| [references/architecture.md](./references/architecture.md) | Needing engine internals, IPC, startup and shutdown ordering |
| [references/sdk-api.md](./references/sdk-api.md) | Writing a custom primitive in Rust, Python, TypeScript, or Go |
| [references/templates.md](./references/templates.md) | Wanting copy-paste primitive implementations |
| [references/error-handling.md](./references/error-handling.md) | Writing Rust primitives, where clippy denies unwrap and expect |

## When a custom primitive really is right

Reaching for the SDK is not a failure. It is the right call when the work is one
irreducible act that exec primitives cannot express:

- **Genuine accumulator state** across messages (a simulation grid, a rolling
  window, a join with complex matching). Keep it to the accumulator alone and
  publish every state change so the rest of the system can still react.
- **A protocol bridge** the marketplace does not cover.
- **A compute kernel** where the parallelism is internal to one atomic result.

The discipline is unchanged: a custom primitive obeys the same five stopping
rules. "I need custom code" justifies one primitive, never a program.
