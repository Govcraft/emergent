# jev-handler: Judgment as a Pipeline Step

`jev-handler` is a marketplace handler that asks [TypeSafe System One](https://docs.typesafe.ai) (Jev) a fixed set of typed questions about each event and publishes the answers with calibrated confidence. Use it when a step in your pipeline is a judgment rather than a transformation: is this message unwanted, which of these folders fits, how urgent is it.

It ships in emergent-primitives 0.12.0 and needs engine 0.14.0 or later.

## Why a Judge Instead of an LLM Call

An `exec-handler` running `claude -p` returns prose. To act on prose you parse it, and a confident-sounding answer looks the same as a guess. `jev-handler` returns numbers you can route on:

- **Typed answers.** Each question is a yes/no (`noul`), a pick from a set (`choice`), or a position on an ordered rubric (`score`). The answer always has the shape the question asked for.
- **Calibrated confidence.** Every `choice` and `score` answer carries a confidence and the full probability distribution, so "sure" and "unsure" are different events.
- **Policy stays in your config.** The handler publishes the answers and makes no decision. Thresholds are `jq` selectors in `emergent.toml`, so changing the policy is a config edit and the raw judgments stay reusable.
- **Failure is an event.** A failed call publishes an error event with a `kind` that tells a router whether to requeue the item, quarantine it, or page a human.

## Install

```bash
emergent marketplace install jev-handler
export TYPESAFE_API_KEY="..."   # from https://console.typesafe.ai
```

The key is read from the `TYPESAFE_API_KEY` environment variable and from nowhere else. There is deliberately no `--api-key` flag, because a key on a command line is a key in the process table and in `emergent.toml`. The key never appears in a log line, in a published payload, or in debug output. The engine forwards its own environment to every primitive, so exporting the variable before you start the engine is enough. If the variable is missing, the handler exits at startup with `TYPESAFE_API_KEY is not set` and the engine reports the exit. For an unattended service, see [Configuration > Secrets](../configuration.md#secrets).

## Run the Example

[`config/examples/jev-triage/`](../../config/examples/jev-triage/) is a complete topology: it reads one message from disk, asks three questions about it, routes the verdict into one of three confidence bands, and prints the result.

```
exec-source (cat message.txt) ──> jev-handler ──> route-confident ──> print
                                       │      ├─> route-uncertain ──> print
                                       │      └─> route-review    ──> print
                                       └─ mail.judge-failed ────────> print
```

```bash
emergent marketplace install exec-source jev-handler exec-handler exec-sink
export TYPESAFE_API_KEY="..."

# From the repository root, because the example uses relative paths
emergent --config ./config/examples/jev-triage/emergent.toml
```

Each run makes one API call. The sample message is a fake mailbox-suspension notice. The sink prints a verdict shaped like this, with your own numbers:

```json
{
  "band": "confident",
  "kind": "phish",
  "confidence": 0.97,
  "lure": 0.95,
  "pressure": 2.0
}
```

Edit `message.txt` and run it again to watch the answers move. Lower the `0.9` threshold in `emergent.toml` and the same verdict lands in a different band without touching any code.

## The Questions File

One file, read once at startup, asked about every message in a single request. The map key is the question id: the answer comes back under the same key, and routers select on it as `.answers.<id>`.

```toml
# noul: a yes/no judgment. The answer is a bare probability of "yes" from 0 to 1.
# `criteria` is optional and may only use the keys "true" and "false".
[questions.lure]
type = "noul"
instructions = "Does this message try to get the reader to click a link or reply with information?"
criteria = { "true" = "There is something concrete to act on.", "false" = "The message is informational only." }

# choice: one option out of a defined set. At least two options.
[questions.kind]
type = "choice"
instructions = "What kind of message is this?"
criteria = { phish = "Credential theft under a false identity.", cold_pitch = "Unsolicited sales.", vendor_notice = "A legitimate operational notice.", personal = "Ordinary correspondence." }

# score: a position along an ordered rubric. Order is the meaning, and the
# answer may land between levels.
[questions.pressure]
type = "score"
instructions = "How much time pressure does the message apply?"
criteria = ["No deadline is expressed.", "A soft deadline.", "Act now or lose access."]
```

The file may also be written as JSON; the extension picks the parser. The id is never sent to the model, so all of the meaning belongs in `instructions` and `criteria`.

Validation happens at startup, before the handler connects to the engine, so a misconfigured handler never appears healthy in a topology. Unknown keys are rejected rather than ignored: a `critera` typo would otherwise leave a choice question with no options on every message. Question ids are restricted to `[A-Za-z_][A-Za-z0-9_]*` so a router can always write `.answers.kind` without quoting.

## What It Publishes

A successful message publishes the answers in the API's exact JSON shape, with the inbound payload nested under `input`:

```json
{
  "input": {"issue": 42, "subject": "Action required"},
  "answers": {
    "lure": {"type": "noul", "noul": 0.93},
    "kind": {"type": "choice", "choice": "phish", "confidence": 0.99,
             "probabilities": {"phish": 0.99, "cold_pitch": 0.0, "vendor_notice": 0.01, "personal": 0.0}},
    "pressure": {"type": "score", "score": 2.0, "confidence": 1.0,
                 "legend": {"0": "No deadline is expressed.", "1": "A soft deadline.", "2": "Act now or lose access."},
                 "probabilities": {"0": 0.0, "1": 0.0, "2": 1.0}}
  },
  "usage": {"input_tokens": 391, "output_tokens": 34},
  "model": "jev-1.13.0",
  "request_id": "req_2f8c1d..."
}
```

The shape is passed through untouched because that is what downstream `jq` routers select on. `input` is nested rather than spread, because `answers`, `usage` and `model` would collide with ordinary payload keys.

By default the whole inbound payload is sent as the state to judge. `--state-pointer` takes a JSON pointer to send only part of it: the example uses `/stdout` to send the text `exec-source` read, and a mail pipeline might use `/body`.

## Routing Is Topology, Not Code

The handler publishes exactly two message types and makes no decision about the answers. Confidence banding, thresholds and fan-out are `exec-handler` blocks running `jq` selectors:

```toml
[[handlers]]
name = "judge"
path = "~/.local/share/emergent/primitives/bin/jev-handler"
args = ["-s", "mail.fetched", "--questions", "./questions.toml",
        "--state-pointer", "/body", "--publish-as", "mail.judged", "-e", "mail.judge-failed"]
subscribes = ["mail.fetched"]
publishes = ["mail.judged", "mail.judge-failed"]

# The selectors are mutually exclusive and cover every value from 0 to 1,
# so exactly one of them republishes each verdict.
[[handlers]]
name = "route-confident"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "mail.judged", "--publish-as", "triage.confident",
        "--", "jq", "-c", "select(.answers.kind.confidence >= 0.9)"]
subscribes = ["mail.judged"]
publishes = ["triage.confident"]

[[handlers]]
name = "route-review"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "mail.judged", "--publish-as", "triage.needs-review",
        "--", "jq", "-c", "select(.answers.kind.confidence < 0.9)"]
subscribes = ["mail.judged"]
publishes = ["triage.needs-review"]
```

A `jq -c 'select(...)'` that matches nothing writes nothing and exits 0, which `exec-handler` treats as a filter, so a band that does not apply publishes nothing rather than an empty event.

## Failures Route on `error.kind`

A failed message publishes the error type (default `jev.error`) carrying the inbound payload, with the details under a reserved `error` key:

```json
{"issue": 42,
 "error": {"kind": "rate_limited", "status": 429, "attempts": 4,
           "message": "rate limited by the API (HTTP 429) after 4 attempt(s)",
           "endpoint": "https://api.typesafe.ai/v1/systemone", "model": "jev-latest",
           "request_id": "req_2f8c1d...", "body": "...", "detail": null}}
```

`error.kind` is the contract a router selects on:

| `error.kind` | What happened | What to do with the item |
|---|---|---|
| `auth` | The credentials were rejected (`401`) | Page a human; the key is wrong or revoked |
| `billing` | The organization is out of API credit (`402`) | Page a human, then hold and requeue the item. The item is fine and succeeds unchanged once credit is added, so do not quarantine it |
| `invalid_request` | The request body was rejected (`422`, or another `4xx` that is not `401`, `402` or `429`) | Quarantine; the questions file is wrong and retrying fails the same way |
| `rate_limited` | The attempt budget was spent on `429`s | Requeue |
| `server_error` | The API failed or was overloaded (`5xx`, including `529`) | Requeue |
| `transport` | DNS, connection, TLS, or a per-attempt timeout | Requeue |
| `timeout` | The whole-message `--timeout` budget elapsed | Requeue |
| `bad_response` | A `2xx` body that is not the documented envelope | Page a human; the vendor's contract moved |
| `answer_contract` | A well-formed response that does not answer the questions asked | Page a human; the vendor's contract moved |
| `state_not_found` | `--state-pointer` did not resolve in the payload | Quarantine; the upstream payload shape is wrong |

Route failures the same way as answers, with exclusive selectors, and make the last router match by negation so a kind added in a later release lands somewhere instead of vanishing. The [emergent-primitives README](https://github.com/Govcraft/emergent-primitives#jev-handler) has the full three-router block.

Error bodies are truncated to 2 KiB and may echo the state that was sent. That is your own data rather than a secret, but it does land in the event store.

## Throughput and Rate Limits

`--max-concurrent` is the real rate control against a rate-limited API, and it defaults to 1 to match the other handlers. A task waiting out a retry backoff holds its slot, so at 1 a run of `429`s stalls the handler until the limit clears. That is backpressure rather than a hang. For an IO-bound API call a higher value is usually right.

The two timeouts are separate on purpose. `--timeout` is the budget for one whole message including every retry; `--request-timeout` bounds one HTTP attempt.

Retries cover `429`, `5xx` and transport failures. Every other `4xx` is fatal on the first attempt, because a bad key or a rejected questions file would fail identically every time. A `Retry-After` header is honoured but clamped to `--max-retry-after-ms`. Only the delta-seconds form is read; the HTTP-date form falls back to computed backoff.

## Flags

| Flag | Default | Meaning |
|---|---|---|
| `-s`, `--subscribe` | required, repeatable | Message types to subscribe to |
| `--questions` | required | Path to the questions file, `.toml` or `.json` |
| `--publish-as` | `jev.answered` | Message type for an answered message |
| `-e`, `--error-as` | `jev.error` | Message type for a failed message |
| `--model` | `jev-latest` | Model alias or pinned model id |
| `--state-pointer` | whole payload | JSON pointer to the part of the payload sent as `state` |
| `-t`, `--timeout` | `120000` | Total budget for one message in ms, retries included |
| `--request-timeout` | `30000` | Timeout for a single HTTP attempt in ms |
| `--max-concurrent` | `1` | Maximum messages in flight at once |
| `--max-attempts` | `4` | Total attempts per message, including the first |
| `--retry-base-ms` | `500` | First backoff step; doubled per attempt |
| `--retry-max-delay-ms` | `30000` | Ceiling on a computed backoff |
| `--max-retry-after-ms` | `60000` | Ceiling on a server-supplied `Retry-After` |
| `--endpoint` | `https://api.typesafe.ai/v1/systemone` | Evaluation endpoint |

This table matches `jev-handler --help` for primitives 0.12.0. The [emergent-primitives README](https://github.com/Govcraft/emergent-primitives#jev-handler) is the reference that ships with the code.

## See Also

- [Handlers](handlers.md): the other ways to build a handler
- [Examples](../examples.md#jev-triage): the runnable triage topology
- [Configuration > Secrets](../configuration.md#secrets): keeping `TYPESAFE_API_KEY` out of files
- [TypeSafe System One documentation](https://docs.typesafe.ai): question types and the API itself
