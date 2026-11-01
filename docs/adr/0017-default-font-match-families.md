# ADR 0017: default font-match rules for more OFL families (NEW-131)

Status: Accepted, 2026-11-01. Amends spec 6.4 rung 3 (`DEFAULT_FONT_MATCH`; frozen: a change needs an ADR).

## Context
HDDN registers more OFL families lazily (Roboto, Open Sans, Lato, Noto Serif/Sans, DejaVu Sans, EB Garamond). Rung 3 only picks a
family that exists in the source name, so without rules those edits fell to the flags rung, which draws a different face.

## Decision
Add rules to `DEFAULT_FONT_MATCH` after the existing mono rules, in spec 6.4 order: roboto, opensans / open sans, lato, notoserif /
noto serif, notosans / noto sans, dejavusans / dejavu sans, verdana and tahoma to DejaVu Sans, and garamond to EB Garamond. Each
new rule names a family that is registered only when HDDN registers it. A rule whose family is not registered is skipped and the
next matching rule applies, so verdana, tahoma and garamond keep their Liberation Sans or Liberation Serif fallback. The mono rules
stay first, so DejaVuSansMono and NotoSansMono remain mono. Existing rules are unchanged. No font is bundled.

## Consequences
- A source name that contains a new pattern now draws with the registered family when one exists, so existing documents edited with
  those families change face. The change is only for names that match a new pattern and a registered family.
- Pattern order is part of the contract: a new family rule must not precede a mono or Liberation rule it would shadow.
