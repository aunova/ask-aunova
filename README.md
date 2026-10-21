# Ask Aunova

Ask a building project a question and get an answer that proves itself: every fact cites a
signed source document, the server checks that signature **before** answering, and what you may
hear is decided by your bearer token, server-side.

Entry for **Build, Ship, Shape: Amazon Developer Hackathon — Alexa+ track**: a self-hosted MCP
server in Rust (spec `2025-11-25`, Streamable HTTP), an open-format Agent Skill, and a web page
that is a real MCP client with a voice path.

> **All project data in this repository is SYNTHETIC DEMO DATA.** "Palm Vista Tower", its
> developer, every buyer and every certificate issuer are invented for this demonstration. No
> real project, person, company or authority is involved, named or implied, and nothing here is
> approved, endorsed or certified by anyone.

## Why it is not just a chatbot over a database

An assistant that repeats a record is a chatbot. This one **refuses to state what it cannot
verify**, and the refusals are the product:

- **Verification is executed, not narrated.** Every source document is content-hashed and
  Ed25519-signed by a fictional issuer. The server recomputes the hash and re-checks the
  signature **on every call** — not at startup, not from a cache. Flip one byte of a certificate
  while the server is running and the next answer refuses, with no restart.
- **The role is the token.** The client cannot name a role; it is resolved server-side. A buyer
  bound to unit 1204 asking about unit 0802 is refused by the server, not filtered by the page.
- **The server does not parse English.** `verify_claim` takes a typed assertion — subject, field,
  value — and compares exactly. It answers `SUPPORTED`, `CONTRADICTED`, or `UNVERIFIED` with a
  reason, and never guesses what a sentence meant. Turning language into arguments is the calling
  model's job, which is what MCP tool schemas are for.

## Three commands

Rust 1.88 or newer for the default build (`rustup` default toolchain is fine). Nothing else to
install: no Node, no Python, no Docker, no system libraries. The optional `live` feature needs
**1.94.1 or newer**, which is the AWS SDK's own minimum, not ours.

```sh
cargo build                                   # 1. build the workspace
cargo run -p verify-dataset -- data/palm-vista --keys-dir keys   # 2. check the signed dataset
cargo run -p ask-aunova-server                # 3. serve MCP on 127.0.0.1:8731
```

Step 2 should print `verified 14 documents from 7 issuers`. If it does not, the dataset and its
signatures disagree and nothing else is worth running.

Then, in a second terminal, the web page:

```sh
cargo run -p ask-aunova-simulator             # http://127.0.0.1:8732
```

Prove the server end-to-end in one command:

```sh
bash scripts/smoke.sh
```

It starts the server on loopback, performs `initialize`, `tools/list` and `tools/call`, checks a
refusal and an evidence card, asserts an unauthenticated request gets `401`, and stops the server
by PID.

## Where the protocol is implemented

Reviewers and automated analysis usually want these paths directly:

| What | Where |
|---|---|
| MCP protocol version `2025-11-25`, pinned and advertised | `server/src/handler.rs` (`SPOKEN_PROTOCOL_VERSION`, `supported_protocol_versions`) |
| Streamable HTTP transport, `POST /mcp`, loopback only | `server/src/lib.rs` (`router`, `MCP_PATH`, `listen_addr`) |
| Bearer token → role, and the `401` | `server/src/auth.rs` |
| The six tools and their closed input schemas | `server/src/tools.rs` |
| Hash + Ed25519 verification on every call | `server/src/verify.rs`, `tools/dataset-manifest/src/verify.rs` |
| Evidence card served as an MCP resource (`ui://`) | `server/src/card.rs` |
| MCP **client** (initialize / tools-list / tools-call / resources-read) | `simulator/src/mcp.rs` |

The pin is deliberate: the SDK's `LATEST` is a later revision that removed the `initialize`
handshake, and its default advertises every revision it knows. `server/src/handler.rs` narrows
that, and a test fails if the unsupported revision is ever advertised.

## The six tools

| Tool | Returns |
|---|---|
| `get_milestones` | milestones, status, dates, and the document ids that evidence each |
| `get_certificates` | certificates, issuer, dates, and the signature check result for each |
| `get_payment_status` | instalments and escrow state for one unit |
| `verify_claim` | one named field compared exactly: `SUPPORTED`, `CONTRADICTED`, `UNVERIFIED` |
| `ask_project` | an answer composed only from verified facts, every one cited |
| `whats_changed` | what changed since the cursor you pass back |

Two demo identities ship with the synthetic data, and they are deliberately obvious, low-entropy
strings rather than secrets: `demo-token-buyer-1204` (bound to unit 1204) and
`demo-token-developer`. Override with `ASK_AUNOVA_BUYER_TOKEN` and `ASK_AUNOVA_DEVELOPER_TOKEN`.

## Stub by default, hosted only when you ask twice

The simulator runs on deterministic stubs — a scripted model, a silent voice and a fixture
transcriber — and the page says so on screen. **Nothing in a default build calls a hosted
service**, and no test ever does.

Using the hosted services takes two switches, on purpose: a compile-time one a reader can grep
for, and a runtime one the person starting it chooses.

```sh
AWS_PROFILE=<your-profile> cargo run -p ask-aunova-simulator --features live -- --live
```

| Service | What it does here | State | Where |
|---|---|---|---|
| **Amazon Bedrock** — Nova Pro (`amazon.nova-pro-v1:0`) | the tool-use loop: picks tools, composes the answer from their results | **written; live run pending** | `simulator/src/model/live.rs` |
| **Amazon Polly** — neural | reads the answer aloud | **run live** | `simulator/src/voice/live.rs` |

The state column is literal, and it is there because "it is implemented" and "it has been seen
to work" are different claims and only one of them is evidence. Polly has synthesised speech
from this code. The Bedrock tool loop has not completed a live call: the account's Nova Pro
token quota is zero pending a support case, so it has never answered a question end to end.

What is tested of it, without a service: that every request carries the output-token bound and
temperature it is supposed to; that the draft parser reads hand-written JSON, fenced JSON, and
prose the model invented, which becomes an empty draft rather than an uncited sentence; that
JSON round-trips through the SDK's `Document` type; and that the model id is the in-region one.
What is **not** tested is the Bedrock-specific part — `conversation`, `tool_config` and
`step_from`, which turn a question and tool specs into a Converse request and read the reply
back. Three live Converse requests have reached Bedrock; all three came back as
`ThrottlingException` against a daily token quota of zero (`FRICTION_LOG.md`, F-034). So the
request side has been over the wire, and `step_from` has never read a real reply.

The demo therefore runs on the scripted stub model, which exercises the same tool-call path and
shows the same calls in the debug drawer.

**The voice can be hosted on its own.** `--live` runs the hosted model and implies the hosted
voice. When the model is unavailable — a quota, an outage — `--live-voice` runs the scripted
stub model with the hosted voice, so the answer is still spoken:

```sh
AWS_PROFILE=<your-profile> cargo run -p ask-aunova-simulator --features live -- --live-voice
```

**Sending a recording of a voice takes a third switch.** `--live` and `--live-voice` send
**text**: a question, tool results, an answer to be read aloud. Neither switches on hosted
transcription, because that sends a recording of whoever is speaking to a service that may
retain it, and that is a different thing to agree to. Push-to-talk stays on the fixture
transcriber until you add:

```sh
AWS_PROFILE=<your-profile> cargo run -p ask-aunova-simulator --features live -- --live --live-transcribe
```

| Service | What it does here | State | Where |
|---|---|---|---|
| **Amazon Transcribe** — streaming | transcribes push-to-talk, only with `--live-transcribe` | **written; never called** | `simulator/src/transcribe/live.rs` |

Transcribe has never been called from this code. The switch, the refusals and the PCM capture
are tested; the service round trip is not, because nobody has agreed to send a recording of a
voice to it. Push-to-talk on the fixture transcriber and the browser recogniser both work.

`--live-transcribe` on its own is refused rather than quietly ignored: a switch about where
somebody's voice goes is the last one that should fail silently. Whatever is actually running,
the page names every hosted service in use at the top of its footer — from the server's own
flags, not from matching the prose beside them.

Credentials come only from the shared profile named by `AWS_PROFILE`. Not "mostly": the SDK's
default chain reads `AWS_ACCESS_KEY_ID` from the environment **before** it reads the profile, so
an exported key pair would quietly sign as a different identity than the profile you named.
`simulator/src/aws.rs` pins the credentials provider to the profile instead, and a test asserts
that an exported key pair is not used. This repository holds no access key, no secret and no
session token, and has no code path that could log one. The in-region model id is used rather
than a cross-region inference profile.

The demo user's IAM policy is in `docs/aws-iam-policy.json`: those three services, plus
`sts:GetCallerIdentity`, and an explicit `Deny` on everything outside `us-east-1`.

The hosted model is never trusted with a fact. It answers with sentences it says it copied from a
named tool result, and `simulator/src/turn.rs` checks every one against the recorded result
before anything is spoken — so a model that writes a fluent sentence from its own knowledge
produces an answer with nothing in it, rather than a confident fabrication.

Enabling `live` adds the AWS SDKs to the build, and the default build stays as it is. The crypto
provider is chosen explicitly (`rustls` over `ring`) rather than taking the SDK default, which
would pull `aws-lc-rs` and `aws-lc-sys`; `cargo tree --features live` shows neither. To be exact
about what that buys: `ring` compiles C and assembly of its own and still needs a C compiler.
What it avoids is cmake and a vendored copy of a whole TLS library.

## The voice path

Push-to-talk uses the browser's own speech recogniser where there is one. Where there is not — it
needs a vendor service and is unavailable in some Chromium builds — the page captures 16 kHz mono
PCM itself and posts it to the simulator, which transcribes it on the fixture stub unless the
build has the `live` feature **and** was started with `--live-transcribe`, in which case it
would go to Amazon Transcribe — a path that is written and has never been run. Both paths feed
the same interpreter and the same confirmation step, and typed input always works.

**Where your voice goes** is stated on the page, because "runs on loopback" stops being the whole
truth once audio leaves the browser: the browser recogniser sends audio to the browser vendor,
and the Transcribe path sends it to AWS. A unit id that cannot be resolved confidently is never
guessed — the page shows what it heard and asks.

## Layout

```
server/      the MCP server
simulator/   the web page: a real MCP client, with the model/voice/transcriber seams
tools/       sign-dataset, verify-dataset, and the shared manifest library
data/        the synthetic signed dataset
keys/public/ the issuer public keys; private keys are generated locally and never committed
skill/       the Agent Skill, in the open format
scripts/     smoke.sh
```

## Licence

Apache-2.0. See `LICENSE`.
