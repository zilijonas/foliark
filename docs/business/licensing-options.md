# Foliark licensing and contribution options (NEW-72)

## Owner decision — 2026-10-22

This options memo is historical. The owner approved AGPL-3.0-only plus separate
commercial licensing, copyright Linas Jonas Zilinskas, no outside code contributions
initially, public source at github.com/zilijonas/foliark and public npm publication
under the zilijonas account. See [LICENSING.md](../../LICENSING.md),
[commercial licensing](../../COMMERCIAL-LICENSING.md) and Workbench NEW-73.
Earlier recommendations and unresolved owner questions below are superseded.

Status: options memo for the owner decision in NEW-73. Not legal advice; have a lawyer review the chosen route and the CLA text. Facts are from the repo and the primary sources linked inline. Claims not checked against a primary source are marked (unverified).

## 1. Goals and constraints

Inferred owner goals: (1) keep rights and control; (2) keep the option to commercialize; (3) replace PyMuPDF (AGPL-3.0 or Artifex commercial) inside the closed web app HDDN; (4) ship as a Rust/WASM SDK via npm to browser apps.

Repo facts:
- Root license today is `UNLICENSED` (`Cargo.toml:10`); no LICENSE file; repo is private and not published.
- Production deps are permissive: 100 prod crates, mostly MIT/Apache-2.0 (also Unicode-3.0, Zlib, Unlicense, BSD-3, BSL-1.0); zero runtime npm deps (`docs/provenance/dependencies.json`).
- hayro-syntax and hayro-interpret 0.8.0 are vendored unpatched (Apache-2.0 OR MIT); patches P1-P6 are planned. hayro upstream is dual Apache-2.0/MIT ([github.com/LaurenzV/hayro](https://github.com/LaurenzV/hayro)). Those files stay under upstream terms whatever Foliark chooses, and their notices must ship.
- 14 Foxit Base-14 `.pfb` fonts are embedded under the PDFium BSD-style notice; `LICENSE_FOXIT` must ship (`docs/provenance/assets.md`, NEW-74). Foxit font terms beyond that notice are unverified.
- Nothing in the dependency graph is copyleft, so no dependency forces any choice. Every option below is technically open.
- Open: who holds copyright (NEW-2). Whoever the holder is decides whether relicensing later is even possible. Current commit authors are agent-written work done for the owner; the work-for-hire/assignment position has to be confirmed by the owner, not assumed.
- Owner always keeps the right to use their own code in HDDN under any option, provided they hold (or are licensed) all copyright in it.

Why the comparison set matters: it shows what the market tolerates for a PDF engine.

| Project | License | Source |
|---|---|---|
| MuPDF / PyMuPDF | AGPL-3.0 or Artifex commercial; Artifex is "the exclusive commercial licensing agent" | [artifex.com/licensing](https://artifex.com/licensing), [PyMuPDF docs](https://pymupdf.readthedocs.io/en/latest/about.html) |
| pdf.js | Apache-2.0 | [LICENSE](https://raw.githubusercontent.com/mozilla/pdf.js/master/LICENSE) |
| PDFium | BSD-3-Clause (plus Apache-2.0 for some parts) | [LICENSE](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/LICENSE) |
| pdf-lib | MIT | [LICENSE.md](https://raw.githubusercontent.com/Hopding/pdf-lib/master/LICENSE.md) |
| hayro | Apache-2.0 and MIT | [repo](https://github.com/LaurenzV/hayro) |

Only MuPDF monetizes via licensing. The rest are permissive, with value coming from elsewhere (browser vendor, Google, hobby/consulting).

## 2. Options

### A. Permissive core (Apache-2.0 or MIT) plus paid services

What it is: publish under Apache-2.0 (patent grant and notice rules; MIT is simpler but has no patent grant). Charge for support, SLAs, hosted conversion/redaction API, extra tooling, custom work.
- Friction: lowest. Matches the dependency stack, pdf.js, PDFium, hayro. Browser apps and npm consumers can embed with no analysis beyond notices.
- Monetization: services only. A competitor (including a cloud vendor) can sell the same code; no license fee is possible on the SDK itself.
- Contribution: DCO is enough (see section 4); inbound=outbound. No relicensing needed because no license revenue.
- Irreversibility: high. Copies already granted under MIT/Apache cannot be revoked. You can stop publishing new versions under it or go closed on future versions, but the old versions stay free for everyone. Relicensing the future code needs the holder (all copyright) to agree.
- Trademark: license grants no trademark rights; register and police "Foliark" separately (unverified for the Apache text: check section 6 of [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0)).
- Risks: free-riding, forks, little revenue lever; adoption is strongest.

### B. Copyleft plus commercial license (dual licensing, MuPDF model)

What it is: release under AGPL-3.0 (or MPL-2.0) and sell commercial licenses to those who cannot or will not comply. Artifex runs exactly this for MuPDF.
- Friction: AGPL is the highest friction for the target users. AGPL section 13 obliges a modifier to offer Corresponding Source to network users ([AGPL-3.0 text](https://www.gnu.org/licenses/agpl-3.0.txt)). Artifex states the open edition cannot be used in a server-based service without disclosing the full application source, otherwise "a commercial license is required" ([artifex.com](https://artifex.com/licensing)). Whether shipping WASM in a browser bundle counts as conveying and creates a combined work is a lawyer question (unverified); many app companies will refuse AGPL by policy and either pay or leave.
- MPL-2.0 alternative: file-level copyleft; new files can stay proprietary and the library can be statically linked into a proprietary larger work, only changes to MPL files must be shared ([MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)). Much lower friction than AGPL but weaker commercial pressure; it is a weaker dual-license lever.
- Monetization: license fees (per product/OEM/SaaS tiers; Artifex uses use-case-specific pricing with per-copy OEM and subscription models, no public prices) plus support. Direct revenue from the very users AGPL repels.
- Contribution: dual licensing only works if you can relicense every line, so you need a CLA or copyright assignment from every outside contributor; DCO is not enough. DCO has contributors certify the right to submit under the file's license; it "does not transfer copyright or grant any relicensing rights" beyond that ([developercertificate.org](https://developercertificate.org/), summary by fetch tool, plain reading). The Apache ICLA model has contributors keep rights and license the project ([apache.org](https://www.apache.org/licenses/contributor-agreements.html)); it is a template, not a drop-in for a commercial relicense. Whether its grant includes sublicensing is unverified from the page alone (the PDF text was not read).
- Irreversibility: AGPL releases already published stay AGPL for those recipients, but a sole holder can stop, relicense future versions, or loosen to permissive later. Going permissive later is easy; going the other way is not.
- Risks: slow adoption, no one buys if the engine is not yet best-in-class, CLA friction for contributors, the held hayro/Foxit parts stay on their own terms (fine: Apache/MIT code can be included in an AGPL or commercial product).

### C. Source-available or proprietary with a free tier

What it is: BSL 1.1, FSL, Elastic License 2.0, or fully closed binaries with a free tier.
- BSL 1.1: source readable and non-production use allowed; production use needs an Additional Use Grant or paid license; converts to a GPL-2.0-or-later-compatible license at the Change Date, which is at most four years after a version's first public distribution; "is not an Open Source license" ([mariadb.com/bsl11](https://mariadb.com/bsl11/)).
- FSL: permits almost all use except undermining the producer; each version becomes Apache-2.0 (FSL-1.1-ALv2) or MIT (FSL-1.1-MIT) two years after release ([fsl.software](https://fsl.software/)). Needs no per-user commercial license but blocks a competing SDK or service.
- ELv2: bars providing the software to third parties as a hosted/managed service and bars bypassing license keys ([elastic.co](https://elastic.co/licensing/elastic-license)). Not suited to an embedded SDK as its main goal is hosted-service protection.
- Closed: ship WASM binary plus types, no source, EULA with free tier. Highest control, no review by users.
- Friction: moderate to high. Legal teams must read a bespoke license; some companies and distros reject non-OSI licenses; npm consumers can still install it, and browser use is fine if the grant allows production embedding in apps. A custom Additional Use Grant (for example "free below revenue X or any use that is not a competing PDF SDK") is the design lever.
- Monetization: paid production license or competing-use carve-out; works without a CLA in the same strict way as B only if the owner still holds all copyright (relicense needed for the paid grant, so contributions need a CLA there too).
- Contribution: CLA again, or accept no external contributions.
- Irreversibility: medium. Each released version becomes permissive by the stated date, by design; earlier versions can be released under a stricter license only for future releases.
- Risks: reputation as "not open source", fork-and-continue after conversion, enforcement burden, mismatch with a stack built entirely on permissive crates (the community may ask why).

### D. Defer: stay private, license later

What it is: keep `UNLICENSED`, no public repo, no external contributors, finish provenance and the rights question first. Ship only inside HDDN.
- Friction: none for HDDN (owner uses own code). No external users yet, so no integration friction to solve.
- Monetization: none yet; keeps all options open.
- Contribution: accept none (or only signed assignments). That keeps clean title so B or C stays possible, which is the real value of waiting.
- Irreversibility: none. Lowest regret; the only decay is time (no community, no adoption, competitors).
- Risks: opportunity cost; "later" can drift; the NEW-2 question stays unresolved if never forced. Private npm publishing or sharing binaries with a customer is a distribution and needs at least an EULA (unverified practical need).

## 3. Comparison

| Dimension | A permissive | B AGPL/MPL + commercial | C BSL/FSL/closed | D defer |
|---|---|---|---|---|
| Adoption friction | lowest | high (AGPL), low-med (MPL) | medium-high | n/a |
| Browser/npm fit | excellent | AGPL doubtful, MPL good | fine if grant allows | n/a |
| License revenue | none | yes (core lever) | yes | none yet |
| Services revenue | yes | yes | yes | none yet |
| Needs all copyright | no | yes (CLA/assignment) | yes if paid grant | keep title clean |
| Contribution tool | DCO | CLA | CLA or none | none |
| Revocable later | no | no for old releases; yes going forward | each version converts by date | yes |
| Open-source label | yes | yes | no until conversion | n/a |
| Main risk | free-riding | few buyers, adoption drag | trust, enforcement | drift |

## 4. Contributions: CLA vs DCO

- DCO is a sign-off certifying right to submit under the project's stated license; it keeps inbound = outbound and adds no relicensing right.
- A CLA (Apache ICLA is the best-known template) is what makes later relicensing or dual licensing possible. A copyright assignment is stronger but harder to get from individuals and in some jurisdictions.
- Under D, or if the repo stays solo-authored, neither is needed yet; the risk is only retrofitting: every outside commit before a CLA exists must be re-cleared or removed.

## 5. Recommendation (the owner decides)

Choose D now, with B (MPL-2.0 or AGPL-3.0 plus a commercial license) as the working plan for the first public release, and A as the fallback if the goal shifts from control and revenue to adoption.

Reasoning:
- The owner's stated goals (keep rights and control, may commercialize) point at a holder who can relicense. Loosening a license later (to Apache-2.0) is always possible for a sole holder; tightening is not, because permissive releases cannot be revoked. So the order that preserves the most options is: stay private, resolve who owns the copyright, then publish at the strictest level the owner is happy with.
- No dependency constrains the choice, and HDDN is unaffected under any option.
- Between AGPL and MPL for B: AGPL has the MuPDF precedent and the strongest commercial pull, but is the worst fit for browser app developers; MPL-2.0 is friendlier but sells fewer licenses. If revenue from licenses matters, AGPL; if ecosystem matters, MPL or A. C (FSL) is a reasonable middle if the owner mostly wants to stop a competitor reselling the SDK while letting everyone else use it.

Uncertainties:
- Demand: no evidence yet that anyone would pay a license fee for a new engine; MuPDF has decades of reputation.
- How AGPL applies to WASM in a browser bundle and to separate-process use (lawyer question; not verified here).
- Whether the MPL Secondary License definition includes AGPL (unverified).
- Foxit font terms beyond the PDFium notice; the three UNLICENSED entries in the cargo graph are Foliark's own crates (to confirm).
- Whether work done by agent sessions on the owner's behalf is cleanly owned by the owner (NEW-2).

## 6. Questions for NEW-73

1. Copyright holder: personal name or an entity (company)? Do any employer, contractor, or agent-tool terms give someone else a claim on code written so far (NEW-2)?
2. CLA vs DCO: will outside contributions be accepted at all? If yes, CLA (relicensable) or assignment; who signs and where is it administered?
3. Trademark and name: is "Foliark" cleared and registered (npm scope `@foliark`, domain, class 9/42)? Who owns the mark?
4. Public timing: when does the repo or npm package go public (before or after HDDN has shipped on it)? Any hard date?
5. Pricing and revenue intent: license fees, support, hosted API, or none? Target customer size and expected price band? (Needed to choose between AGPL, MPL, FSL, Apache.)
6. Red lines: is a competing SDK or hosted PDF service allowed to reuse the code? Is "not OSI open source" acceptable?
7. Evidence for the decision: approve a lawyer review of the chosen license and CLA text before first publication?
