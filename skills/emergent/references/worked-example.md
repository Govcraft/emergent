# Worked Example: Monolith to Topology

One requirement, built twice. The first way is what an agent produces by
default. The second is the same behavior expressed as structure, and it does
something the first cannot.

**Requirement.** Watch a repository for new issues. Triage each one with an LLM.
Apply a label. Escalate anything the model is unsure about.

---

## Attempt 1: the monolith

```toml
[engine]
name = "triage"
socket_path = "auto"

[[sources]]
name = "triage"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "bash", "--command", "./triage.py", "--interval", "60000"]
publishes = ["exec.output"]

[[sinks]]
name = "log"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "exec.output", "--", "jq", "."]
subscribes = ["exec.output"]
```

`triage.py` polls the GitHub API, loops over new issues, calls the LLM for each,
parses a confidence score, decides whether to escalate, applies labels, posts to
Slack, and prints a summary.

This runs. It also fails every stopping rule at once:

| Rule | Violation |
|---|---|
| One observable state change | Roughly six: fetched, scored, labeled, deduped, escalated, notified. One event. |
| No branching, no sequencing | The escalation decision is an `if` inside the script; the whole flow is a sequence inside the script. |
| One irreducible I/O act | GitHub read, LLM call, GitHub write, Slack write. Four services, one primitive. |
| Independently resumable | An LLM failure on issue 7 of 20 loses the run. Nothing resumes. |
| Concurrency from the topology | The loop is serial, or worse, parallel inside the script where the engine cannot see it. |

And the anti-patterns by name: **The Wrapper** (delete the engine, lose only the
60-second timer), **The Internal Loop**, **The Hidden Branch**, **The Black Box**,
**The Private State** (the seen-issues set lives in the script).

The deepest problem is not any of those. It is that this system can only ever do
what `triage.py` does. Every new requirement is an edit to one file, every
behavior change risks the rest, and nothing can surprise you.

---

## Attempt 2: the topology

### Event catalog

```
issues.listed
  fired when:   a poll of the issue tracker returned at least one unlabeled issue
  payload:      {command, stdout, exit_code}, stdout holding one JSON array
  published by: poll-issues
  consumed by:  split-issues

issue.found
  fired when:   one unlabeled issue entered triage
  payload:      {number, title, body, attempt: 1, context: []}
  published by: split-issues
  consumed by:  score-severity, detect-duplicates

issue.scored
  fired when:   the model assigned a severity and a confidence
  payload:      {number, title, body, attempt, context[], severity, confidence, rationale}
  published by: score-severity
  consumed by:  route-confident, route-uncertain, depth-guard

issue.triaged
  fired when:   a score was confident and well-formed enough to act on
  payload:      same as issue.scored
  published by: route-confident
  consumed by:  label-issue, notify, settle

issue.uncertain
  fired when:   a score was too weak to act on and attempts remain
  payload:      same as issue.scored
  published by: route-uncertain
  consumed by:  enrich-context

issue.enriched
  fired when:   recent comments were added to an issue's context
  payload:      same as issue.scored, with context[] one longer and attempt + 1
  published by: enrich-context
  consumed by:  score-severity

issue.score-failed
  fired when:   the model call failed, timed out, or replied with something that is not a JSON object
  payload:      {...the issue, error: {exit_code, stderr, command}}
  published by: score-severity (error path)
  consumed by:  route-rescorable, escalate-unscorable

issue.score-retryable
  fired when:   a scoring failure was found to have attempts left
  payload:      same as issue.score-failed
  published by: route-rescorable
  consumed by:  delay-rescore

issue.backoff-elapsed
  fired when:   the pause after a retryable scoring failure ended
  payload:      the issue, with attempt + 1 and no error key
  published by: delay-rescore
  consumed by:  score-severity

issue.enrich-failed
  fired when:   the comments for an issue could not be fetched
  payload:      {...the issue, error: {exit_code, stderr, command}}
  published by: enrich-context (error path)
  consumed by:  escalate-unenrichable

issue.escalated
  fired when:   an issue left automated triage without settling
  payload:      {number, attempt, reason}
  published by: depth-guard, escalate-unscorable, escalate-unenrichable
  consumed by:  label-escalated, notify-human, settle

issue.dupe-checked
  fired when:   the issue was compared against the other open issues
  payload:      {number, duplicate_of|null}
  published by: detect-duplicates
  consumed by:  route-duplicate

issue.duplicate-found
  fired when:   an open issue with a matching title exists
  payload:      {number, duplicate_of}
  published by: route-duplicate
  consumed by:  label-duplicate

issue.settled
  fired when:   an issue finished triage, by either exit
  payload:      {number}
  published by: settle
  consumed by:  split-issues (the ack that releases the next issue)

issues.drained
  fired when:   every issue from one poll has settled
  payload:      {count}
  published by: split-issues
  consumed by:  nobody yet, on purpose (a digest or a metrics sink can take it later)
```

`issues.list-done`, the poller's exit event, is also unconsumed on purpose. No
error topic is: the three domain ones are routed, and `exec.error` and
`issues.list-failed` go to the `errors` sink.

Feedback edges: `issue.uncertain` to `enrich-context` to `issue.enriched`, back
into `score-severity`. `issue.score-failed` to `route-rescorable` to
`delay-rescore` to `issue.backoff-elapsed`, back into `score-severity`. `issue.settled` back into
`split-issues`, which is what paces the batch.
Fan-out: `issue.found` to `score-severity` + `detect-duplicates`. `issue.scored`
to three exclusive routers. `issue.triaged` to `label-issue` + `notify` + `settle`.
Fan-in: three publishers of `issue.escalated`, three event types into
`score-severity`. Join: none required. The duplicate check is a side branch that
nothing waits for.

Two names deserve a second look, because the first draft got them wrong. The
feedback edges used to republish `issue.found`. That name was a lie on the
second pass (nothing was found, context was added), and it had a cost: every
subscriber of `issue.found` ran again, so the duplicate check and its label
fired once per attempt. Naming the edges for what became true
(`issue.enriched`, `issue.backoff-elapsed`) fixed the behavior without touching
`detect-duplicates`.

### Topology

```
poll-issues ─> issues.listed ─> split-issues ─> issue.found ─┬─> detect-duplicates ─> issue.dupe-checked
                                    ▲                        │        └─> route-duplicate ─> issue.duplicate-found ─> label-duplicate
                                    │ (ack)                  ▼
                              issue.settled          score-severity <──────────────┬──────────────────────┐
                                    ▲                   │        │                  │                      │
                                  settle                │        └─> issue.score-failed ─┬─> route-rescorable ─> delay-rescore ─> issue.backoff-elapsed
                                    ▲                   ▼                                └─> escalate-unscorable ──┐
                                    │              issue.scored                                                    │
                                    │      ┌────────────┼──────────────┐                                           │
                                    │      ▼            ▼              ▼                                           │
                                    │ route-confident  route-uncertain  depth-guard ───────────────────────────────┤
                                    │      │            │                                                          │
                                    │      ▼            ▼                                                          ▼
                                    ├─ issue.triaged   issue.uncertain ─> enrich-context ─> issue.enriched   issue.escalated ─┬─> label-escalated
                                    │      ├─> label-issue                      └─> issue.enrich-failed            │          └─> notify-human
                                    │      └─> notify                               └─> escalate-unenrichable ─────┘
                                    └──────────────────────────────────────────────────────────────────────────────┘
```

### Config

Every primitive below sits on rung 1, 2, or 3 of the ladder in `SKILL.md`. There
is no script file, no loop, and no `if`. Shell bodies are TOML literal strings
(`'''`), so the shell receives exactly what is written: in a basic `"""` string
`\"` decodes to a bare quote before the shell ever sees it, which silently
unquotes your JSON and your paths.

```toml
[engine]
name = "triage"
socket_path = "auto"
api_port = 8891

# ---------------------------------------------------------------- ingress

# Rung 3: one act (`gh`), shaped by gh's own --jq. It prints ONE JSON array,
# because one execution publishes at most one stdout event.
#
# `no:label` makes GitHub the seen-set. Every exit from this topology applies a
# label, so a settled issue drops out of the next poll and no primitive holds
# private state.
#
# --correlate stamps one ID on everything this process publishes, which groups
# the whole poller in the event store. It is NOT a per-issue key. Per-issue
# identity is `number`, carried in the payload.
#
# `select(length > 0)` prints nothing for an empty list, and blank stdout
# publishes no stdout event, so an idle poll leaves only its exit event.
#
# The three topics come from `publishes` in order (stdout, stderr, exit).
[[sources]]
name = "poll-issues"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--correlate", "--interval", "300000", "--shell", "sh", "--command",
  '''gh issue list --state open --search no:label --limit 50 --json number,title,body --jq 'map(. + {attempt: 1, context: []}) | select(length > 0)' ''']
publishes = ["issues.listed", "issues.list-failed", "issues.list-done"]

# Rung 1: the marketplace splitter, used as itself. `unwrap_stdout` has the SDK
# parse `.stdout` before the primitive sees it, so the array arrives bare and no
# shaping handler is needed.
#
# It releases one issue, waits for `issue.settled`, then releases the next. A
# poll that lands while a batch is still draining is dropped, which is the
# behavior you want from an interval poller: no issue is ever in flight twice.
# The price is that every path an issue can take MUST end in `issue.settled`,
# or the batch stalls until the engine restarts. That is why both failure
# topics below are routed and none is left to `exec.error`.
[[handlers]]
name = "split-issues"
path = "~/.local/share/emergent/primitives/bin/stream-runner"
args = ["--load-topic", "issues.listed", "--publish-as", "issue.found",
        "--ack-topic", "issue.settled", "--end-topic", "issues.drained"]
unwrap_stdout = true
subscribes = ["issues.listed", "issue.settled"]
publishes = ["issue.found", "issues.drained"]

# --------------------------------------------------------- analysis (fan-out)

# These two run concurrently because they subscribe to the same event.
# No &, no xargs -P, no gather. The topology is the scheduler.

# Rung 3, the carry-through idiom: capture, one act (`claude`), merge. `$orig`
# keeps every field of the issue, so the payload survives the feedback loop, and
# the merge takes three named fields from the model and nothing else, so a reply
# cannot overwrite `number` or `attempt`.
#
# Every way this can fail is `issue.score-failed`. `pipefail` makes a dead
# `claude` (auth, network, rate limit) the body's exit status; without it the
# last jq would read nothing, exit 0, and publish nothing at all. A reply that is
# not a JSON object fails the last jq by itself.
#
# `--tools ""` because the issue text comes from strangers and this judge needs
# nothing but its prompt.
[[handlers]]
name = "score-severity"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "-s", "issue.enriched", "-s", "issue.backoff-elapsed",
        "--publish-as", "issue.scored", "--error-as", "issue.score-failed", "-t", "60000",
        "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); \
     jq -r '"Rate this issue. Reply with one JSON object {severity, confidence, rationale}, severity one of low|medium|high|critical, confidence 0 to 1.\n\(.title)\n\(.body)\nPrior context: \(.context | join("; "))"' <<< "$p" \
     | claude -p --tools "" --output-format text \
     | jq -c --argjson orig "$p" '$orig + {severity, confidence, rationale}' ''']
subscribes = ["issue.found", "issue.enriched", "issue.backoff-elapsed"]
publishes = ["issue.scored", "issue.score-failed"]

# Rung 3 again: capture, one act (`gh` search), merge. Nothing waits on this
# side branch, so its failure stays on `exec.error` for the `errors` sink.
[[handlers]]
name = "detect-duplicates"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.dupe-checked", "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); \
     gh issue list --state open --search "$(jq -r .title <<< "$p")" --json number \
     | jq -c --argjson orig "$p" '{number: $orig.number, duplicate_of: (map(.number) - [$orig.number] | first)}' ''']
subscribes = ["issue.found"]
publishes = ["issue.dupe-checked"]

# Rung 2. "Not a duplicate" needs no response, so this filter has one arm.
[[handlers]]
name = "route-duplicate"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.dupe-checked", "--publish-as", "issue.duplicate-found", "--",
        "jq", "-c", "select(.duplicate_of != null)"]
subscribes = ["issue.dupe-checked"]
publishes = ["issue.duplicate-found"]

# ------------------------------------------------------ routing (the branch)

# The if/else, as three rung-2 routers on one event. Exactly one matches any
# payload: `settled` or not, and if not, attempts remaining or not. The model's
# reply is untrusted input, so `settled` also demands a numeric confidence and a
# known severity. A reply of {"severity": "banana"} is merely uncertain.

[[handlers]]
name = "route-confident"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.triaged", "--", "jq", "-c",
  '''def settled: (.confidence | type) == "number" and .confidence >= 0.8 and (.severity | IN("low", "medium", "high", "critical"));
     select(settled)''']
subscribes = ["issue.scored"]
publishes = ["issue.triaged"]

[[handlers]]
name = "route-uncertain"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.uncertain", "--", "jq", "-c",
  '''def settled: (.confidence | type) == "number" and .confidence >= 0.8 and (.severity | IN("low", "medium", "high", "critical"));
     select((settled | not) and .attempt < 3)''']
subscribes = ["issue.scored"]
publishes = ["issue.uncertain"]

# The guard that keeps the loop finite. It is the third arm of the same branch,
# not a bystander: an attempt-3 issue matches here and nowhere else, so it
# escalates INSTEAD of circulating.
[[handlers]]
name = "depth-guard"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.scored", "--publish-as", "issue.escalated", "--", "jq", "-c",
  '''def settled: (.confidence | type) == "number" and .confidence >= 0.8 and (.severity | IN("low", "medium", "high", "critical"));
     select((settled | not) and .attempt >= 3) | {number, attempt, reason: "still uncertain"}''']
subscribes = ["issue.scored"]
publishes = ["issue.escalated"]

# ------------------------------------------------------------ the feedback edge

# An uncertain issue goes back to the judge carrying more than it had. This
# single block is what makes the system adaptive rather than merely sequential.
# Rung 3: capture, one act (`gh` view, shaped by its own --jq), merge.
[[handlers]]
name = "enrich-context"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.uncertain", "--publish-as", "issue.enriched",
        "--error-as", "issue.enrich-failed", "--", "bash", "-c",
  '''set -o pipefail; p=$(cat); \
     gh issue view "$(jq -r .number <<< "$p")" --json comments --jq '[.comments[-3:][].body] | join("\n") | .[:2000]' \
     | jq -R -s -c --argjson orig "$p" '. as $c | $orig | .context += [$c] | .attempt += 1' ''']
subscribes = ["issue.uncertain"]
publishes = ["issue.enriched", "issue.enrich-failed"]

# --------------------------------------------------------------- failure policy

# Two exclusive routers on the failure event decide first: try again, or give
# up. Only a retryable failure ever waits, so an issue that is out of attempts
# escalates at once instead of sleeping first.
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
args = ["-s", "issue.score-failed", "--publish-as", "issue.escalated", "--", "jq", "-c",
        'select(.attempt < 3 | not) | {number, attempt, reason: "scoring failed"}']
subscribes = ["issue.score-failed"]
publishes = ["issue.escalated"]

# The delay. Its one act is the pause, 2, 4, then 8 seconds, read from the
# payload because nothing else knows the attempt. `-t` has to exceed the longest
# pause. `del(.error)` strips the reserved key exec-handler added. While this
# sleeps the whole batch waits, because `split-issues` has one issue in flight.
# That is the price of acking at the end of the flow, and it is why the pauses
# are short. `--max-concurrent` stays at 1 for the same reason.
[[handlers]]
name = "delay-rescore"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.score-retryable", "--publish-as", "issue.backoff-elapsed",
        "-t", "20000", "--", "bash", "-c",
  '''p=$(cat); sleep $((2 ** $(jq -r .attempt <<< "$p"))); \
     jq -c 'del(.error) | .attempt += 1' <<< "$p" ''']
subscribes = ["issue.score-retryable"]
publishes = ["issue.backoff-elapsed"]

[[handlers]]
name = "escalate-unenrichable"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.enrich-failed", "--publish-as", "issue.escalated", "--", "jq", "-c",
        '{number, attempt, reason: "enrichment failed"}']
subscribes = ["issue.enrich-failed"]
publishes = ["issue.escalated"]

# Fan-in. Both exits from triage become the one ack `split-issues` waits for.
[[handlers]]
name = "settle"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.triaged", "-s", "issue.escalated", "--publish-as", "issue.settled", "--",
        "jq", "-c", "{number}"]
subscribes = ["issue.triaged", "issue.escalated"]
publishes = ["issue.settled"]

# ---------------------------------------------------------------------- egress

# One act each. Every label is also what removes the issue from the next poll.

[[sinks]]
name = "label-issue"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.triaged", "--", "bash", "-c",
  '''p=$(cat); gh issue edit "$(jq -r .number <<< "$p")" --add-label "severity:$(jq -r .severity <<< "$p")" ''']
subscribes = ["issue.triaged"]

[[sinks]]
name = "label-duplicate"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.duplicate-found", "--", "bash", "-c",
  '''gh issue edit "$(jq -r .number)" --add-label duplicate''']
subscribes = ["issue.duplicate-found"]

[[sinks]]
name = "label-escalated"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.escalated", "--", "bash", "-c",
  '''gh issue edit "$(jq -r .number)" --add-label needs-human''']
subscribes = ["issue.escalated"]

# SLACK_WEBHOOK is inherited from the engine's own environment. Args are
# literals, so "$SLACK_WEBHOOK" only expands because a shell runs this body.
[[sinks]]
name = "notify"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.triaged", "--", "bash", "-c",
  '''set -o pipefail; jq -c '{text: "Triaged #\(.number) as \(.severity)"}' \
     | curl -sf -X POST -H 'Content-Type: application/json' -d @- "$SLACK_WEBHOOK" ''']
subscribes = ["issue.triaged"]

[[sinks]]
name = "notify-human"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.escalated", "--", "bash", "-c",
  '''set -o pipefail; jq -c '{text: "Needs a human: #\(.number) after \(.attempt) attempts, \(.reason)"}' \
     | curl -sf -X POST -H 'Content-Type: application/json' -d @- "$SLACK_WEBHOOK" ''']
subscribes = ["issue.escalated"]

# Named types, not `exec.*`: this config predates wildcard routing (0.10.10).
[[sinks]]
name = "errors"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "exec.error", "-s", "issues.list-failed", "--", "jq", "-c", "."]
subscribes = ["exec.error", "issues.list-failed"]
```

This config was run end to end against stubbed `gh`, `claude`, and `curl` with
three issues: one clear, one vague, one the model answered in prose. The event
store recorded 1 `issue.triaged`, 2 `issue.enriched`, 3 `issue.score-failed`,
2 `issue.score-retryable`, 2 `issue.backoff-elapsed`, 2 `issue.escalated`,
3 `issue.settled`, 1 `issues.drained`, and nothing on `exec.error`. The third
failure escalated at once, with no pause in front of it.

---

## What emerged

Nothing in that config states how many times an issue gets scored. That number
is a consequence of independent local rules meeting each other: a confidence
produced by a model, a threshold in a jq predicate, a depth limit in a guard,
and a retry policy that shares the same counter.

The observable result is that clear issues settle on the first pass. Ambiguous
ones circulate, accumulating comment context each time, and either cross the
threshold on a later attempt or fall out to a human after the third. Some issues
will settle on attempt two because the comments happened to clarify them. That
behavior was not designed. It fell out.

So did the pacing. No primitive knows how long a triage takes, yet the batch
moves exactly as fast as issues settle, because the ack that releases the next
issue is the same event that records the last one finishing.

That is what to aim for, and it is the difference between this and `triage.py`,
which could only ever have looped once because someone wrote a `for`.

## Gate 2, applied

| Test | Result |
|---|---|
| **Script** | No script file. Every primitive is a marketplace primitive used as itself, one command as an args array, or one shell body with a single act. Every body with an act in the middle of a pipe opens with `set -o pipefail`. |
| **Requirement** | Poll, triage, label, escalate: four verbs. Everything else traces to a failure of one (`route-rescorable`, `delay-rescore`, the three escalators), to the uncertain case the requirement names (`enrich-context`, the routers), or to `stream-runner`'s ack (`settle`). `detect-duplicates` and its two followers are the one thing nobody asked for. They are here to show fan-out, and in a real build they would wait for someone to want them. |
| **Dead end** | Every error topic has a subscriber. `issues.drained` and `issues.list-done` have none, on purpose. Each router group is exhaustive: the last arm of each is written as the negation of the others. |
| **Log** | Every state change has an event. One issue's trail reads as a narrative: `sqlite3 ~/.local/share/emergent/triage/events.db "SELECT message_type, source FROM events WHERE json_extract(payload_json, '$.number') = 7 ORDER BY timestamp_ms"`. |
| **Subscriber** | "Also track time-to-triage": one sink on `issue.found` and `issue.settled`, zero edits. "Post a weekly digest": a subscriber on `issues.drained`, which nobody consumes yet. |
| **Injection** | Add the `inject` source from `patterns.md` with a one-line unwrap publishing `issue.found`, POST a synthetic issue, and the whole triage path runs on it. |
| **Swap** | `claude -p` to an `ollama` curl is one line in `score-severity`. Swapping it for `jev-handler` is a step up the ladder, from rung 3 to rung 1. Nothing else notices either way. |
| **Kill** | Kill `detect-duplicates` and triage still runs, minus dedup. Kill `notify` and labeling continues. Kill `settle` and the batch stalls after one issue, which is the honest cost of ack-driven pacing. |
| **Name** | Each name is one act or one predicate: poll-issues, split-issues, score-severity, detect-duplicates, route-confident, route-rescorable, delay-rescore, enrich-context, depth-guard, settle, label-issue. No "and" anywhere. |
| **Surprise** | Yes: attempt counts, which issues resolve through enrichment versus escalate, and a batch pace nobody set. |

## Honest notes on the cost

The decomposed version is more TOML, and the jq for carrying fields through an
LLM call is awkward. That is a real cost, not one to pretend away.

It also processes one issue at a time. That is a choice, made by picking
`stream-runner` as the splitter, and it buys bounded work and a poller that can
never double-process. If throughput mattered more, the split would be HTTP
fan-out instead, `score-severity` would take `--max-concurrent 4`, and the
in-flight guarantee would have to come from somewhere else, such as a
`triage:pending` label applied on `issue.found`.

What you buy is that every one of those review answers is "yes." In the monolith
every one is "no," and no amount of later refactoring inside `triage.py` will
change that, because the boundaries do not exist to refactor toward.

If a step's shell really is getting unreadable, that is the signal to write one
small SDK primitive for that step, not to merge the step into its neighbors and
not to move it into a script file.
