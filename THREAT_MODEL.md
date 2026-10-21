# 🕯 Graveyard Shift: threat model

What each party can learn or do, what stops them, and which test proves it. Everything here runs
against the real Ootle engine (`tari_template_test_tooling`). File names point at `tests/`.

## Who sees what

| Party | Holds | Learns | Can't |
|---|---|---|---|
| Public observer | nothing | public budget (total supply), roster (account + role label), that a period was paid, head count per shift, commitments and ciphertext sizes | any salary, any memo, the treasury balance |
| Employee | own account key | own amount, mask and memo, decrypted from the chain | anyone else's pay or memo |
| Auditor | resource view key (secret) | every amount in every vault of the `SHIFT` token, so they can reconcile | memos (stubs are encrypted to employees, not to the view key), spending anything |
| Employer | employer key | everything it paid; treasury masks recoverable from the chain | pay outside the rules below, hide a paycheck from the auditor |
| Landlord / lender | one payslip from the employee | one verified amount | anything else |

## Attacks and defences

| # | Attack | Defence | Test |
|---|---|---|---|
| 1 | Employer publishes a salary by accident (revealed amount) | `pay`/`pay_shift` reject any revealed input/output/change | `payroll.rs::revealed_paychecks_are_rejected_so_salaries_cannot_leak` |
| 2 | Employer pays off-book, hidden from the auditor | resource has a view key, so the engine requires a `ViewableBalanceProof` on every output | `payroll.rs::employer_cannot_hide_a_paycheck_from_the_auditor` |
| 3 | Salary leaks through events | events carry only `employee`/`period` (`pay`) or `period`/`paychecks` (`pay_shift`); fields are checked exactly | `payroll.rs::paycheck_events_never_contain_amounts`, `secrets.rs::whole_shift_is_paid_in_one_transaction_with_one_amountless_event` |
| 4 | Salary leaks anywhere in the receipt | full receipt scanned for the amount in decimal, LE bytes and hex | `secrets.rs::a_salary_never_appears_anywhere_in_the_transaction_receipt` |
| 5 | Coworker reads a colleague's pay | stubs are DH-encrypted to the recipient; ElGamal balances need the view key | `secrets.rs::coworkers_cannot_open_each_others_stubs`, `payroll.rs::salaries_are_hidden_…` |
| 6 | Auditor reads private memos | view key ≠ stub key | `secrets.rs::auditor_sees_amounts_but_not_the_private_memo` |
| 7 | Auditor (or anyone) moves treasury funds | treasury has no withdraw method; every write is `rule!(public_key(employer))` | `secrets.rs::the_auditor_view_key_has_no_spending_power` |
| 8 | Outsider hires, pays, opens periods, tops up, terminates, runs a shift | employer-only access rules, `default(deny_all)` | `payroll.rs::outsiders_cannot_hire_or_pay`, `secrets.rs::outsiders_cannot_open_periods_top_up_terminate_or_run_shifts` |
| 9 | Internal helper called directly | not exposed; default deny | `secrets.rs::internal_helpers_are_not_callable` |
| 10 | Double pay in a period | `last_paid_period` check | `payroll.rs::cannot_pay_the_same_employee_twice_in_one_period` |
| 11 | Same person twice inside one shift | same check, whole shift reverts | `secrets.rs::the_same_employee_twice_in_one_shift_is_rejected` |
| 12 | One bad paycheck in a shift, the rest go through | `pay_shift` is atomic | `secrets.rs::a_shift_with_one_bad_paycheck_pays_nobody` |
| 13 | Paying terminated staff / before a period opens | `active` and `period > 0` checks | `payroll.rs::terminated_staff_are_not_paid`, `secrets.rs::paying_before_any_period_is_opened_fails` |
| 14 | Inflated, borrowed or forged payslip | verifier recomputes `C = m·G + v·H` and checks it sits in that employee's vault | `secrets.rs::inflated_or_borrowed_payslips_are_caught` |
| 15 | Employer loses its books | change is encrypted to the employer; treasury rebuilt from chain | `secrets.rs::employer_rebuilds_its_treasury_from_chain_state_alone` |
| 16 | Books drift over many periods (raises, exits, top-ups) | Σ paychecks + treasury = public budget, checked after a month | `secrets.rs::books_reconcile_across_a_whole_month_of_shifts` |
| 17 | Minting, burning or recalling `SHIFT` outside the rules | mint locked to the component (employer-gated `top_up`); burn and recall `deny_all`, `LOCKED` | `payroll.rs::top_up_grows_the_public_budget_only`, #8 |

## What still leaks (by design or by the platform)

* **Roster and timing.** Who is on staff, their role label and the period they were paid are
  public. That is the price of a publicly auditable headcount. `pay_shift` hides the order and
  timing *within* a run but not membership. A follow-up could pay into stealth UTXOs so the
  recipients are hidden as well.
* **Total budget.** Supply is public (value proofs on mint). How it's split is not.
* **Ciphertext size.** Memo ciphertexts are padded to size tiers by the wallet crypto, so the length
  reveals a coarse bucket only (all demo stubs are 144 bytes).
* **The auditor key is all-or-nothing** for amounts of this token, and fixed at creation. Pick the
  auditor carefully; a new payroll is needed to rotate it.
* **Payslips are copyable.** Once an employee shows a payslip, the verifier knows that one amount
  and could pass it on. It reveals nothing about other paychecks.
* **Fees and transaction graph.** Who signs and pays fees for a payroll transaction is visible like
  any Ootle transaction.
* **Not deployed to Esmeralda yet** (the public faucet is empty: 999.65 tTARI held, 1,000 per
  claim). Everything is verified on the in-process engine. Unaudited, so treat it as testnet software.
