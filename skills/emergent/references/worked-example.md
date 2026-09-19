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
  fired when:   a poll of the issue tracker completed
  payload:      {command, stdout, exit_code} — stdout holds one issue per line
  published by: poll-issues
  consumed by:  split-issues

issue.received
  fired when:   one issue was POSTed into the topology by the splitter
  payload:      {method, headers, body: {number, title, body, attempt, context[]}}
  published by: issue-in
  consumed by:  unwrap-issue

issue.found
  fired when:   a single issue is awaiting triage at a given attempt level
  payload:      {number, title, body, attempt, context[]}
  published by: unwrap-issue, enrich-context (feedback), retry-scoring
  consumed by:  score-severity, detect-duplicates, depth-guard

issue.scored
  fired when:   the model has assigned a severity and a confidence
  payload:      {number, severity, confidence, rationale, attempt, context[]}
  published by: score-severity
  consumed by:  route-confident, route-uncertain

issue.dupe-checked
  fired when:   the issue has been compared against open issues
  payload:      {number, duplicate_of|null}
  published by: detect-duplicates
  consumed by:  label-issue

issue.triaged
  fired when:   triage settled with enough confidence to act
  payload:      {number, severity, confidence, rationale}
  published by: route-confident
  consumed by:  label-issue, notify

issue.uncertain
  fired when:   triage produced a verdict too weak to act on
  payload:      {number, severity, confidence, attempt, context[]}
  published by: route-uncertain
  consumed by:  enrich-context

issue.escalated
  fired when:   an issue exhausted its triage attempts without settling
  payload:      {number, attempt, last_confidence}
  published by: depth-guard
  consumed by:  notify-human

issue.score-failed
  fired when:   the model call errored
  payload:      {error, ...original}
  published by: score-severity (error path)
  consumed by:  retry-scoring
```

Feedback edge: `issue.uncertain` to `enrich-context` back to `issue.found`.
Fan-out: `issue.found` to `score-severity` + `detect-duplicates` + `depth-guard`.
Join: none required; `label-issue` acts on whichever arrives.

### Topology

```
                    ┌──────────────────────────────────────────┐
                    │                                          │ (feedback)
poll-issues ─> issues.listed ─> split-issues ─(N POSTs)─> issue-in ─> unwrap-issue
                                                                            │
                              ┌─────────────────────────────────────────────┘
                              ▼
                          issue.found ──┬──> score-severity ──> issue.scored
    ▲                         │                          │
    │                         ├──> detect-duplicates     ├──> route-confident ──> issue.triaged ──┬──> label-issue
    │                         │           │              │                                        └──> notify
    │                         │    issue.dupe-checked    └──> route-uncertain ──> issue.uncertain
    │                         │                                                          │
    │                         └──> depth-guard ──> issue.escalated ──> notify-human       │
    │                                                                                     │
    └──────────────────────────── enrich-context ─────────────────────────────────────────┘
```

### Config

```toml
[engine]
name = "triage"
socket_path = "auto"
api_port = 8891

# ---------------------------------------------------------------- ingress

# The poll lists; it does not fan out. One execution is one event, so this
# publishes a single `issues.listed` whose stdout holds every issue as lines.
#
# --correlate stamps one ID on everything this process publishes, which groups
# the whole poller in the event store. It is NOT a per-issue key: every issue
# from every poll shares it. Per-issue identity is `number`, carried in the
# payload, and that is what any join here keys on.
#
# Note the three topics come from `publishes` in order (stdout, stderr, exit).
[[sources]]
name = "poll-issues"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--correlate", "--interval", "60000", "--shell", "bash", "--command",
  """gh issue list --state open --json number,title,body --limit 50 \
     | jq -c '.[] | {number, title, body, attempt: 1, context: []}'"""]
publishes = ["issues.listed", "issues.list-failed", "issues.list-done"]

# One event per issue. This is the part that cannot be done with a jq split:
# exec-handler publishes exactly one event per execution, so emitting N objects
# on stdout would collapse into one event holding a blob. The loop here does no
# work, it only publishes, and each POST becomes an independent event that runs
# concurrently with its siblings.
[[sources]]
name = "issue-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8090"]
publishes = ["issue.received"]

[[sinks]]
name = "split-issues"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issues.listed", "--", "bash", "-c",
  """jq -r '.stdout' | while IFS= read -r line; do \
       [ -n "$line" ] || continue; \
       printf '%s' "$line" \
       | curl -sf -X POST -H 'Content-Type: application/json' -d @- \
           http://127.0.0.1:8090; \
     done"""]
subscribes = ["issues.listed"]

# http-source nests the POST body under `.body`. Unwrap once here so every
# downstream handler sees a flat issue, and so the feedback edges can republish
# `issue.found` in the same shape.
[[handlers]]
name = "unwrap-issue"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.received", "--publish-as", "issue.found", "--",
        "jq", "-c", ".body"]
subscribes = ["issue.received"]
publishes = ["issue.found"]

# --------------------------------------------------------- analysis (fan-out)

# These two run concurrently because they subscribe to the same event.
# No &, no xargs -P, no gather. The topology is the scheduler.

[[handlers]]
name = "score-severity"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.scored",
        "--error-as", "issue.score-failed", "-t", "60000", "--", "bash", "-c",
  """p=$(cat); \
     jq -r '"Rate this issue. Reply JSON {severity,confidence,rationale}.\\n\\(.title)\\n\\(.body)\\nPrior context: \\(.context|join("; "))"' <<< "$p" \
     | claude -p --output-format text \
     | jq -c --argjson orig "$p" '. + {number: $orig.number, attempt: $orig.attempt, context: $orig.context}'"""]
subscribes = ["issue.found"]
publishes = ["issue.scored", "issue.score-failed"]

[[handlers]]
name = "detect-duplicates"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.dupe-checked", "--", "bash", "-c",
  """p=$(cat); n=$(jq -r .number <<< "$p"); t=$(jq -r .title <<< "$p"); \
     dup=$(gh issue list --state open --search "$t" --json number \
           | jq -c --argjson me "$n" 'map(select(.number != $me)) | first | .number // null'); \
     jq -n -c --argjson n "$n" --argjson d "$dup" '{number: $n, duplicate_of: $d}'"""]
subscribes = ["issue.found"]
publishes = ["issue.dupe-checked"]

# ------------------------------------------------------ routing (the branch)

# The if/else. Two subscribers, opposite predicates, nothing choosing between
# them. Each route is separately visible in the log and separately swappable.

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
        "jq", "-c", "select(.confidence < 0.8)"]
subscribes = ["issue.scored"]
publishes = ["issue.uncertain"]

# ------------------------------------------------------------ the feedback edge

# An uncertain issue re-enters triage carrying more than it had. This single
# block is what makes the system adaptive rather than merely sequential.

[[handlers]]
name = "enrich-context"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.uncertain", "--publish-as", "issue.found", "--", "bash", "-c",
  """p=$(cat); n=$(jq -r .number <<< "$p"); \
     comments=$(gh issue view "$n" --json comments | jq -r '.comments[-3:][].body' | head -c 2000); \
     jq -c --arg c "$comments" '.context += [$c] | .attempt += 1' <<< "$p\""""]
subscribes = ["issue.uncertain"]
publishes = ["issue.found"]

# The guard that keeps the loop finite. Predicates on both sides are exclusive,
# so a fourth-attempt issue escalates instead of circulating forever.

[[handlers]]
name = "depth-guard"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.found", "--publish-as", "issue.escalated", "--",
        "jq", "-c", "select(.attempt > 3) | {number, attempt, last_confidence: (.confidence // null)}"]
subscribes = ["issue.found"]
publishes = ["issue.escalated"]

# --------------------------------------------------------------- retry policy

[[handlers]]
name = "retry-scoring"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "issue.score-failed", "--publish-as", "issue.found", "--", "bash", "-c",
        "sleep 5; jq -c 'select(.attempt <= 3) | .attempt += 1'"]
subscribes = ["issue.score-failed"]
publishes = ["issue.found"]

# ---------------------------------------------------------------------- egress

[[sinks]]
name = "label-issue"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.triaged", "--", "bash", "-c",
        "n=$(jq -r .number); s=$(jq -r .severity); gh issue edit \"$n\" --add-label \"severity:$s\""]
subscribes = ["issue.triaged"]

[[sinks]]
name = "notify"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.triaged", "--", "bash", "-c",
        """jq -c '{text: "Triaged #\\(.number) as \\(.severity)"}' \
           | curl -s -X POST -H 'Content-Type: application/json' -d @- "$SLACK_WEBHOOK\""""]
subscribes = ["issue.triaged"]

[[sinks]]
name = "notify-human"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "issue.escalated", "--", "bash", "-c",
        """jq -c '{text: "Needs a human: #\\(.number) after \\(.attempt) attempts"}' \
           | curl -s -X POST -H 'Content-Type: application/json' -d @- "$SLACK_WEBHOOK\""""]
subscribes = ["issue.escalated"]

[[sinks]]
name = "errors"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "system.error.*", "-s", "exec.error", "--", "jq", "-c", "."]
subscribes = ["system.error.*", "exec.error"]

[[sinks]]
name = "topology"
path = "~/.local/share/emergent/primitives/bin/topology-viewer"
args = ["--port", "8009"]
subscribes = ["system.started.*", "system.stopped.*", "system.error.*"]
```

---

## What emerged

Nothing in that config states how many times an issue gets triaged. That number
is a consequence of three independent local rules meeting each other: a
confidence score produced by a model, a threshold in a jq predicate, and a depth
limit in a guard.

The observable result is that clear issues settle on the first pass, ambiguous
ones circulate, accumulating comment context each time, and either cross the
threshold on a later attempt or fall out to a human at four. Some issues will
settle on attempt two because the comments happened to clarify them. That
behavior was not designed. It fell out.

That is what to aim for, and it is the difference between this and `triage.py`,
which could only ever have looped once because someone wrote a `for`.

## Gate 2, applied

| Test | Result |
|---|---|
| **Log** | Every state change has an event. One issue's trail reads as a narrative: filter the log on `number`, or follow the `causation_id` chain that `exec-handler` sets on every hop. |
| **Subscriber** | "Also track time-to-triage": one sink on `issue.found` and `issue.triaged`, zero edits. |
| **Injection** | POST a synthetic `issue.found` and the whole triage path runs on it. |
| **Swap** | `claude -p` to an `ollama` curl is one line in `score-severity`. Nothing else notices. |
| **Kill** | Kill `detect-duplicates` and triage still runs, minus dedup. Kill `notify` and labeling continues. |
| **Name** | poll-issues, score-severity, detect-duplicates, route-confident, enrich-context, depth-guard, label-issue. No "and" anywhere. |
| **Surprise** | Yes: attempt counts, and which issues resolve through enrichment versus escalate. |

## Honest notes on the cost

The decomposed version is more TOML and more shell quoting, and the jq gymnastics
for carrying fields through an LLM call are genuinely awkward. That is a real
cost, not one to pretend away.

What you buy is that every one of those seven review answers is "yes." In the
monolith every one is "no," and no amount of later refactoring inside
`triage.py` will change that, because the boundaries do not exist to refactor
toward.

If a step's shell really is getting unreadable, that is the signal to write one
small SDK primitive for that step, not to merge the step into its neighbors.
