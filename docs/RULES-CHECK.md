# Rules check — Build, Ship, Shape: Amazon Developer Hackathon (Alexa+ track)

Read on 2026-10-18 from the live Devpost pages (`/rules`, `/details/faqs`, `/resources`, overview).
Every line below is quoted or paraphrased from those pages. Re-check `/rules` before submitting:
it carries a "September 16, 2026 UPDATE" banner, so it has changed once already.

## Can we enter? Yes

- Open to individuals of age of majority, Teams, and Organizations that exist at time of entry.
- Excluded jurisdictions named: Brazil, Quebec, Russia, Crimea, Cuba, Iran, North Korea, plus
  OFAC-sanctioned countries. UAE and Estonia are not excluded.
- Excluded: Amazon/Devpost employees and affiliates, judges, and projects built with financial or
  preferential support from Amazon. AWS Activate credits we already hold are standard promotional
  credits, not project funding. **[INFERENCE] — confirm with a written clarification request if in doubt.**

## Dates

| What | When |
|---|---|
| Submission deadline | **Fri 2026-10-23, 23:00 GMT+4** (= 12:00 PDT) |
| Hackathon AWS credit form ($150 per person) | **2026-10-21, 12:00 PT** (= 23:00 Dubai) — https://forms.gle/5hyhr1u6x3fuV2aW7 |
| Judging | 2026-11-09 → 2026-11-20 |
| Winners | ~2026-12-03 |

## Alexa+ track — what is actually required

- "Build a working Agent Skill or a self-hosted MCP server, implementing MCP spec version
  (minimum acceptable version is 2025-11-25)" over **Streamable HTTP**.
- Repo must use the track tech **at runtime in code** ("imported and actually called … not just
  named in the README").
- Alternative: a **simulated Alexa+ experience** in a web app with any agentic tool.

### PRD correction 1 — there is no Alexa+ console to register in

FAQ, verbatim: "Amazon's Alexa+ Add-on developer tools (Category SDK, MCP Toolkit, CLI, and Web
Simulator) are in preview and available to select partners only - there is currently no way for
hackathon participants to apply for or gain access."

FAQ, verbatim: "A web page that acts as an actual MCP client (sending initialize, tools-list, and
tools-call requests over Streamable HTTP) satisfies the requirement … there's no way for any
participant to connect a self-hosted MCP server to a real Alexa+ device or developer console."

So the original brief's PRD steps "Alexa+ skill manifest + registration in the Alexa+ dev
console" and "record with real voice queries [on Alexa]" **cannot be done**. Replace with: a
self-hosted MCP server + an open-standard Agent Skill (`SKILL.md`) + a web page that is a real
MCP client with voice in/out.
"Showing an existing AI assistant app calling the same server is a nice bonus."

### PRD correction 2 — the brief as written lands in the judges' "obvious" bucket

Judging, Quality of the Idea, Alexa+, verbatim:
- **Obvious:** "single-turn Q&A bot, basic MCP wrapper around an existing API."
- **Creative:** "agentic workflow that orchestrates across services autonomously, context-aware
  add-on that maintains state across sessions, purchasing capabilities, media support (cards,
  carousels, etc), MCP Apps, Agent Skills."

Five read tools over a JSON file is the obvious tier. See `IDEA.md` for what lifts it.

### PRD correction 3 — Open Source mini-challenge is an *additional* contribution

Rules: "Create a **new, additional** open-source project or contribute to an existing public
repository during the hackathon window, alongside a primary track submission … a new repo (with
open-source license), a branch, a fork, or a pull request." Required fields: contribution URL,
project repo URL, GitHub username, description.

The primary repo's visibility is irrelevant here (the rules say nothing about it); only the
contribution must be open. Plan: an upstream PR (e.g. to `rmcp`) fixing a real friction-log item.

## Repo rules

- Public **with an open-source licence file visible in the GitHub About section**, OR private and
  shared with `testing@devpost.com` + `chris-trag, knmeiss, giolaq, anishamalde, mosesroth,
  emersonsklar`. Either satisfies the track; the Open Source mini-challenge is judged on a
  separate public contribution (new repo, fork, branch or PR), not on this repo being public.
- Clear setup and run instructions; all source, assets, instructions.
- FAQ: "A locally runnable public repo plus your demo video is enough … hosting isn't required."
  Server does not need to stay live after submission.

## Video

- **Under 3:00**; judges need not watch past 3:00.
- Public on **YouTube or Vimeo**, English.
- **No third-party trademarks or copyrighted music** without permission. → Do not show the Alexa
  logo/Echo device as if it were our integration; no unlicensed music.

## Submission extras

- Product feedback on tools used. For AWS Builder: name the AWS services and how they were used,
  in the Product Feedback answer.
- Friction log entries, each: task attempted, steps, expected vs actual, severity, workaround,
  actionable suggestion. Up to **10% bonus**, assessed in Stage 1.
- Optional feature requests: description, why it matters, priority (Critical / Important / Nice-to-have).

## Prizes and stacking

- Alexa+ track: $25K + $15K AWS credits / $15K + $5K / $4K + $1K. 1st place also gets a meeting
  with the Amazon Developer team. AWS Builder and Open Source: $5K cash + $5K credits each, one winner each.
- "A project can only win one (1) track prize and one (1) mini challenge prize." So: Alexa+ track
  + at most ONE of AWS Builder or Open Source, not both as the original brief implied.
- AWS Builder: "Any primary track project that incorporates AWS services (i.e. Amazon Bedrock,
  AgentCore, Strands SDK, Kiro Crew, SageMaker, etc.) with documented integrations."

## Judging

Four equally weighted: Tech Implementation, Design, Potential Impact, Quality of the Idea.
Stage 1 may use "automated AI-driven analysis" → the README must state plainly where MCP 2025-11-25
and Streamable HTTP are implemented (file paths).
