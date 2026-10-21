# 🦇 Graveyard Shift: payroll that stays in the dark

**Confidential payroll on the Tari Ootle.** No salary ever appears on-chain. Each employee reads
their own encrypted pay stub straight from the chain. A designated auditor reconciles the books
with a view key, and a coworker sees nothing at all.

[![CI](https://github.com/XachP/graveyard-shift/actions/workflows/ci.yml/badge.svg)](https://github.com/XachP/graveyard-shift/actions/workflows/ci.yml)
· **26 engine tests** · MIT · *Tari Ootle "Spooky Secrets" contest, October 2026*

**▶ Try it in the browser (no wallet, 30 s):** https://xachp.github.io/graveyard-shift/
Pick a lantern (public, employee, auditor, coworker) and watch the same three paychecks change.

![demo](docs/img/demo.gif)

---

## Why this matters

Every business runs payroll, and no business can run it on a transparent chain. A block explorer
would show every salary, raise and severance to coworkers, competitors and scammers. That isn't a
niche worry. Pay confidentiality is a legal and HR requirement in most companies, and it's why
real payroll never touches public chains.

But payroll can't be a black box either. Accountants, tax authorities and boards need to check
the books. **The Ootle can do both at once**, with primitives a transparent chain doesn't have:

| | Transparent chain | Mixer / custodian | **Graveyard Shift on Ootle** |
|---|---|---|---|
| Salaries hidden from the public | ✘ | ✔ | ✔ Pedersen commitments + range proofs |
| Employee gets pay info without a side channel | n/a | trust the custodian | ✔ encrypted pay stub on their output |
| Auditor can reconcile | ✔ (so can everyone) | trust the custodian | ✔ resource view key (ElGamal) |
| Auditor can't spend | n/a | ✘ | ✔ view key ≠ spend key |
| Employer can't pay off-book | ✘ | ✘ | ✔ engine requires a proof the auditor can read |
| Prove one salary to one landlord | ✘ (everyone sees it) | ✘ | ✔ payslip opening checked against the chain |

## The four lanterns

| 🕯 Lantern | Holds | Sees |
|---|---|---|
| 👁 **The public** | nothing | the public budget, the roster, that a shift was paid and its head count, and opaque commitments. **No amounts anywhere**: state, events and receipts were all scanned in tests |
| 🔑 **The employee** | own key | **their own** amount, mask and private memo ("36h nights + 4h overtime"), decrypted from the chain. Can spend it confidentially |
| 📜 **The auditor** | resource view key | **every amount**, enough to prove `Σ paychecks + treasury = public budget`. **No memos. No spending power** |
| 🦇 **A coworker** | any other key | nothing |

## Ootle privacy features used

* **Confidential resources.** The `SHIFT` payroll token is `ResourceBuilder::confidential()`. Balances are Pedersen
  commitments with Bulletproof range proofs.
* **Resource view keys.** `.with_view_key(auditor)`: every output carries an ElGamal
  `ViewableBalanceProof` to the auditor, and **the engine refuses outputs without one**.
* **Encrypted output data.** Each paycheck's `EncryptedData` is DH-encrypted to the employee
  (`encrypt_value_and_mask`): amount, mask and memo, with memo length padded to size tiers.
  Treasury change is encrypted to the employer, so the employer's wallet can be rebuilt from chain state.
* **`ConfidentialWithdrawProof`.** Paychecks spend treasury commitments into one hidden output and hidden change.
  Revealed amounts are rejected.
* **Value proofs** on mint, so the total budget is public while every split stays private.
* **Atomic multi-payment transactions.** `pay_shift` pays the whole crew in one transaction with a
  single `ShiftPaid {period, paychecks}` event, so per-person payment timing doesn't leak within a run.

## Architecture

```mermaid
sequenceDiagram
    autonumber
    participant E as Employer wallet
    participant GS as GraveyardShift component
    participant A as Alice's account
    participant AU as Auditor (view key)
    participant L as Landlord
    E->>GS: new(auditor_vk, budget commitments, value proofs)
    Note over GS: mints confidential SHIFT<br/>view key locked on the resource
    E->>GS: hire(alice, "night nurse") · open_period()
    E->>E: build paychecks: hidden output → stub encrypted to Alice,<br/>hidden change → encrypted to employer
    E->>GS: pay_shift([(alice, proof), (bob, proof), (carol, proof)])
    GS->>A: deposit(hidden paycheck)
    Note over GS: event ShiftPaid {period: 1, paychecks: 3}<br/>no names, no amounts
    A->>A: decrypt own stub: 3000 + memo
    AU->>AU: ElGamal-decrypt every vault:<br/>3000 + 2500 + 1313 + 3187 = 10000 ✔
    A-->>L: payslip (amount, mask), off-chain, to one person
    L->>L: recompute C = m·G + v·H, check it's in Alice's vault ✔
```

```mermaid
flowchart LR
    subgraph Public["👁 public (on-chain)"]
      B[budget: 10000 SHIFT] --- R[roster: alice · bob · carol]
      R --- C1[Alice: commitment C₁] & C2[Bob: C₂] & C3[Carol: C₃]
    end
    C1 -- employee key --> S1[🔑 3000 · memo]
    C1 -- view key --> V1[📜 3000]
    C1 -- other key --> X1[🦇 nothing]
```

### Methods

| Method | Who | What |
|---|---|---|
| `new(auditor_vk, budget, value_proofs)` | anyone (becomes the employer) | creates the confidential `SHIFT` token with the auditor view key and mints the opening budget |
| `hire(account, role)` / `terminate(account)` | employer | public roster (role label only, max 64 bytes); rehire after termination is allowed |
| `open_period()` | employer | next pay period; each active employee can be paid once per period |
| `pay(account, proof)` | employer | one confidential paycheck → event `PaycheckIssued {employee, period}` |
| `pay_shift([(account, proof)…])` | employer | up to 64 paychecks, atomic, one `ShiftPaid {period, paychecks}` event |
| `top_up(budget, value_proofs)` | employer | mint more confidential budget (total becomes public, split doesn't) |
| `roster`, `period`, `is_paid_this_period`, `staff_count`, `paychecks_issued`, `public_budget`, `treasury_commitment_count`, `payroll_resource` | anyone | read-only, never amounts |

## Run it

```bash
rustup target add wasm32-unknown-unknown
cargo test                                   # 26 tests on the real Ootle engine
cargo run --example haunted_payroll 2>/dev/null   # the narrated night shift (the GIF above)
cargo build --release --target wasm32-unknown-unknown   # → graveyard_shift.wasm (~190 KB)
```

`cargo run --example haunted_payroll -- --json docs/demo/transcript.json` then
`python3 docs/build_demo.py` regenerates the browser demo from a fresh engine run. Every commitment,
ciphertext and decryption on the page comes from that run, and the page decrypts nothing itself.

### Tests (26, all against the real engine)

| Area | Tests |
|---|---|
| Hidden salaries + reconciliation | salaries hidden / auditor reconciles · books reconcile across a month (raise, exit, top-up) · top-up grows only the public budget |
| Encrypted pay stubs | employee reads own stub from chain · auditor sees amount but not memo · coworkers can't open each other's stubs · employer rebuilds treasury from chain alone · employee spends paycheck, landlord reads rent |
| Payslips | honest payslip verifies · inflated, borrowed and forged payslips are caught |
| One-transaction shifts | one tx, one amountless event · one bad paycheck pays nobody (atomic) · same person twice rejected · empty shift rejected |
| Leak checks | events carry no amount fields · whole receipt scanned for the salary in decimal, LE bytes and hex |
| Guard rails | revealed paychecks rejected · off-book (no view-key proof) rejected by the engine · double pay · terminated staff · pay before a period opens · hire twice / rehire · outsiders can't hire, pay, open periods, top up, terminate or run shifts · auditor can't spend · internal helpers not callable |

See **[THREAT_MODEL.md](THREAT_MODEL.md)**, which maps 17 attacks to the test that plays each one, plus what still leaks.

## Honest limits

* **The roster is public** (account + role + paid-this-period). An auditable headcount needs that.
  Paying into stealth UTXOs would hide recipients too, and that's next.
* **The total budget is public** (supply tracking). The split isn't.
* **The auditor key is fixed per payroll** and opens all amounts of this token.
* **Not yet on Esmeralda.** The public testnet faucet is currently empty (999.65 tTARI held,
  1,000 per claim). Everything is verified on the in-process engine (`tari_template_test_tooling 0.47`).
  Unaudited.

> Build note: `tari_template_test_tooling 0.47.0` doesn't compile against `tari_crypto 0.24.1`.
> `Cargo.lock` pins `0.24.0`. If you regenerate the lock, run `cargo update -p tari_crypto --precise 0.24.0`.

## Roadmap

1. Deploy to Esmeralda and record explorer links for a full shift.
2. Stealth paychecks, so the roster is private too (pay to one-time stealth outputs and prove
   headcount with a counter).
3. Sapient Web Extension page for employees: connect, read your stubs, export a payslip.
4. Auditor CLI that reconciles straight from the indexer.

## Disclosure

Built with an AI coding agent under the submitter's direction. All code is new for this contest (October 2026). MIT.
