# Ask Aunova — the idea

**One line:** ask a building project a question out loud and get an answer that proves itself:
every fact cites a signed source document, the server checks that signature before answering,
and what you are allowed to hear depends on who you are.

**Why it matters.** Off-plan buyers are sold on claims (progress, certificates, escrow status) they
cannot check. Developers hold the evidence but have no safe way to expose it. An assistant that only
repeats a database is a chatbot; an assistant that **refuses to state what it cannot verify** is
trust infrastructure. *AI is the interface; the signed record is the product.*

All data is **synthetic**: one fictional project, "Palm Vista Tower", fictional developer, fictional
issuers, every document watermarked `SYNTHETIC DEMO DATA`. No real project, regulator or authority
is named or implied.

## What is built

```
 Voice / text  ──►  "Alexa+ simulator" web page  ──►  Ask Aunova MCP server (Rust)
 (browser mic)      real MCP client: initialize,       Streamable HTTP, spec 2025-11-25
                    tools/list, tools/call              │
                    + Bedrock agent loop                ├─ role from bearer token (server-side)
                    + spoken answer                     ├─ signed project record (JSON + docs)
                    + evidence card (MCP App)           └─ per-caller "since you last asked" state
 Any MCP client (Claude Desktop, omp, Kiro) ──► same server         (bonus in video)
 Agent Skill (SKILL.md) ──► teaches any agent how to use the server honestly
```

1. **MCP server, Rust** (`rmcp` 3.5.1 — verified it negotiates `2025-11-25`, Streamable HTTP).
2. **Agent Skill** — an open-standard `SKILL.md` "ask-aunova": when to call which tool, and the
   rule that an answer without a verified citation is spoken as "unverified".
3. **Alexa+ simulator** — single web page, the FAQ-sanctioned path: real MCP client, push-to-talk,
   spoken reply, card rendering. Clearly labelled a simulator; no Alexa branding.

## Tools (tight, 5 + 1)

| Tool | Returns | Role effect |
|---|---|---|
| `get_milestones` | milestone, status, date, evidence refs | buyer: public milestones; developer: + contractor notes |
| `get_certificates` | certificate, fictional issuer, date, **signature check result** | buyer: issued certs; developer: + pending/expired |
| `get_payment_status(unit)` | instalments, escrow state | buyer: **own unit only**; developer: any unit |
| `verify_claim(subject_type, subject_id, field, asserted_value)` | the named field, compared exactly: `SUPPORTED`, `CONTRADICTED`, or `UNVERIFIED` with a reason | same verifier for both; a unit outside the caller's scope is refused |
| `ask_project(topic, unit?)` | answer composed only from verified facts + citations | scoped by role; a unit outside scope is refused, never substituted |
| `whats_changed(since_cursor?)` | changes after the cursor, plus `next_cursor` (state across sessions, held by the client) | scoped by role |

## What lifts it out of the judges' "obvious" bucket

The rules name "single-turn Q&A bot, basic MCP wrapper around an existing API" as obvious, and
"state across sessions, cards, MCP Apps, Agent Skills, agentic workflow" as creative. Mapping:

1. **Verification is executed, not narrated.** Each source document is content-hashed and
   Ed25519-signed by a fictional issuer key in the dataset. `verify_claim` and `get_certificates`
   recompute the hash and check the signature on every call. **Demo beat:** tamper one byte of a
   certificate on camera → the same voice question now answers "I can't verify that certificate."
2. **Role enforced by the server, not the prompt.** Role comes from the bearer token; the client
   never sends a role. **Demo beat:** buyer asks for another buyer's unit → refused by the server;
   developer asks the same → answered. Show the raw `tools/call` JSON once.
3. **State across sessions.** `whats_changed` keeps a per-token cursor: "Since you last asked on
   Tuesday, the façade milestone completed; evidence: site report SR-07, signed by the fictional
   engineer."
4. **Evidence card as an MCP App** (`ui://` resource) — the citation rendered as a card with
   issuer, date, hash, signature status. Media support is explicitly named as creative.
5. **Agent Skill** shipped alongside the server.
6. **Agentic loop** — the simulator runs a Bedrock model that chooses tools across turns
   (e.g. a compound question → milestones + certificates + verify).

## AWS Builder mini-challenge fit

- Amazon **Bedrock** runs the agent loop in the simulator (tool selection + answer composition).
- Hosting: **AWS Lambda** for the MCP server (FAQ recommends it) — or skip hosting entirely
  (FAQ: locally runnable repo is enough). Decide by Wednesday based on friction.
- Amazon **Polly** speaks every answer (owner decision D4).
- Document every integration in README + Product Feedback answer (required for this mini-challenge).
- AWS profile on this workstation: always `aunova`, never `[default]` (that is Quilibrium).

## Open Source mini-challenge fit

Main repo public under Apache-2.0, plus one **upstream contribution** born from the friction log
(e.g. an `rmcp` example: "Streamable HTTP server on Lambda", or a fix we hit). We can only *win*
one mini-challenge; enter both, lead with whichever is stronger by Thursday.

## Out of scope

Real ATI data, real names, real regulators/authorities; production auth (two demo tokens only);
multi-project; anything not in the video. No claim, anywhere in code, docs or video, that any
regulator or authority approves, endorses or works with this project.
