# Ask Aunova — build plan (start Mon 2026-10-19, submit Fri 2026-10-23)

Hard deadline **Fri 23 Oct 23:00 Dubai**. Internal target: **submitted by Fri 18:00 Dubai**.
Rules digest: `docs/RULES-CHECK.md`. Concept: `docs/IDEA.md`. Owner decisions: `docs/DECISIONS.md`.

**Rule for the whole week:** every papercut goes into `FRICTION_LOG.md` the moment it happens,
in the required shape. It is worth up to 10% of the score and cannot be reconstructed on Friday.

Separate adversarial review before submit. Every task below has a done-when; nothing is "done"
without it.

## Day 0 — Sun 18 Oct (done)

- [x] Rules verified against live Devpost; PRD corrections recorded (`docs/RULES-CHECK.md`).
- [x] Idea sharpened past the "obvious" tier (`docs/IDEA.md`).
- [x] Repo created locally (`~/Work/ask-aunova`, Apache-2.0).
- [x] Owner answered D1–D5 (`docs/DECISIONS.md`): entrant Aunova Technologies Ltd, repo private
      under `aunova` until Friday, brand Ask Aunova / ATI, Chris asks + Polly answers, no credit form.
- [x] Private remote `aunova/ask-aunova` created and `main` pushed (Mon 19 Oct, agent, on owner ask).
- [x] Owner registered on Devpost (Mon 19 Oct; deadline shown Oct 23 08:00 GMT-11 = 23:00 Dubai).
- [ ] Owner, Monday: `aws login --profile aunova` (session expired), then an agent runs a one-token
      Bedrock `converse` probe so a model-access/use-case-form delay surfaces today, not Wednesday.

## Day 1 — Mon 19 Oct: data + server core

1. **Synthetic dataset** `data/palm-vista/`
   - project.json: 10 milestones, 5 certificates (fictional issuers), payment schedules for 3 units
     (1204, 0802, 1510), ~12 source docs (markdown/PDF stubs, each watermarked `SYNTHETIC DEMO DATA`).
   - `tools/sign-dataset`: sha256 every doc, Ed25519-sign with fictional issuer keys, write
     `manifest.json` (doc id, hash, issuer pubkey, signature). Private keys stay out of git; the
     committed manifest + pubkeys are enough to verify.
   - Done-when: `verify-dataset` exits 0 on the clean set and **exits non-zero naming the doc after
     one byte is flipped** (run both, paste output into the PR).
2. **Rust MCP server skeleton** `server/` — `rmcp` 3.5.1, Streamable HTTP on `/mcp`, protocol `2025-11-25`.
   - Bearer token → role (`buyer:1204`, `developer`) resolved server-side; unknown token = 401.
   - Done-when: `curl` initialize returns `protocolVersion: "2025-11-25"`; `tools/list` lists the
     tools; a request with no token is 401. Scripted in `scripts/smoke.sh`.
3. **Friction log** started; first entries from rmcp/MCP spec reading.

## Day 2 — Tue 20 Oct: tools + verification + state

4. Implement `get_milestones`, `get_certificates`, `get_payment_status`, `verify_claim`,
   `ask_project`, `whats_changed`, all role-filtered **in the server**.
   - Done-when (integration tests, real HTTP): buyer token on unit 0802 → refusal; developer → data;
     tampered doc → `get_certificates` marks it `signature_invalid` and `verify_claim` returns
     `UNVERIFIED`; `whats_changed` returns changes once, then nothing on the second call.
5. **Evidence card** as MCP App `ui://` resource. First hour: confirm whether `rmcp` 3.5.1
   exposes the MCP Apps extension; if not, serve the HTML resource as a plain resource and render
   it in our simulator (log the gap as friction).
6. **Agent Skill** `skill/ask-aunova/SKILL.md` (open Agent Skills format).
   - Done-when: loaded in one real agent (omp or Claude Desktop) against the local server and a
     question answered with a citation — screenshot kept for the video.

## Day 3 — Wed 21 Oct: simulator + AWS

7. **Alexa+ simulator page** `simulator/` — real MCP client over Streamable HTTP, push-to-talk
   (Web Speech API), spoken answer, card panel, role switcher (swaps the token, nothing else).
   - Done-when: in a real browser, voice question → `tools/call` visible in a debug drawer →
     spoken answer + card. Recorded once as a rehearsal clip.
8. **Bedrock agent loop** in the simulator backend (AWS profile `aunova`): model picks tools,
   composes answer only from tool output.
   - Done-when: compound question triggers ≥2 tool calls in one turn, logged.
9. **Polly** voice for the answers (D4): server-independent, called from the simulator backend
   with the `aunova` profile. Done-when: one answer synthesised to audio and played in the page.
10. Hosting decision: Lambda deploy *or* local-only (FAQ allows local). Do not spend more than
    3 hours on hosting; log every snag.

## Day 4 — Thu 22 Oct: end-to-end, docs, video

11. README: what it is, 3-command setup, **where MCP 2025-11-25 and Streamable HTTP are
    implemented (file paths)** for the AI-assisted Stage 1 screen, AWS services used, licence.
12. Adversarial review (separate reviewer): fresh clone → follow README → all smoke and
    integration tests pass → tamper demo works. PASS required before recording.
13. Video script (<3:00): 0:00–0:25 problem; 0:25–1:35 live demo (buyer asks progress →
    certificate → tries other unit → refused; developer asks same → full answer; tamper → unverified;
    "what changed since last time"); 1:35–2:15 architecture + signed record; 2:15–2:45 same server in
    a second MCP client + Agent Skill; 2:45–2:55 close. No Alexa logos, no unlicensed music.
14. Record (D4, updated 2026-10-21): generated from screenshots of the real page; questions typed
    on screen; answers are Polly audio from the simulator; narration is generated speech.

## Day 5 — Fri 23 Oct: ship

15. Edit to ≤ 2:55. **OWNER APPROVAL:** the public YouTube/Vimeo upload is a public post.
    Generated video (owner, 2026-10-21): screenshots of the real page + Polly answer audio +
    NanoGPT TTS narration. Keep the page's `#about` line (model · voice · transcriber) in EVERY
    demo frame, or burn it in as a fixed caption, and have the narration say once which model
    drove the recording (the scripted stub, while Nova is quota-blocked). Record the TTS model
    and its licence next to the video. Prefer a permissive model (e.g. Kokoro-82M, Apache-2.0);
    if it is an OpenAI voice, the YouTube description must say the voice is AI-generated.
16. Product feedback + friction log + feature requests finalised.
17. Repo per D2: public with licence visible in About, **or** private shared with
    `testing@devpost.com` + the six Amazon GitHub users (`docs/RULES-CHECK.md`). Making it public
    is **OWNER APPROVAL**. The upstream `rmcp` PR (Open Source mini-challenge) must fix a real
    friction-log item; a README/typo PR is the judges' "obvious" tier. Opening it is also
    **OWNER APPROVAL** (public post).
    **Before either option, clean the history.** Force-pushing an orphan over this repo is NOT
    enough: GitHub keeps force-pushed-away commits reachable by SHA until Support garbage-collects
    them, so making the same repo public still exposes early commits with working references.
    Instead: (a) rename the current repo to private `aunova/ask-aunova-dev` (history kept);
    (a2) BEFORE (b), repoint every clone: `git remote set-url origin
    https://github.com/aunova/ask-aunova-dev.git` in this checkout (its worktrees share the config;
    this laptop is the only machine with a clone, owner 2026-10-21).
    Once a new repo takes the old name, GitHub's rename redirect stops, and pushing any new branch
    from an un-repointed clone would upload the whole old history into the public repo;
    (b) create a fresh `aunova/ask-aunova` and push ONLY the `publish` orphan commit to it FROM A
    SEPARATE THROWAWAY CLONE, so the URLs in README/Devpost stay the same; (c) make the fresh repo
    public (OWNER APPROVAL).
    Done-when: the sweep grep over the publish tree returns nothing; the fresh remote has exactly
    one commit; `gh api repos/aunova/ask-aunova/commits/855fad0773b293db5fbe3f98b64db56df8a43160`
    returns 404 AND the same call against `repos/aunova/ask-aunova-dev/...` returns 200 (the
    control, which proves the probe works).
18. **OWNER APPROVAL: submit on Devpost by 18:00 Dubai.** Agents stage every field; the owner
    presses submit. Screenshot the confirmation.

## Risks

| Risk | Mitigation |
|---|---|
| rmcp lacks MCP Apps support | render card in our simulator; log friction |
| Web Speech API mishears "Palm Vista", "twelve oh four" | push-to-talk + on-screen transcript; type fallback in demo |
| Bedrock model/region access delay | request access Monday morning; fallback deterministic router, logged |
| Lambda + Streamable HTTP friction | FAQ: local-only is acceptable; cap at 3 h |
| Scope creep | anything not in the video script is cut |
