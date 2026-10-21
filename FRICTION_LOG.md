# Friction log

Required shape per entry (Devpost rules): task attempted, steps taken, expected vs actual,
severity, workaround, actionable suggestion. Log it when it happens, not on Friday.

Severity: Critical (blocked) · Major (hours lost) · Minor (minutes) · Papercut.

---

## F-001 — Finding out whether Alexa+ tooling is available to participants
- **Task:** register an Alexa+ skill pointing at our MCP server.
- **Steps:** read the hackathon overview and Resources page ("Build with Agent Skills",
  "Streamable HTTP transport"), then the Alexa+ add-on docs; only the hackathon FAQ states access
  is impossible.
- **Expected:** the track page says up front that the Alexa+ Add-on developer tools (Category SDK,
  MCP Toolkit, CLI, Web Simulator) are partner-only.
- **Actual:** the overview and Resources pages do not say so; the FAQ admits individual setup-guide
  pages sometimes omit the notice, "which has caused some confusion". Our own brief planned a
  console registration step that cannot happen.
- **Severity:** Major (would have cost a build day).
- **Workaround:** web-page MCP client ("simulated experience"), as the FAQ allows.
- **Suggestion:** put the partner-only notice on the Alexa+ track card and at the top of every
  add-on setup page, and link a public MCP-client test harness participants can use instead.

## F-002 — Devpost rules/FAQ pages not machine-readable
- **Task:** read the official rules with a fetch tool / reader.
- **Steps:** plain HTTP GET of `/rules` and `/details/faqs`.
- **Expected:** 200 with the rules text.
- **Actual:** HTTP 403; only a full headless browser could read them.
- **Severity:** Papercut.
- **Workaround:** headless Chromium.
- **Suggestion:** publish the official rules as a static page or PDF too.

## F-003 — rmcp's default advertises a protocol revision that has no `initialize` handshake
- **Task:** make an `rmcp` 3.5.1 server speak MCP `2025-11-25`, as the Alexa+ track requires.
- **Steps:** added `rmcp` 3.5.1; read `src/model.rs`, `src/handler/server.rs` and
  `src/service/server.rs`, and rendered the crate's own docs with `cargo doc --offline -p rmcp`.
- **Expected:** a server built with rmcp's defaults advertises only revisions on which it can
  actually complete a handshake.
- **Actual:** `ProtocolVersion::LATEST` is `2026-07-28` (`model.rs:176`), a revision that replaced
  the `initialize` handshake entirely, and the default `supported_protocol_versions()`
  (`handler/server.rs:389`) advertises every known revision including that one. `initialize` still
  answers `2025-11-25`, because rmcp ignores a preferred version that has no handshake
  (`service/server.rs:494-497`) — so the wrong default is invisible over `initialize` and to a
  smoke test, and shows up only in what `server/discover` advertises and in per-request version
  validation. The published crate ships 0 examples.
- **Severity:** Minor (minutes, plus a wrong default that would otherwise have shipped).
- **Workaround:** overrode `supported_protocol_versions` with `ProtocolVersion::known_up_to`, and
  added a test asserting `2026-07-28` is never advertised. One override carries it: also setting
  `get_info().protocol_version` is belt-and-braces and changes nothing observable, which took a
  round of mutation testing to establish.
- **Suggestion:** default a server that still implements the handshake to `LATEST_WITH_INITIALIZE`
  rather than `LATEST`. The rustdoc already explains this well once you go looking —
  `supported_protocol_versions` says to narrow the set with `known_up_to`, `known_up_to` carries a
  worked example, and `LATEST_WITH_INITIALIZE` says "use this, not `LATEST`, whenever the subject
  is the handshake itself" — but nothing points a reader at the question before they have already
  shipped the wrong default, which an example named for the job would fix.

## F-004 — Streamable HTTP responses open with an empty `data:` line
- **Task:** parse the JSON-RPC reply out of a Streamable HTTP POST response.
- **Steps:** POST `initialize` with `Accept: application/json, text/event-stream`; took the first
  `data:` line of the SSE body, the obvious reading of the transport.
- **Expected:** the first `data:` line carries the JSON-RPC message.
- **Actual:** the stream is primed with `data:` (empty), `id: 0`, `retry: 3000` before the message,
  so a first-match parse fails with `Error("EOF while parsing a value", line: 1, column: 0)` —
  six integration tests failed at once, with an error pointing at JSON rather than at framing.
- **Severity:** Minor (one test cycle).
- **Workaround:** take the first *non-empty* `data:` line.
- **Suggestion:** omit the payload-less `data:` line from the priming event, or document the
  priming event where `sse_retry` is documented; one line would have saved the cycle.

## F-005 — `cargo add` cannot tell "no such crate" from "no such version"
- **Task:** confirm `rmcp` 3.5.1 actually exists before building on it.
- **Steps:** `cargo add rmcp@999.0.0 --dry-run`, to make the tool print the available versions.
- **Expected:** an error naming the versions that do exist — the usual way to enumerate them
  without a browser.
- **Actual:** `error: the crate rmcp@999.0.0 could not be found in registry index.` — the same
  wording cargo uses for a crate name that does not exist at all, and no version list, so the
  answer needed a second command.
- **Severity:** Papercut.
- **Workaround:** `cargo add rmcp --dry-run` with no version, to see what resolves.
- **Suggestion:** when the name resolves but the version does not, say so and print the
  available versions.

## F-006 — crate-vetting facts are not reachable from cargo's own output
- **Task:** vet a dependency before adopting it — licence, MSRV, release date, `unsafe` count,
  whether tests ship — without a browser.
- **Steps:** `cargo add`, `cargo tree`, `cargo metadata`, `cargo doc`, then
  `cargo vendor --locked --versioned-dirs` into a scratch directory, and grep.
- **Expected:** cargo answers the standard vetting questions about a crate it has fetched.
- **Actual:** `cargo metadata` gives licence, MSRV, edition and target kinds — so "do tests ship?"
  is answerable (84 test targets) — but there is no publish date and no way to count `unsafe`.
  `cargo doc` renders the public API and says nothing about either. The only route left is
  obtaining the source and grepping it.
- **Severity:** Minor.
- **Workaround:** `cargo vendor` into scratch; the vendored tree carries `.cargo-checksum.json`,
  so it is provably the published artifact and not a repo checkout.
- **Suggestion:** have `cargo info`/`cargo metadata` report the publish date, and have rustdoc
  surface a crate-level "forbids unsafe code" marker when `#![forbid(unsafe_code)]` is present.
  The single most-asked vetting question should not require vendoring the source.

## F-007 — `cargo add --no-default-features` sticks, and a later plain `cargo add` does not undo it
- **Task:** add `serde` with the `derive` feature to a new crate.
- **Steps:** ran `cargo add serde --features derive --no-default-features` to inspect the feature
  list; then, having decided I needed `std`, ran `cargo add serde --features derive` again.
- **Expected:** the second add, with no `--no-default-features`, restores default features.
- **Actual:** the manifest kept `default-features = false` and the printed list still showed
  `- std`. A `no_std` serde would have been built into a std binary.
- **Severity:** Minor (minutes, but it leaves a silently wrong committed manifest).
- **Workaround:** `cargo add serde --features derive --default-features`.
- **Suggestion:** when `cargo add` re-adds a dependency that already carries
  `default-features = false`, print a line saying it is keeping that setting from the existing
  entry. Silence is what makes this one bite.

## F-008 — `cargo add sha2 --features std` fails, and the "similar name" hint points the wrong way
- **Task:** add `sha2` with default features off and only `std` on, the usual RustCrypto shape.
- **Steps:** `cargo add sha2 --no-default-features --features std`.
- **Expected:** success, or an error listing the features that do exist.
- **Actual:** `error: unrecognized feature for crate sha2: std`, then
  `help: a feature with a similar name exists: 'oid'`. The 0.11 line replaced `std` with
  `alloc`/`oid`, and `oid` is X.509 algorithm identifiers — not remotely a substitute, so the
  hint actively misleads.
- **Severity:** Papercut.
- **Workaround:** `cargo add sha2 --dry-run` first, to print the real feature list.
- **Suggestion:** when the nearest feature name is far in edit distance, list all available
  features instead of offering the nearest string at any distance.

## F-009 — ed25519-dalek's safe verification is not the one you reach for
- **Task:** verify an Ed25519 signature over a canonical payload.
- **Steps:** the crate re-exports `Signer`/`Verifier` from its root, so the natural call is
  `verifying_key.verify(payload, &signature)`. Read `src/verifying.rs` before committing.
- **Expected:** the trait method is the safe default.
- **Actual:** `impl Verifier for VerifyingKey` (`verifying.rs:567`) → `multipart_verify` (`:578`) →
  `raw_verify` (`:211`), which performs **no** small-order check. Only the inherent
  `verify_strict` (`:367`) rejects a low-order public key and a low-order `R` (`:380`). The crate's
  own README states plainly at line 154 that `verify()` permits weak keys, and that a weak key
  yields a signature valid for *any* message with high probability; the `is_weak` rustdoc says the
  same. The documentation is honest — it is the ergonomics that point at the wrong function.
- **Severity:** Minor (minutes. The behaviour is documented, so what it cost was reading time
  during crate vetting. The security impact is far larger than the time lost, but this log's scale
  grades time lost, not blast radius.)
- **Workaround:** call `VerifyingKey::verify_strict` explicitly, and do not rely on the signature
  check alone — our verifier also requires the key to match the committed issuer list, which is
  the only route by which a weak key could reach us at all.
- **Suggestion:** in the next major version make strict the behaviour of `Verifier::verify` and
  rename the permissive path `verify_rfc8032_compatible`; failing that, put the weak-key warning
  on the trait impl's own rustdoc, where the person about to call it will see it.

## F-010 — `CARGO_BIN_EXE_<name>` is unavailable to a sibling crate's integration tests
- **Task:** have `verify-dataset`'s integration tests drive the real `sign-dataset` binary, the
  way a user would.
- **Steps:** added `sign-dataset` as a dev-dependency of `verify-dataset` and tried
  `env!("CARGO_BIN_EXE_sign-dataset")` from `tools/verify-dataset/tests/cli.rs`.
- **Expected:** cargo, which has already built that binary for the same workspace, exposes its
  path as it does for the owning package.
- **Actual:** the variable is only set for integration tests of the package that *contains* the
  binary. The alternatives are hardcoding a `target/` path — which breaks under a custom
  `CARGO_TARGET_DIR`, as this repo requires — or shelling out to cargo from a test.
- **Severity:** Papercut (it pushed us to a better design anyway).
- **Workaround:** moved the logic into a shared library crate so tests exercise it directly, and
  kept one thin end-to-end test per binary using its own `CARGO_BIN_EXE_*`.
- **Suggestion:** set `CARGO_BIN_EXE_<name>` for any binary target cargo built for the current
  test invocation, or add an opt-in `[package.metadata.test-binaries]` key.

## F-011 — `cargo test` hides failures in later test binaries
- **Task:** show a set of tests failing against pre-fix code across three packages in one run.
- **Steps:** removed the check under test, ran
  `cargo test -p dataset-manifest -p sign-dataset -p verify-dataset`.
- **Expected:** one run reports every failing test in the selected packages, the way a single test
  binary reports all of its own failures.
- **Actual:** the run stopped after the first test binary that failed
  (`error: test failed, to rerun pass …`), so two of the five expected failures — one in
  `sign-dataset`, one in `verify-dataset` — were never printed. Nothing in the output says that
  other binaries were skipped rather than passing.
- **Severity:** Papercut.
- **Workaround:** `cargo test --no-fail-fast …`, which reports all of them.
- **Suggestion:** when `cargo test` stops early, print a line naming the test binaries that were
  skipped ("3 test binaries not run"), so an absent failure cannot be read as a pass.

## F-012 — a cached `cargo clippy` looks identical to a real run at default verbosity
- **Task:** paste a trustworthy `cargo clippy --all-targets -- -D warnings` for a gate.
- **Steps:** ran clippy, fixed everything, ran it again after further edits to doc comments only.
- **Expected:** a gate that reports on the current source, or says plainly that it did not look
  at it.
- **Actual:** the second run printed only `Finished dev profile … in 0.32s` and exited 0, with no
  `Checking` line for any crate — the same exit code and a near-identical tail to a real clean
  run. At default verbosity, "clippy checked the code and found nothing" and "clippy reused a
  cached result and looked at nothing" are the same output. `-v` does distinguish them: it prints
  one `Fresh <crate>` line per package. But a gate is normally pasted from the default output,
  and there nothing says the work was skipped.
- **Severity:** Minor (minutes, spent establishing that the gate had actually looked at the code).
- **Workaround:** `touch` each crate root before the run so cargo re-checks, and confirm one
  `Checking <crate>` line per package appears; or run with `-v` and read the `Fresh` lines.
- **Suggestion:** say something at default verbosity when packages were served from cache. The
  information already exists one verbosity level down, so it is only hidden at the level people
  actually paste from; a `(N packages fresh)` note on the `Finished` line would be enough. A lint
  gate is the one place where silence about skipped work costs the most.

## F-013 — reaching rmcp's protocol-version check needs two headers, and neither error names the other
- **Task:** write an HTTP test proving that overriding `supported_protocol_versions` actually
  refuses a request naming a revision the server does not implement.
- **Steps:** POSTed `tools/list` to the Streamable HTTP endpoint with
  `_meta."io.modelcontextprotocol/protocolVersion": "2026-07-28"` plus the `Mcp-Method` header;
  then re-sent it with `MCP-Protocol-Version: 2026-07-28` as well; then read
  `src/transport/streamable_http_server/tower.rs:840-860` and `src/handler/server.rs:64-71` in the
  published crate to work out the rule.
- **Expected:** a request that names a revision in its own `_meta` reaches the server's version
  check and is answered `-32022`, the code the crate defines for exactly that (`model.rs:623`).
- **Actual:** the transport rejects it first, with `-32020` and a different message depending on
  which header is missing: "request _meta protocolVersion requires MCP-Protocol-Version header"
  without the version header, and "missing required Mcp-Method header" without the SEP-2243
  header. Neither message mentions the other requirement, so each fix reveals one more. Only with
  both headers present does the request reach the check and return `-32022`.
- **Severity:** Minor (about 20 minutes and two failed test runs).
- **Workaround:** send `MCP-Protocol-Version` and `Mcp-Method` together on any request whose
  `_meta` names a revision.
- **Suggestion:** when `_meta` names a protocol version, validate the whole header set once and
  list every missing header in one error; and document "headers a 2026-07-28 request must carry"
  in one place in the transport's rustdoc.

## F-014 — rmcp 3.5.1 documents the MCP Apps extension identifier but implements nothing behind it
- **Task:** decide whether to serve our evidence card as an MCP App
  (`io.modelcontextprotocol/ui`) or as a plain MCP resource.
- **Steps:** searched the published crate for `io.modelcontextprotocol/ui`, `ui://`, `mcp-app` and
  `mimeTypes`; compared what exists for the Tasks extension; read `src/model/capabilities.rs` end
  to end.
- **Expected:** either support for the extension (a builder, a capability predicate, some handling
  of the resource scheme) or no mention of it at all.
- **Actual:** the identifier appears as a doc-comment example at `src/model/capabilities.rs:15`,
  `:26`, `:181` and in that module's tests at `:600`, `:615`, `:646`, `:656`, together with the
  mime type `text/html;profile=mcp-app` at `:28` — which reads like support. There is no
  `enable_ui`/`enable_apps` beside `enable_tasks` (`:429`, `:460`), no `supports_ui` beside
  `supports_tasks` (`:200`, `:248`), and `grep -rn 'ui://' src/` returns zero. The extensions this
  SDK does implement are Tasks (`src/model/task.rs:13-14`, `src/task_manager.rs:1-2`) and OAuth
  client credentials, SEP-1046 (`src/transport/auth.rs:951-952`, with a test asserting the
  identifier at `:8589-8590`) — neither of which is Apps. Establishing that negative took a full
  pass over the crate, because the docs give an extension identifier without saying it is an
  example of the *shape* and not of support.
- **Severity:** Minor (about 40 minutes to prove a negative).
- **Workaround:** serve the card as an ordinary resource under a `ui://` URI with the documented
  mime type, and do not declare an extension whose negotiation we do not run.
- **Suggestion:** in the `extensions` capability docs, say which extensions this SDK implements
  (today: Tasks and OAuth client credentials) and mark the other identifiers as illustrative; a
  one-line table in the module docs would have answered this in seconds.

## F-015 — `cargo add` appends a bare dependency line to a manifest where every entry carries a comment
- **Task:** add three dependencies to a crate whose `Cargo.toml` documents why each dependency
  exists.
- **Steps:** `cargo add --path tools/dataset-manifest`, then `cargo add sha2@0.11.1
  --no-default-features`, then `cargo add serde@1.0.229 --features derive`.
- **Expected:** the new entries land where the curated list keeps them, and there is some way to
  attach the rationale cargo already made me think about.
- **Actual:** all three were appended as bare lines after the commented block, splitting the file
  into a documented section and an undocumented tail. Re-ordering and commenting them by hand
  afterwards is the only option, and it is the step most likely to be skipped under time pressure.
- **Severity:** Papercut.
- **Workaround:** edit the manifest by hand after every `cargo add`.
- **Suggestion:** a `cargo add --comment "<why>"` flag that writes the note above the new entry,
  and insertion that respects the existing ordering of the table.

## F-016 — a JSON integer too large for the target type is reported as a floating-point type error
- **Task:** prove that an out-of-range money amount in a signed record is refused, not rounded.
- **Steps:** deserialised `"attested_balance_minor": 170141183460469231731687303715884105727`
  into a struct whose field is `i64`, with serde_json 1.0.154.
- **Expected:** an error naming the real problem — an integer literal outside the range of the
  target type.
- **Actual:** ``invalid type: floating point `1.7014118346046923e+38`, expected i64``. The value is
  refused, which is the behaviour that matters, but the message says the document contained a
  float when it contained 39 digits. For anyone auditing a money path that is a misleading
  diagnosis: it suggests the source file is wrong about its own format rather than out of range.
- **Severity:** Papercut (one wrong test assertion, a couple of minutes).
- **Workaround:** assert on "expected i64" rather than on a range message, and keep the amount
  integral in the type so the refusal happens at all.
- **Suggestion:** when a JSON number literal has no fraction or exponent and overflows the target
  integer type, say so — "integer literal out of range for i64" — instead of reporting the f64 the
  parser fell back to.

## F-017 — `cargo test` gives no workspace-wide total
- **Task:** report how many tests actually ran across the workspace, not "tests pass".
- **Steps:** `cargo test` at the workspace root; read the output.
- **Expected:** one line giving the total number of tests run across all targets.
- **Actual:** thirteen separate `test result:` lines, one per target, with no aggregate — and a
  target that registered zero tests prints a line identical in shape to one that ran. The total
  has to be summed by hand to be trusted. `cargo-nextest`, which does print one, is not installed
  here.
- **Severity:** Papercut.
- **Workaround:** `cargo test 2>&1 | grep '^test result' | awk -F'[. ]' '{s+=$5} END {print s}'`.
- **Suggestion:** have `cargo test` print a final workspace summary line — `N tests run across M
  targets, K failed` — the way it prints `Finished` for a build. "Tests passed" is the claim
  people make from this output, and the number that would make it checkable is the one number
  cargo does not print.

## F-018 — a shared `tests/common` module forces a blanket `allow(dead_code)`
- **Task:** share one dataset-fixture helper between two integration-test binaries.
- **Steps:** `mod common;` in both `tests/mcp_over_http.rs` and `tests/signed_record.rs`; each
  uses the helpers its own tests need.
- **Expected:** a helper used by at least one test binary counts as used code.
- **Actual:** Rust compiles `tests/common/mod.rs` separately into every test binary, so helpers
  called only by the other binary are dead code there, and `cargo clippy -- -D warnings` fails.
  The only ways out are a crate-level `allow`, duplicating the helper, or publishing a support
  crate.
- **Severity:** Papercut.
- **Workaround:** `#![allow(dead_code)]` at the top of `tests/common/mod.rs`, with a comment
  naming why it is correct there.
- **Suggestion:** treat a `tests/` support module as used if any test target in the package uses
  it, or name the `allow` as the sanctioned pattern in the Cargo book's integration-test section.
  A lint that is always wrong in a supported layout teaches people to silence lints.

## F-019 — a probe script that hardcodes its build directory stops being re-runnable evidence
- **Task:** re-run an earlier set of HTTP probe scripts against a freshly built binary, to show
  that a defect they found is fixed.
- **Steps:** ran the scripts as they were, without editing them.
- **Expected:** each one takes the binary to exercise as an argument, as the smallest script in
  the set already did.
- **Actual:** the shared helper hardcoded one build directory, and only that smallest script
  accepted an override. The others could be re-run only by rebuilding into that exact directory
  or by editing them — and editing an evidence script destroys the thing that makes it evidence.
- **Severity:** Minor (minutes, and it nearly left a check unrun).
- **Workaround:** rebuild into the path the scripts expect, with `CARGO_TARGET_DIR` pointing
  there, and run them untouched.
- **Suggestion:** when a script is meant to be evidence somebody else re-runs, take every path it
  depends on as an overridable parameter with today's value as the default. It costs one line per
  path and it is the difference between a result that can be reproduced and one that has to be
  trusted.

## F-020 — nothing warns you not to give a trust-bearing MCP tool a free-text argument
- **Task:** build `verify_claim`, a tool that checks a statement about a building project
  against a signed record and answers `SUPPORTED`, `CONTRADICTED` or `UNVERIFIED`. The obvious
  first schema is the one a person would write: one string argument, the claim.
- **Steps:** declared `verify_claim(claim: string)` and implemented a parser in Rust over a
  deliberately closed set of claim shapes — unit × instalment × status, certificate × status,
  milestone × completion — with whole-word matching, explicit negation handling, and the
  predicate vocabulary stripped before the subject is resolved. Hardened it twice against an
  adversarial corpus of 173 labelled claims.
- **Expected:** a bounded grammar over a record of 3 units, 5 certificates and 10 milestones is
  small enough to get right, and the verdict would be trustworthy.
- **Actual:** after two rounds it answered `SUPPORTED` for **101 of 140 false claims** and
  `CONTRADICTED` for **18 of 31 true ones**. Each round fixed every named case and left the
  class untouched. The failures were not exotic: a negation attached to the second clause
  inverted the first, a sentence whose extra clause was ignored passed on the clause that
  remained, `1.47m` parsed as 1.47, and `1,470,000.00 USD` was accepted as the same figure in
  dirhams. The tool's own description promised a verdict only where a field had been read and
  compared, so an agent following our Agent Skill would have stated all 101 as fact, each with a
  citation whose signature genuinely verified. **A correct signature under a misread sentence is
  worse than no verification at all**, because it looks like proof.
- **Severity:** Major (two full rebuild rounds across two days).
- **Workaround:** the language left the server. `verify_claim` now takes
  `subject_type`, `subject_id`, `field` and `asserted_value` with closed enums in the input
  schema; money is integer minor units, dates are ISO-8601, and the comparison is exact. The
  calling model turns language into arguments, which is what MCP tool schemas are for, and the
  parser was deleted rather than kept as a fallback.
- **Suggestion:** say this in the tool-schema guidance, because the default pull is the other
  way — a free-text argument reads as the friendlier API, and a server-side parser feels like
  being helpful. The rule worth writing down is that **a tool whose result a model will treat as
  evidence must take structured arguments and compare, never interpret**; put the normalisation
  burden on the model, which is the component that is actually good at language. The SDK ships
  no examples at all, so there is nothing to copy in either direction; one worked example of a
  verification tool with closed enums, next to the counter-example of the same tool taking a
  sentence, would have saved us both rounds.
  The test that would have caught it earlier is cheap and worth naming too: **if a component's
  defects can only be enumerated by a corpus, it does not belong inside a thing whose whole
  value is that it cannot be talked into lying.**

## F-021 — an MCP SDK does not enforce the input schema it advertises
- **Task:** make a bad tool call fail at the schema, before it can reach a verdict.
- **Steps:** declared `enum`, `additionalProperties: false` and an `allOf`/`if`/`then` block in
  the tool's `inputSchema`, then called it with `subject_type: "escrow"`, which the enum does
  not contain.
- **Expected:** the SDK validates `arguments` against the schema the server itself declared, or
  says plainly that it does not.
- **Actual:** the arguments reached the handler unchecked. The declared schema is advertisement
  to clients and nothing more. Nothing in the type signature hints at this: a handler receives
  the arguments as free-form JSON either way.
- **Severity:** Minor (about half an hour, caught on the first schema test).
- **Workaround:** validate every closed enum server-side and answer `-32602`, and treat the
  schema purely as the client-facing contract the server never trusts.
- **Suggestion:** either validate against the declared schema by default, or say in the
  `inputSchema` documentation that it is not enforced. A schema that is part of a safety
  argument needs to say *where* it is enforced; "closed enum in the schema" is a statement about
  clients, not about the server.

## F-022 — a Streamable HTTP test read another request's answer and passed
- **Task:** prove that abandoning a `whats_changed` call costs the caller nothing.
- **Steps:** abandoned a POST after 1 ms on a shared session, then called again on the same
  session. Every request in the file reused `"id": 7`.
- **Expected:** each POST's body is the answer to that POST.
- **Actual:** Streamable HTTP may deliver a queued message on a later POST's stream, so under
  load the follow-up call read the *abandoned* call's answer — a body that looked almost right.
  The test passed when run alone and failed twice in the full suite.
- **Severity:** Minor (about fifty minutes, and two full suite runs to localise).
- **Workaround:** a unique JSON-RPC id per request, an assertion that the reply's id matches the
  request's, and a separate client, connection and session for the abandoning caller.
- **Suggestion:** in any MCP-over-HTTP client helper, match reply to request by id rather than
  by arrival order, and say so in the transport documentation. Reusing an id is the obvious
  shortcut in a test file, and it fails silently and intermittently, which is the worst way for
  it to fail.

## F-023 — nothing tells you a dependency stopped being used
- **Task:** make one tool stateless, then leave the manifest honest.
- **Steps:** deleted the per-caller state module; the type that hashed the bearer token then had
  no consumer, which in turn left `sha2` unused. Checked by hand with `grep`.
- **Expected:** the build, or a lint, mentions a dependency nothing imports any more.
- **Actual:** nothing does. `cargo build` and `cargo clippy -- -D warnings` are both perfectly
  happy to compile and link a dependency with no remaining users, so an unused crate stays in
  the manifest, the lock file and the supply chain until somebody greps for it. The tools that
  would catch it, `cargo-machete` and `cargo-udeps`, are separate installs.
- **Severity:** Minor (about twenty minutes, most of it verifying the sweep by hand).
- **Workaround:** grep every dependency against the source after any deletion that removes a
  type, and treat a stale "why this dependency is here" comment as a signal that the dependency
  itself may be stale.
- **Suggestion:** warn on a dependency with no reachable use, at least behind a flag. Supply
  chain is the argument people make for small dependency lists, and the default toolchain
  currently gives no way to notice that a list has stopped being small for a reason.

## F-024 — there is no supported way to build a previous commit for a before-and-after
- **Task:** show two regression tests failing on the previous commit and passing on this one.
- **Steps:** the new tests cannot run against the old code, because the tool signatures changed;
  so the evidence had to come from running a probe against both binaries.
- **Expected:** one command to materialise and build the previous commit read-only.
- **Actual:** with read-only git there is no checkout, worktree, archive or clone available, so
  there is no supported way to produce the earlier tree at all. The only pre-fix binary that
  existed was one an earlier run had happened to leave behind; without that accident there would
  have been no before-and-after to show.
- **Severity:** Minor (about twenty-five minutes deciding how to produce honest evidence).
- **Workaround:** a curl-only probe script that takes the binary as its first argument, run
  against the old binary and the new one in turn.
- **Suggestion:** keep one built binary per reviewed commit deliberately rather than by
  accident, and give every repro probe a binary argument. "It failed before" is the single most
  load-bearing claim in a fix report, and it is the one most likely to rest on a build nobody
  can reproduce.

## F-025 — a Rust test that loops over cases stops measuring at the first failure
- **Task:** run one table of assertions twice, once per identity, and report every defect in both
  runs.
- **Steps:** built the cases, looped over the two runs, and asserted per run inside the loop —
  first that the run found no defects, then that it had executed enough probes to be meaningful.
- **Expected:** a table advertised as covering both identities reports both identities' defects.
- **Actual:** the first run's probe-count assertion failed, which ends the test, so the second
  identity's defects were never evaluated at all. The output said "1 failed" and named a probe
  count — nothing suggested that half the table had not been looked at. Raising the count
  surfaced 347 previously unexamined rows in one go. Rust's standard test harness has no
  parametrised cases and no soft assertion: `assert!` ends the whole test, so a loop over cases
  silently measures a prefix, and the shorter that prefix is the greener the suite looks.
- **Severity:** Major (three runs of several minutes each to work out that the gate had been
  measuring a subset rather than failing honestly).
- **Workaround:** collect every case's defects into one collection and assert once, after the
  loop, so a failure cannot truncate the measurement; and assert the cheap invariant before the
  expensive one.
- **Suggestion:** give the built-in harness either parametrised cases that each report
  separately, as several other ecosystems have, or a soft-assert that records and continues.
  The failure mode is not a wrong answer, it is a gate quietly covering less than it claims,
  which is the kind of defect a green suite is supposed to rule out.

## F-026 — an MCP SDK's client types cannot be constructed the way its docs show
- **Task:** write an MCP client that calls `tools/call` and `resources/read` against our own
  server.
- **Steps:** built the request parameter structs as struct literals with
  `..Default::default()`, matched exhaustively on the resource-contents enum, and handled the
  error type the service's `cancel()` appeared to return.
- **Expected:** the shapes the crate's own documentation shows are constructible from outside
  the crate.
- **Actual:** four separate compile failures. The request parameter types and the
  resource-contents enum are `#[non_exhaustive]`, which makes both struct-literal construction
  and exhaustive matching illegal downstream, and the running service's `cancel()` returns a
  join error rather than the service error the surrounding code was written for. None of this
  is visible from the type signatures as rendered; it is visible only as four errors in a row.
- **Severity:** Minor (about forty minutes).
- **Workaround:** the `::new(..)` builders with `.with_arguments(..)`, a wildcard match arm, and
  a separate branch for the join error.
- **Suggestion:** ship one client example that goes `initialize` → `tools/list` → `tools/call` →
  `resources/read`. The crate documents the transport constructor but never shows a request
  being built, which is the only part a caller cannot guess, and `#[non_exhaustive]` means the
  obvious guess cannot compile.

## F-027 — a spawned program's path is resolved after the working-directory change
- **Task:** start a server binary from an integration test that needs a different working
  directory, so the server resolves its data paths the way it does in production.
- **Steps:** set the program to a relative path and the working directory to the repository
  root in the same command builder, having first confirmed the binary existed at that path.
- **Expected:** the path is resolved from where the test is, since that is where it was just
  checked.
- **Actual:** `No such file or directory`, moments after a successful existence check on the
  same string. POSIX applies the directory change *before* resolving the program, so the
  relative path was resolved against the new directory and pointed outside the repository
  altogether.
- **Severity:** Minor (about twenty minutes, most of it disbelieving the existence check).
- **Workaround:** canonicalise the program path before spawning.
- **Suggestion:** say in the documentation for the working-directory setter that it changes how
  the program path itself resolves. The failure is silent, the error names a file that does
  exist, and the resolved path can escape the directory the caller believed it was working in.

## F-028 — a supply-chain gate with no configuration rejects every dependency, including MIT
- **Task:** run a licence and advisory check over a new dependency set.
- **Steps:** ran the checker with no configuration file present.
- **Expected:** a signal — advisories, bans, sources and licences each either clean or naming
  something real.
- **Actual:** advisories, bans and sources came back clean; licences **failed with one rejection
  per crate**, 131 of them, including plain `MIT`. With no configuration the default allow-list
  is empty, so the gate rejects everything and reads exactly like a catastrophic finding. The
  real spread is entirely permissive.
- **Severity:** Papercut.
- **Workaround:** run the other three checks by name and read licences separately until a
  configuration file exists.
- **Suggestion:** when no configuration is found, say so in the first line of output and skip
  the licence check rather than failing it. A gate whose default state is "everything is
  rejected" trains people to ignore it, which is the opposite of what a supply-chain tool wants.

## F-029 — the dry run that is supposed to cost nothing also tells you nothing
- **Task:** find out how many transitive packages a large cloud SDK would add, *before* deciding
  whether to add it.
- **Steps:** `cargo add --dry-run` with the three SDK crates.
- **Expected:** the package count, which is the single number the decision turns on.
- **Actual:** it resolved the three named crates, printed `aborting add due to dry run`, and
  stopped before resolving the graph. No transitive count is printed at all, because the count
  is only reported when the manifest and lock are actually written.
- **Severity:** Papercut.
- **Workaround:** none that does not write a manifest and a lock file — so measuring the cost
  requires paying it, and the only containment is to do it in a scratch directory.
- **Suggestion:** have the dry run resolve and report the count it declines to write. "How much
  does this dependency cost me?" is the main reason to ask for a dry run, and it is the one
  question the dry run cannot answer.

## F-030 — one optional feature silently raised the toolchain floor by six releases
- **Task:** add an optional `live` feature pulling three AWS SDK crates, with the default build
  staying on the toolchain the README asks for.
- **Steps:** `cargo build` (fine), then `cargo build --features live`.
- **Expected:** if a dependency needs a newer compiler, to be told that when I add it — the
  manifest declares `rust-version`, so the information exists before anything is compiled.
- **Actual:** the default build works on 1.88; enabling the feature requires 1.94.1, because one
  transitive SDK crate declares that MSRV. Nothing says so at add time. You find out from a
  build failure, and only if your toolchain is older — on a current toolchain it passes locally
  and fails for whoever follows the README.
- **Severity:** Papercut, with a sharp edge: the person who hits it is never the person who
  caused it.
- **Workaround:** read `rust-version` out of the vendored manifests by hand and write the real
  floor into the README, per feature.
- **Suggestion:** report the resolved MSRV of the graph when a feature is enabled, or at minimum
  have `cargo add --features` print the highest `rust-version` it just pulled in. A workspace
  has one toolchain but features give it several floors, and only one of them is documented.

## F-031 — the default TLS stack for a cloud SDK is a C toolchain
- **Task:** add three AWS SDK crates to a pure-Rust project with no system dependencies, and
  keep it that way.
- **Steps:** add the SDKs with default features; inspect `cargo tree`.
- **Expected:** rustls, which the ecosystem already standardises on.
- **Actual:** the default HTTPS client pulls `aws-lc-rs` and `aws-lc-sys`, which bring a C
  library, a cmake build and over 11,000 `unsafe` blocks into a project that had none of those.
  It is reachable without ever naming a TLS crate, and nothing in the quickstart mentions it.
- **Severity:** Annoying — it changes what the project needs installed, which is a README claim.
- **Workaround:** depend on `aws-smithy-http-client` explicitly, select
  `tls::Provider::Rustls(CryptoMode::Ring)`, and pass the client to every SDK config; then keep
  a check that greps `cargo tree` for `aws-lc`, because a later default can quietly put it back.
  Being exact about the win: `ring` compiles C and assembly of its own and still needs a C
  compiler, so this is not "no C". What it drops is cmake and a vendored copy of a whole TLS
  library, and 11,000-odd `unsafe` blocks.
- **Suggestion:** say in the getting-started docs which TLS implementation the default selects
  and what it costs, and how to choose the other one. A dependency that adds a C toolchain
  should never arrive unannounced.

## F-032 — opting out of the default HTTP client turns one provider into a startup panic
- **Task:** having chosen the HTTP client explicitly (F-031), start the app with the live
  feature and a profile.
- **Steps:** set the client on the config loader with `.http_client(...)` before `.load()`, so
  every provider the loader builds inherits it. Build the region chain with the SDK's own
  `RegionProviderChain::default_provider()`. Run the binary.
- **Expected:** either it works, or a provider that cannot be built says so and the chain moves
  on — a chain of fallbacks exists precisely so that one unavailable source is not fatal.
- **Actual:** `panicked at .../orchestrator/operation.rs:425: a http_client is required. Enable
  the default-https-client crate feature or configure an HTTP client to fix this.` Before any
  network call, before the port is bound. The region chain is built *by the caller*, not by the
  loader, so it never receives the loader's client; its instance-metadata leg then constructs a
  request with no client at all. The advice in the message is the opposite of what the code
  wants — the whole point was not to enable `default-https-client`, and doing so would have
  pulled the C toolchain back in.
- **Severity:** Blocking, and expensive to diagnose: the panic names a file in the runtime
  crate, not the line of user code that built the provider, and it is invisible until the first
  run on a machine with no `AWS_REGION` set.
- **Workaround:** stop using the default region chain. Env var, then profile file, then a
  constant — three providers that read variables and files and never need a client. On a laptop
  the metadata leg could only have timed out anyway.
- **Suggestion:** a provider built without an HTTP client should fail as a `Result` the chain
  can skip, not a panic; a chain of fallbacks that aborts the process on its third fallback is
  not a fallback chain. Failing that, make the message name the provider and say "this provider
  was constructed outside a loader", because that is the actual mistake and nothing in the
  current text points at it.

## F-033 — the SDK logs an access key id at INFO, into a terminal being recorded
- **Task:** run the app live for a demo recording, with ordinary `INFO` logging.
- **Steps:** `tracing_subscriber::fmt().with_max_level(Level::INFO).init()`, then start with a
  profile.
- **Expected:** INFO to be the level you can safely leave on. Credentials at `debug` or `trace`
  would already be arguable; at INFO they are in every default run.
- **Actual:** the first line of the log was the resolved **access key id**, emitted by
  `aws_config` while building the credentials provider. The app had been written specifically so
  that it never reads or holds a credential — and a dependency logged one anyway, at the level
  everybody runs. For a hackathon this is a recorded terminal in a video; elsewhere it is a key
  id in log aggregation, which is half of a credential and all of an identifier.
- **Severity:** Major. It is silent, it is default-on, and the application code that would
  have to be "fixed" is code that is already doing the right thing.
- **Workaround:** hold the SDK's own targets at `warn` — `info,aws_config=warn,...` — and do
  **not** honour `RUST_LOG`, because honouring it hands the leak back to anyone who exported it.
  Note that a directive like `aws_sdk=warn` does not do what it looks like: targets are matched
  on `::` boundaries, so it never matches `aws_sdk_bedrockruntime`. Each crate must be named.
- **Suggestion:** do not put credential material in a log event at INFO. If the key id is
  genuinely useful for support, it belongs at `debug` behind an explicit opt-in, and the
  getting-started docs should say which targets a production logger ought to quieten — because
  the current answer is "all of them, by name, and you have to know they exist".

## F-034 — a model quota of zero, not adjustable, found only by using it
- **Task:** run the hosted tool loop against Amazon Nova Pro on Bedrock for the demo.
- **Steps:** one Converse call, then the rest of the loop.
- **Expected:** either a working quota, or a refusal at the first call naming the limit.
- **Actual:** the first Converse call succeeded: a one-message probe from the AWS CLI
  (`aws bedrock-runtime converse`), not through this repository's code. Every call after it returned
  `ThrottlingException: Too many tokens per day`, on Nova Pro and on Nova Lite and Nova Micro,
  the last two of which had never been called at all. Service Quotas in `us-east-1` shows the
  applied value for Nova Pro tokens per day, tokens per minute and requests per minute as **0**,
  marked **Not adjustable**, so the console offers no increase to request. The only route is a
  support case, filed 2026-10-21.
- **Severity:** Critical. It blocked the live tool loop for the whole demo window.
- **Workaround:** none within the account. The demo runs on the scripted stub model, which
  exercises the same tool-call path; the hosted voice was split onto its own `--live-voice`
  switch so a spoken answer did not depend on the blocked model.
- **Suggestion:** an applied quota of zero on a model the console offers is indistinguishable
  from a broken integration until you read Service Quotas. Say it at the first call — a refusal
  naming the quota costs nothing and would have replaced a day of diagnosis. And a quota of
  zero marked "Not adjustable", with a support case as the only path, should be stated on the
  model's enablement page, not discovered after enabling it.

## F-035 — a browser sign-in that fails with 400 and no diagnostics
- **Task:** sign in to AWS from the CLI to use a profile.
- **Steps:** `aws login --profile <p> --region us-east-1`, then the same with `--remote`.
- **Expected:** a sign-in page, or an error naming what is wrong.
- **Actual:** the browser shows a bare `400 Bad Request`. No code, no request id, nothing on
  the terminal. A cookieless `curl` of the same authorize URL returns `302`, so the URL itself
  is well formed and the failure depends on browser state that nothing reports.
- **Severity:** Blocking for that route.
- **Workaround:** abandon it; create a dedicated least-privilege IAM user and use static keys
  in a named profile.
- **Suggestion:** return a diagnosable error. A 400 with no body on an interactive sign-in
  leaves the user with nothing to search for, and the one thing they can easily try — the same
  URL outside the browser — succeeds, which points away from the actual cause.

## F-036 — an environment variable silently overrides the configured profile
- **Task:** make a Bedrock call signed by the profile the binary was told to use.
- **Steps:** configure the client from a named profile; have `AWS_BEARER_TOKEN_BEDROCK`
  exported in the environment.
- **Expected:** the configured profile to be used, or a refusal if a conflicting credential is
  present.
- **Actual:** `aws-sdk-bedrockruntime` switches the client's auth scheme preference to
  `httpBearerAuth`, so the call is signed with the token's identity instead of the profile's.
  Nothing is reported at `INFO` or `WARN` — the SDK does trace auth-scheme order at `TRACE`,
  which is not a level anyone runs a demo at — and the profile is still configured and still
  ignored. The same shape as the environment access key the credentials chain prefers, reached
  through a different door: it is an auth-scheme decision rather than a credentials one, so
  pinning the credentials provider does not close it.
- **Severity:** Major. It is silent at the levels people run, it is default-on, and it changes
  which identity a request is made as.
- **Workaround:** build the service config explicitly rather than from the shared config, and
  pin the auth scheme preference to SigV4. That one line is the whole of it: calling
  `set_token_provider(None)` as well looks like a second defence and is not one, because in
  1.148.0 that setter only acts on `Some`. Measured rather than assumed — with the token
  cleared and the preference left alone, the config still resolves to `httpBearerAuth`. Polly
  and Transcribe need nothing: their generated config builders have no `set_token_provider` at
  all.
- **Suggestion:** an environment variable that changes the signing identity of a client built
  from an explicit profile should log that it did so, or refuse when both are present. Silently
  preferring the ambient one is the wrong default for a credential.
