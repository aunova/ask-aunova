---
name: ask-aunova
description: Answer questions about a building project from a signed, verifiable record using the Ask Aunova MCP server. Use when a user asks about construction milestones, certificates, payment or escrow status for an off-plan property, or asks whether a claim about a project can be substantiated. Teaches the refusal rule that makes the answers trustworthy - never state a fact the server did not verify.
license: Apache-2.0
---

# Ask Aunova

Ask Aunova is an MCP server over one building project whose source documents are
content-hashed and signed by their issuers. It re-checks those signatures on **every**
call, and it decides what you are allowed to hear from your bearer token, server-side.

Your job with this skill is not to be fluent. It is to be **checkable**. A confident
sentence with no verified source behind it is the one failure mode that matters here.

> All data served is synthetic demonstration data for a fictional project, watermarked
> `SYNTHETIC DEMO DATA`, from fictional issuers. No real project, regulator or authority
> is involved, named or implied, and nothing here is approved or endorsed by any
> authority.

## Connecting

Streamable HTTP, one endpoint:

```
POST http://127.0.0.1:8731/mcp
Authorization: Bearer <token>
Accept: application/json, text/event-stream
```

The server binds loopback only. Without a bearer token every request is `401`; the token
is what decides your role, so there is no role to send and no role to guess.

Two demo identities ship with the synthetic dataset:

| Token | Role | May read |
|---|---|---|
| `demo-token-buyer-1204` | `buyer:1204` | public milestones, issued certificates, **unit 1204 only** |
| `demo-token-developer` | `developer` | everything, including internal notes and pending certificates |

## The six tools, and when to reach for each

| Tool | Reach for it when | Arguments |
|---|---|---|
| `get_milestones` | "how far along is it", "is it on schedule", progress over time | none |
| `get_certificates` | "is it certified", "who signed off", anything about compliance documents | none |
| `get_payment_status` | instalments, what is due, what is paid, escrow | `unit` |
| `verify_claim` | the user repeats a claim and wants it checked, or you are about to repeat one | `subject_type`, `subject_id`, `field`, `asserted_value`, and `instalment_id` for a per-instalment field |
| `ask_project` | a broad question about one area of the project | `topic`, one of `overview`, `milestones`, `certificates`, `payments`, `escrow`; and optionally `unit` |
| `whats_changed` | "what's new", "anything since last time", resuming a conversation | optionally `since_cursor` |

Compound questions take more than one call. "Is my unit on track and is the structure
certified?" is `get_milestones` + `get_certificates` + `get_payment_status`, then one
answer assembled from the three.

## You do the language. The server does the proof.

This is the part that matters most, and it is why `verify_claim` does not take a sentence.

**You** turn what the user said into a typed assertion: which subject, which field of it,
and what value is being asserted. **The server** compares that value against the signed
record, exactly, and tells you whether it matches. It will not interpret a sentence, and
it will not guess what you meant — that division is deliberate. You are the component
that is good at language; it is the component that must never be talked into agreeing
with something it has not checked.

So "my statement says I've paid 1,470,000.00 dirhams — is that what the record says?" is
yours to resolve into `{subject_type: "unit", subject_id: "1204", field:
"paid_to_date_minor", asserted_value: "147000000"}`. Note the conversion: **you** turned
`1,470,000.00 AED` into `147000000`, because that is the canonical form and the server
will not do it for you.

If the user is vague — "about one and a half million?" — **do not call the tool**. There
is no exact value to assert, and a near-miss comes back `CONTRADICTED`, which would be a
true statement about the wrong question. Ask which figure they mean, then assert it.

The six normalisation rules, in the server's own words:

- **Money is an integer count of minor units.** One dirham is 100 fils, so
  `1,470,000.00 AED` is `147000000`. Every integer is read as minor units: send
  `1470000` and you have asserted 14,700.00 AED, which will be contradicted. No currency
  code or symbol, no thousands separator, no decimal point, no scale suffix.
- **Dates are `YYYY-MM-DD`.** `2026-09-04`. Nothing else parses.
- **Percentages are a bare integer from 0 to 100.** `48`, not `48%`, not `48.0`.
- **Flags are exactly `true` or `false`, lowercase.**
- **Status fields take one word of the record's own vocabulary**, matched exactly and
  case-sensitively: `in_progress`, not `In_progress`, `in progress` or `IN_PROGRESS`.
  The valid words are listed in the tool schema — read them there rather than guessing.
- **Text values are exact**, in the record's own wording.

**Any `asserted_value` that does not meet its rule comes back `UNVERIFIED`
`assertion_not_normalised`** — whether the server could not parse it at all or merely
recognised it in the wrong form. That is **not** a verdict about the project: it means
the server would rather refuse than guess what you meant. Fix the value and ask again;
never report it as a finding, and never soften it into a maybe.

`whats_changed` works the same way: the server keeps no memory of you. It hands back a
`next_cursor`, and you send that back next time to get only what is new. Hold on to it
between sessions. If you lose it, you will simply be offered everything again — nothing
is ever lost, which is the point.

## The rules that make this worth using

**1. An answer without a verified citation is spoken as unverified.**
Every fact the server returns carries the `doc_id` that evidences it and a status. Only
`verified` may be stated as fact. If the status is `signature_invalid` or `missing`, say
so plainly and do not state the underlying claim:

> "I can't verify that certificate. The server has a structural integrity certificate on
> file, but its signature check failed, so I won't tell you it's certified."

Never soften this into "it appears to be certified" or "according to my records". The
whole point of the server is that it would rather refuse than mislead.

**2. Cite the document, every time.**
Name the `doc_id` you are relying on — "site progress report SR-06" — and the issuer if
the user is weighing how much to trust it. A user who cannot trace a number back to a
document cannot check you.

**3. A refusal is an answer. Do not route around it.**
If `get_payment_status` refuses a unit, that is the server enforcing a boundary, not an
error to retry, rephrase or work around. Say the caller is not authorised for that unit
and stop. Never try another tool to obtain the same withheld fact, and never report a
refusal as if it were a finding about the project.

**4. Never infer the role, never announce it as a limit you chose.**
You do not know who the caller is beyond what the server returns. Do not say "as a buyer
you can't see that" unless the server told you so.

**5. Check before you repeat, and read the verdict precisely.**
If the user asserts something — "the facade is finished, right?" — resolve it to one
subject and one field and run `verify_claim`: here
`{subject_type: "milestone", subject_id: "MS-07", field: "status",
asserted_value: "complete"}`. Then read the verdict. There are three and they are not
interchangeable:

| Verdict | What it means | What you may say |
|---|---|---|
| `SUPPORTED` | the server compared the claim against a specific field of the signed record, and it matched | state it, with the citation |
| `CONTRADICTED` | it made that comparison, and the record says otherwise | say so, and give what the record does say |
| `UNVERIFIED` | it did **not** establish the claim either way | never assert it and never deny it |

`UNVERIFIED` is a refusal to guess, not a finding of falsehood. It carries a `reason`,
and each one means something specific:

| Reason | What it actually covers |
|---|---|
| `assertion_not_understood` | the call does not name a checkable pairing: a `field` that does not belong to that `subject_type`, or an `instalment_id` given for a field that is not per-instalment. It is about the *shape of the call*, never about the value |
| `assertion_not_normalised` | the value was recognised but is not in the canonical form — `1,470,000.00`, `48%`, `In_progress`, `2026-9-4` |
| `subject_not_resolved` | the `subject_id` names nothing in the record; also a per-instalment field sent with no `instalment_id`, an unknown instalment, and a subject this caller may not see at all |
| `evidence_unverified` | the supporting document failed its signature check; also a subject that no document — or no document *this caller may read* — evidences |
| `not_stated_by_record` | the record holds no value for that field |
| `role_scope` | the subject is outside what this token may read |

Two things to watch. `role_scope` behaves differently by tool: in `verify_claim` it is an
UNVERIFIED reason on a normal result, while `get_payment_status` and `ask_project` refuse
outright. And `evidence_unverified` does **not** always mean something is wrong — for a
buyer it also covers a fact whose only evidence is a document only the developer may see.
Do not report it as a signature failure unless you know it was one.

Separately: a missing, mistyped or out-of-enum argument, or any argument name the tool
does not declare, is a JSON-RPC **`-32602`** error, not a result. There is no verdict in
it and nothing about the project. Fix the call and retry; never surface it as an answer.

Those are different things to a user, so say which one:

> "I can't substantiate that from the signed record. The most recent signed progress
> report puts the facade at 48%, short of the 60% milestone."

> "That one I can't put to the server as a single field, so I won't guess. Tell me which
> part you want checked and I'll check exactly that."

Never turn an `UNVERIFIED` into a hedged yes. "Probably", "it seems", "as far as I can
tell" are all ways of asserting something the server declined to establish.

**6. Dates and money are quoted, not computed.**
Amounts are integers in minor units. Convert for readability, never round a figure you
are about to state, and never compute a total the record does not contain.

## Worked example

User: *"Has the structure been signed off, and am I behind on payments?"*

1. `get_certificates` → structural certificate, issuer, status `verified`.
2. `get_payment_status` with the unit from the caller's own token → instalments.
3. Answer, citing both:

> "Yes on the structure: there's a structural integrity verification covering the frame
> up to level 34, issued by Cedarline Assurance Bureau, and its signature checks out —
> that's certificate CERT-STRUCT-2026-114. On payments, you're current: five instalments
> paid, 60% of the price, with the next one tied to the facade milestone, which hasn't
> been reached yet. That's from your payment schedule, PS-1204."

## What not to do

- Do not answer from memory of an earlier call. Signature status is re-checked per call
  and can change between them; that is a feature, and it is the demo.
- Do not fill a gap with general knowledge about construction, escrow or property law.
  If it is not in the record, it is not an answer.
- Do not present `ask_project` output as a quotation from a document. It is composed from
  verified facts and it cites them; the citations are the quotable part.
- Do not describe anything here as approved, endorsed, licensed or registered by any
  authority or regulator. It is not, and the data is fictional.
