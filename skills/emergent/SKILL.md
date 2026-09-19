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
| **Recoverability** | The event is a resume point. Retry becomes a subscriber on the failure event, and replay becomes reading a payload out of the event store and POSTing it back in. |
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
free. What is not free is having many *items* in flight at once, and two things
cap it. `stream-runner`, the splitter, holds each item until the previous is
acked. And every `exec-handler` and `exec-sink` runs one command at a time, in
arrival order, unless you raise `--max-concurrent`. Both defaults are
deliberate: they buy bounded in-flight work, ordering, and natural backpressure.

So decide the two separately. Breadth per item is always fan-out. Items in
parallel is a choice, and if you want it you split with HTTP fan-out or an SDK
splitter instead, and raise `--max-concurrent` on each per-item handler. What you must not do is respond to "I want parallelism" by
removing the splitter, which leaves you with no way to turn a collection into
events at all.

## Inside a primitive: compose commands, never write programs

The stopping rules say how small a primitive must be. This says what it may be
made of. Take the first rung that expresses the step, and justify every step
down out loud.

1. **A marketplace primitive used as itself.** `stream-runner` to split and
   pace, `http-source` to receive, `jev-handler` to judge, `sse-sink` to push.
   Flags only, no command. Someone already wrote, tested, and documented this
   behavior, and its events are already in the shape everything else expects.
2. **An exec primitive around one existing command, as an args array.**
   `"--", "jq", "-c", "select(.confidence >= 0.8)"` or
   `"--", "gh", "issue", "edit", ...`. No shell. This is where most of a good
   topology lives: routers, projections, unwraps, guards.
3. **An exec primitive around one shell pipe of the form `shape | act | shape`.**
   Exactly one command in the pipe touches the world (an API, a model, a file, a
   queue). Everything else is pure `jq`. The shell is there because a pipe needs
   one, not because there is logic to hold.
4. **An SDK primitive**, only for the three cases in "When a custom primitive
   really is right" at the end of this document.

There is no rung for a script file. A `score.sh` or `triage.py` referenced from
`emergent.toml` is a program the topology cannot see into: its steps publish
nothing, its branches route nothing, and the next requirement means editing it
instead of adding a subscriber. When a script already exists, do not wire it in.
Read it, list the acts it performs, and give each act its own primitive.

A command has become a program when any of these is true. Each tell names its
own split:

| Tell | Split |
|---|---|
| Two commands that each touch the world, joined by `;`, `&&`, or a pipe | Two primitives with an event between them, so the first act is logged and the second is retryable alone |
| `if`, `case`, `[ ... ] &&`, or a ternary that picks what happens next | Two routers with exclusive `jq select()` predicates |
| `for`, `while`, `xargs` over items | `stream-runner`, or a splitter whose loop body only publishes |
| A variable that outlives one pipe, a temp file, a lock | An event carrying that state, or one small accumulator primitive |
| A retry loop or a `sleep` between attempts at the same act | The error topic, a delay handler, and a guard. See `references/patterns.md` |
| It would read better with a comment explaining its phases | It has phases. Each phase is a primitive |

**One shell idiom is legitimate and worth recognizing.** `exec-handler` replaces
the payload with the command's stdout, so an act whose output does not carry the
item's identity (a model reply, a search result) would orphan it. Capturing the
payload and merging it back is carry-through, not logic:

```
p=$(cat); jq -r .prompt <<< "$p" | claude -p | jq -c --argjson orig "$p" '. + {number: $orig.number}'
```

That is still rung 3: one capture, one act, one merge. The moment a second act
appears between the capture and the merge, split it.

**Write shell bodies as TOML literal strings** (`'''...'''`), never basic ones
(`"""..."""`). In a basic string TOML decodes `\"` to a bare `"` and `\\n` to
`\n` before the shell sees anything, so the JSON template in your prompt loses
its quotes and a path with a space splits in two. In a literal string the shell
receives exactly what you typed. A jq program with no single quote in it can use
a one-line literal too: `'select(.verdict == "approved")'`.

**Know the one silence.** `exec-handler` publishes nothing only when the command
exits 0 with empty stdout, which is what a false `jq select()` does. Every
non-zero exit publishes the error type. So `[ cond ] && cmd` is not a filter:
its false case exits 1 and lands on `exec.error`. If a non-zero exit really is a
normal outcome, declare it with `--silent-exit-codes`.

## Translating code constructs into topology

Decomposition feels like a leap of faith only until you know the mechanical
substitutions. Reach for these; they are the idiom.

| You would normally write | Build this instead |
|---|---|
| `for item in items:` | Publish one event per item. Note that no exec primitive splits its stdout: printing N objects publishes **one** event, not N. Use `stream-runner`, HTTP fan-out, or a small SDK splitter. See below. |
| `if x: A else: B` | Two subscribers on the same event with mutually exclusive `jq select()` predicates. Nothing decides between them; both look, one matches. |
| `try/except` | Publish `<domain>.failed` with the error in the payload. A separate subscriber owns the response. Failure is data, not control flow. |
| retry with backoff | `*.failed` to a delay handler to a republish of the original event. The retry count rides in the payload and a guard predicate ends it. |
| `parallel_map(f, items)` | Publish once, let N handlers subscribe, each publishing its own result type. The engine is the scheduler. |
| `gather()` / join | An accumulator that publishes its bucket on every arrival, and a router whose predicate recognizes a full one. See the join below. |
| a state variable | An event carrying the new state. If something must accumulate, one small stateful handler owns that accumulator and publishes every change. |
| a loop counter / guard | A depth field in the payload plus a guard handler whose predicate drops or diverts past the limit. |
| `sleep()` for pacing | Ack-driven `stream-runner`, or an interval `exec-source`. Time is an event source, not a blocking call. |
| a function call | Publish an event; subscribe to the response type. |
| a config flag switching behavior | Two primitives, one enabled. Or two subscribers with predicates on the flag in the payload. |
| a judgment call no rule can express | An `exec-handler` handing the decision to a model, publishing one verdict event, with routers turning it into the vocabulary. See non-deterministic routing below. |
| a yes/no, pick-one, or rubric judgment at volume | `jev-handler` asking typed questions and publishing one calibrated verdict, with routers turning confidence bands into the vocabulary. |

### One execution, one event

The single most common way a well-designed topology fails on first run: **no
exec primitive splits its stdout.** `exec-source` puts all stdout into one
string and publishes at most one stdout event per execution (plus a stderr event
when there is stderr, and always an exit event). `exec-handler` publishes at
most one event: it parses stdout as one JSON value and wraps it as
`{"output": "..."}` when that fails. Printing ten JSON objects publishes one
event containing ten stringified objects, not ten events.

So `jq -c '.[] | ...'` and `split("\n") | .[] | fromjson` look exactly like
splitting and are not. To get N events, something must publish N times:

- **`stream-runner`** is the splitter. It is the marketplace primitive built for
  exactly this: hand it one JSON array, it emits one item at a time. Reach for
  it first. Put `unwrap_stdout = true` on it and it reads the array straight out
  of an `exec-source` with no shaping handler between them.
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

### The join

A join is an accumulator, a router, and a sink, and keeping them apart is what
stops it becoming a script. The accumulator publishes its whole bucket on every
arrival. The completeness test is a predicate in the config, where you can read
it, rather than an `if` in a shell, where you cannot.

```toml
[[handlers]]
name = "collect-shards"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "shard.done", "--publish-as", "shards.collected", "--", "bash", "-c",
  '''p=$(cat); f="/tmp/join-$(jq -r .job_id <<< "$p").jsonl"; \
     jq -c . <<< "$p" >> "$f"; \
     jq -s -c '{job_id: .[0].job_id, results: .}' "$f" ''']
subscribes = ["shard.done"]
publishes = ["shards.collected"]

[[handlers]]
name = "route-complete"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "shards.collected", "--publish-as", "shards.complete", "--",
        "jq", "-c", "select(.results | length == 3)"]
subscribes = ["shards.collected"]
publishes = ["shards.complete"]

[[sinks]]
name = "clear-joined"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "shards.complete", "--", "bash", "-c",
  '''rm -f "/tmp/join-$(jq -r .job_id).jsonl"''']
subscribes = ["shards.complete"]
```

The accumulator is the one place a file of state is legitimate, because
accumulating is its whole job and it publishes every change. It needs no lock
while `--max-concurrent` stays at its default of 1.

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
  '''f=$(jq -r .path); \
     claude -p "Review $f against our conventions. Print exactly one JSON object \
       on stdout and nothing else, either {\"verdict\":\"approved\",\"path\":\"$f\"} \
       or {\"verdict\":\"needs_work\",\"path\":\"$f\",\"reasons\":[...]}. \
       Do not edit files, do not comment, take no other action." \
       --allowedTools Read Grep''']
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
- **Failure is already an event.** Any non-zero exit, a timeout, or a failure to
  spawn publishes the error type, carrying the original payload plus an `error`
  key. You can route it like anything else.
- **Malformed output is still an event.** Stdout that is not JSON becomes
  `{"output": "..."}`, so an agent that ignores the format and narrates ends up
  at `route-invalid` rather than vanishing.
- **No open port**, so no listening actuator to defend.

One silence remains: a clean exit with empty stdout publishes nothing. If that
is plausible for your agent, add the reaper from `references/patterns.md`.

### The fast judge: `jev-handler` for typed questions

When the judgment fits a typed question (a yes/no, one option from a known set,
a position on a rubric), the marketplace `jev-handler` is the same node at a
fraction of the latency and cost: one sub-second API call per message, answers
carrying calibrated confidence, errors already an event. It obeys the same rule
as every judge here. It publishes one verdict type and never chooses a flow, so
"auto-file above 0.9, send the rest to review" is two exclusive routers on
`.answers.<id>.confidence`, not a flag on the primitive. A threshold change is a
config edit, and the raw verdicts stay reusable by subscribers nobody has
designed yet.

Keep the agent judge for decisions that need reading files, using tools, or
explaining themselves. A common shape uses both: `jev-handler` settles the
confident majority, and its low-confidence band is the event an agent judge
subscribes to. `references/primitives.md` has the flags, the questions file, the
payload shapes, and the model's limits.

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
  `EMERGENT_CORRELATION_ID` into the body and trace on that. The variable is set
  only when the inbound message carries a correlation id, so the run has to
  start from an `exec-source --correlate`.
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

**The Script File.** A path to your own script in `args`, or a `bash -c` that
needs scrolling. *Tell:* `./something.sh`, `something.py`, or a command with a
loop, a conditional, or two world-touching calls in it. It is the most common
way a decomposed-looking topology hides a monolith, because the TOML has many
blocks and each one looks small. Count acts, not blocks.

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
`http-source`, or read one out of the event store and POST it back) and get
sensible behavior? If a
primitive only works when its predecessor just ran, they are coupled through
hidden state.

**The concurrency test.** If ten items arrive at once, do ten flow through
independently, or does something serialize them? Trace one item's path and name
every point where it could be waiting on a different item: a `stream-runner`
ack, and every exec primitive left at `--max-concurrent 1`. Each one needs a
reason you can state (a rate limit, ordering, an accumulator, a poller that must
not double-process), because otherwise you have capped throughput at one without
deciding to.

**The dead-end test.** For every event, including every `--error-as` topic,
name its subscriber. For every router group, show that some arm matches any
payload. An event nobody consumes and a payload no arm matches both vanish
silently, and behind a `stream-runner` either one stalls the batch for good.
Subscriptions are exact-match, so `"system.error.*"` and `"issue.*"` subscribe
to nothing: list each type.

**The script test.** List every primitive whose command is not a bare args
array. For each, count the commands that touch the world. More than one fails.
Any loop or conditional fails. Any file of your own on disk fails. For each
failure, name the event that belongs between the two halves and split there.

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

**The decomposition.** `poll-issues` lists unlabeled issues as one JSON array and
`split-issues`, a `stream-runner`, publishes `issue.found` once per issue (loop
becomes a splitter). `score-severity` and `detect-duplicates` both subscribe to
`issue.found` and run concurrently, publishing `issue.scored` and
`issue.dupe-checked` (fan-out is the concurrency). Three routers subscribe to
`issue.scored` with exclusive predicates: confident publishes `issue.triaged`,
not confident with attempts left publishes `issue.uncertain`, not confident with
none left publishes `issue.escalated` (branch becomes topology, and the third
arm is the depth guard). `enrich-context` subscribes to `issue.uncertain`, adds
recent comments, and publishes `issue.enriched` with `attempt: n+1`, which
`score-severity` also subscribes to (the feedback edge). `label-issue` consumes
`issue.triaged`, a Slack sink consumes `issue.escalated`, and `settle` turns
either one into the ack that releases the next issue.

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
