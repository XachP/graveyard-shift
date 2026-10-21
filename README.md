# 🦇 Graveyard Shift: payroll that stays in the dark

**Confidential payroll on Tari Ootle.** Salaries never show up on-chain. A designated auditor
can still open the books with a view key.

*Entry for the Tari Ootle October 2026 "Spooky Secrets" build contest. MIT licensed.*

---

## The secret

Every business runs payroll, and no one wants it on a public ledger. On a transparent chain a
block explorer shows every salary, raise and bonus to coworkers, competitors and scammers.
Most "private payroll" setups fix this by trusting a custodian.

Graveyard Shift uses Ootle's native **confidential resources** and **view keys** instead:

| Who | Sees |
|---|---|
| **Everyone** | The total budget the employer minted (supply tracking), who is on staff, and which pay periods each person was paid for |
| **Nobody on-chain** | Any individual salary. Paychecks are Pedersen commitments, so no amount appears in state, events or receipts |
| **The employee** | Their own paycheck. They hold the mask and can spend it confidentially (rent, savings, etc.) |
| **The auditor** (view key) | Every paycheck and the treasury balance, enough to reconcile `Σ paychecks + treasury = budget`. The view key can't spend anything. |

That's the spooky part: the money moves in plain sight, but only the person holding the right key can read it.

## How it works

```
employer ──new(auditor_vk, budget)──▶  GraveyardShift component
                                        ├─ treasury: Vault<SHIFT>   (confidential, view key = auditor)
                                        ├─ staff:    account → {role, active, last_paid_period}
                                        └─ period:   u64
employer ──hire(alice, "night nurse")──▶ roster (public)
employer ──open_period()──────────────▶ period += 1
employer ──pay(alice, withdraw_proof)─▶ treasury ─(hidden amount)─▶ alice's account
                                        event: PaycheckIssued{employee, period}  ← no amount
auditor  ──(off-chain) decrypt with view key──▶ 3000 + 2500 + 4500 = 10000 ✔
```

* **`new(auditor_view_key, budget, value_proofs)`** creates the `SHIFT` confidential payroll token
  with the auditor's view key fixed on the resource and mints the opening budget as commitments.
  Only the component itself can mint more (`top_up`), and nobody can burn or recall.
* **`hire` / `terminate`** manage a public roster that holds a role label and no amounts.
* **`open_period` / `pay`**: each active employee can be paid **once per period**. A paycheck is
  a `ConfidentialWithdrawProof` the employer builds client side. It spends treasury commitments
  into exactly one hidden output for the employee, and the hidden change goes back to the treasury.
* **`top_up`** adds confidential budget. The minted total becomes public. How it's split into
  salaries doesn't.
* Read-only views: `period`, `is_paid_this_period`, `staff_count`, `paychecks_issued`,
  `public_budget`, `treasury_commitment_count`, `payroll_resource`.

### Guard rails (all covered by tests)

1. **No accidental leaks.** `pay` rejects any proof with a revealed input, output or change
   amount, so an employer can't publish someone's salary by mistake.
2. **No hiding from the auditor.** Because the resource carries a view key, the engine refuses
   any paycheck output without a `ViewableBalanceProof`. An employer can't sneak an off-book
   payment past the auditor.
3. **No double pay.** One paycheck per employee per period.
4. **Employer-only.** `hire`, `terminate`, `open_period`, `pay` and `top_up` require the
   instantiating key (`rule!(public_key(employer))`, taken from `CallerContext`, never from an argument).
5. **Terminated staff can't be paid.**
6. **Fixed supply rules.** Mint is locked to the component, and burn and recall are `deny_all` and `LOCKED`.

## Tests

The tests run against the real Ootle execution engine via `tari_template_test_tooling`:

```
$ cargo test
running 9 tests
test salaries_are_hidden_but_the_auditor_can_reconcile_the_books ... ok
test paycheck_events_never_contain_amounts ... ok
test employee_can_spend_their_paycheck ... ok
test cannot_pay_the_same_employee_twice_in_one_period ... ok
test revealed_paychecks_are_rejected_so_salaries_cannot_leak ... ok
test employer_cannot_hide_a_paycheck_from_the_auditor ... ok
test outsiders_cannot_hire_or_pay ... ok
test terminated_staff_are_not_paid ... ok
test top_up_grows_the_public_budget_only ... ok
test result: ok. 9 passed; 0 failed
```

The headline test hires Alice (night nurse) and Bob (security guard), pays them 3,000 and 2,500
from a 10,000 budget, then shows that:

* each employee vault holds a single opaque commitment,
* the auditor decrypts 3,000, 2,500 and 4,500 (treasury) with the view key, and they sum to the public budget,
* a random "nosy coworker" key decrypts nothing.

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release   # → target/wasm32-unknown-unknown/release/graveyard_shift.wasm
cargo test
```

> Note: `tari_template_test_tooling 0.47.0` doesn't compile against `tari_crypto 0.24.1`
> (type inference break in `support/stealth.rs`). `Cargo.lock` pins `tari_crypto 0.24.0`. If you
> regenerate the lock file, run `cargo update -p tari_crypto --precise 0.24.0`.

## Limitations and next steps

* **Mask delivery.** In the tests the employer hands each employee their paycheck mask
  directly. A production client would put it in the output's encrypted data for the employee,
  the same way Ootle wallets already deliver confidential transfers.
* **Treasury bookkeeping** happens client side. The employer's wallet tracks treasury
  commitment masks, like any confidential wallet does.
* **Roster privacy.** Who is employed is public by design here, since an auditable payroll needs a
  public headcount. A follow-up could pay into stealth UTXOs to hide recipients too.
* **Next:** a small CLI (ootle-rs) for the employer and an auditor CLI that prints a reconciliation report
  from the indexer, plus a Sapient Web Extension front end for employees.

## Disclosure

Built with the help of an AI coding agent, under the direction of the submitter. All code is new
for this contest (October 2026).
