# Owner decisions

All answered by the owner on 2026-10-18; D4 and D5 updated by the owner on 2026-10-21.

| # | Question | Decision | Consequence |
|---|---|---|---|
| D1 | Who is the entrant? | **Organization: Aunova Technologies Ltd** (DIFC, incorporated 2026-06-26, reg. 13718, so it "exists at time of entry") | Chris registers on Devpost and enters as the company's **Representative**; any prize is paid to the company; the post-competition affidavit is signed for the company. |
| D2 | GitHub home and visibility | **Private under `aunova` while building; public on Friday only with owner approval** | If Friday is "no": share privately with `testing@devpost.com` + `chris-trag, knmeiss, giolaq, anishamalde, mosesroth, emersonsklar`. |
| D3 | Brand | **Ask Aunova / Aunova Trust Infrastructure** | Synthetic data only; no claim that any regulator or authority approves, endorses or works with it. |
| D4 | Voice in the video | **Questions typed on screen, Amazon Polly speaks the answers, a generated narration voice tells the story** (2026-10-21; replaces the 2026-10-18 "Chris asks, Amazon Polly answers") | Polly is a runtime AWS service → listed in the AWS Builder documentation and Product Feedback answer. The narration names the model that drove the recording. |
| D5 | $150 hackathon AWS credit form | **Skip** | Use the existing company AWS account through a dedicated least-privilege IAM user (policy: `docs/aws-iam-policy.json`). |

Already decided by the rules, not by preference:
- No Alexa+ console integration (partner-only per FAQ) → simulator path.
- Licence Apache-2.0.
