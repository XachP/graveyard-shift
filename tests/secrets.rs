//! The privacy features: encrypted pay stubs, payslip disclosure, one-transaction shifts, and the
//! limits of each key. Every test runs against the real Ootle engine.

mod common;

use common::*;
use tari_template_test_tooling::{
    crypto::{RistrettoSecretKey, SecretKey as _},
    template_lib_types::{Amount, ComponentAddress},
    transaction::args,
};

// ---------------------------------------------------------------- encrypted pay stubs

#[test]
fn employee_reads_their_own_pay_stub_straight_from_the_chain() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    let mask = p.pay(&alice, 3_000, "Oct wk1: 36h night shift + 4h overtime");

    // No off-chain hand-off: Alice's key opens the encrypted data on her paycheck output.
    let stubs = p.read_stubs(alice.account, &alice.secret.clone());
    assert_eq!(stubs.len(), 1);
    assert_eq!(stubs[0].amount, 3_000);
    assert_eq!(stubs[0].mask, mask);
    assert_eq!(stubs[0].memo.as_deref(), Some("Oct wk1: 36h night shift + 4h overtime"));
}

#[test]
fn auditor_sees_amounts_but_not_the_private_memo() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "medical leave next week");

    // The view key opens the amount (ElGamal viewable balance)...
    assert_eq!(p.audit(alice.account, &p.auditor_secret.clone()), Some(3_000));
    // ...but it is not the employee's key, so the stub and its memo stay sealed.
    assert!(p.read_stubs(alice.account, &p.auditor_secret.clone()).is_empty());
}

#[test]
fn coworkers_cannot_open_each_others_stubs() {
    let mut p = setup();
    let alice = p.new_staff();
    let bob = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("hire", args![bob.account, "security guard"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "alice");
    p.pay(&bob, 2_500, "bob");

    assert!(p.read_stubs(alice.account, &bob.secret.clone()).is_empty(), "Bob can't read Alice's pay");
    assert!(p.read_stubs(bob.account, &alice.secret.clone()).is_empty(), "Alice can't read Bob's pay");
    assert_eq!(p.audit(alice.account, &bob.secret.clone()), None);
    assert_eq!(p.read_stubs(bob.account, &bob.secret.clone())[0].amount, 2_500);
}

#[test]
fn employer_rebuilds_its_treasury_from_chain_state_alone() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "wk1");

    // The employer "loses" its local bookkeeping and recovers it with its key from the chain.
    p.treasury.clear();
    p.recover_treasury_from_chain();
    assert_eq!(p.treasury_value(), 7_000);

    // ...and can keep running payroll with the recovered masks.
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "wk2");
    assert_eq!(p.audit(alice.account, &p.auditor_secret.clone()), Some(6_000));
}

// ---------------------------------------------------------------- payslips (selective disclosure)

#[test]
fn employee_proves_their_salary_to_a_landlord_with_a_payslip() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "wk1");

    // Alice shares one opening, privately, with one landlord. Nobody else learns anything.
    let stub = p.read_stubs(alice.account, &alice.secret.clone()).pop().unwrap();
    let slip = Payslip { employee: alice.account, commitment: stub.commitment, amount: stub.amount, mask: stub.mask };
    assert!(p.verify_payslip(&slip), "an honest payslip verifies against the chain");
}

#[test]
fn inflated_or_borrowed_payslips_are_caught() {
    let mut p = setup();
    let alice = p.new_staff();
    let bob = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("hire", args![bob.account, "security guard"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "a");
    p.pay(&bob, 2_500, "b");
    let stub = p.read_stubs(bob.account, &bob.secret.clone()).pop().unwrap();

    // Bob claims he earns 5,000.
    let inflated = Payslip { employee: bob.account, commitment: stub.commitment, amount: 5_000, mask: stub.mask.clone() };
    assert!(!p.verify_payslip(&inflated));
    // Bob passes off his payslip as Alice's (it isn't in her vault).
    let borrowed = Payslip { employee: alice.account, commitment: stub.commitment, amount: 2_500, mask: stub.mask.clone() };
    assert!(!p.verify_payslip(&borrowed));
    // A made-up mask doesn't open anything.
    let forged = Payslip {
        employee: bob.account,
        commitment: stub.commitment,
        amount: 2_500,
        mask: RistrettoSecretKey::random(&mut rand::rng()),
    };
    assert!(!p.verify_payslip(&forged));
}

// ---------------------------------------------------------------- one-transaction shifts

#[test]
fn whole_shift_is_paid_in_one_transaction_with_one_amountless_event() {
    let mut p = setup();
    let alice = p.new_staff();
    let bob = p.new_staff();
    let carol = p.new_staff();
    for (s, role) in [(&alice, "night nurse"), (&bob, "security guard"), (&carol, "gravedigger")] {
        p.employer_call("hire", args![s.account, role]);
    }
    p.employer_call("open_period", args![]);

    let batch = p.build_shift(&[(&alice, 3_000, "a"), (&bob, 2_500, "b"), (&carol, 1_313, "c")]);
    let result = p.employer_tx("pay_shift", args![batch]);

    let topics: Vec<String> = result.finalize.events.iter().map(|e| e.topic().to_string()).collect();
    assert_eq!(topics.iter().filter(|t| t.ends_with("ShiftPaid")).count(), 1, "{topics:?}");
    assert_eq!(topics.iter().filter(|t| t.ends_with("PaycheckIssued")).count(), 0, "no per-person events");
    let shift_event = result.finalize.events.iter().find(|e| e.topic().ends_with("ShiftPaid")).unwrap();
    let payload = shift_event.payload();
    let keys: Vec<&str> = payload.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["paychecks", "period"], "the event carries a head count and a period, nothing else");
    assert_eq!(payload.get_str("paychecks"), Some("3"));
    assert_eq!(payload.get_str("period"), Some("1"));

    assert_eq!(p.audit(alice.account, &p.auditor_secret.clone()), Some(3_000));
    assert_eq!(p.audit(bob.account, &p.auditor_secret.clone()), Some(2_500));
    assert_eq!(p.audit(carol.account, &p.auditor_secret.clone()), Some(1_313));
    assert_eq!(p.audit(p.payroll, &p.auditor_secret.clone()), Some(BUDGET - 6_813));
    assert_eq!(p.read_stubs(carol.account, &carol.secret.clone())[0].memo.as_deref(), Some("c"));
}

#[test]
fn a_shift_with_one_bad_paycheck_pays_nobody() {
    let mut p = setup();
    let alice = p.new_staff();
    let bob = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("hire", args![bob.account, "security guard"]);
    p.employer_call("terminate", args![bob.account]);
    p.employer_call("open_period", args![]);
    let before = p.treasury.clone();

    let batch = p.build_shift(&[(&alice, 3_000, "a"), (&bob, 2_500, "b")]);
    let reason = p.employer_call_fails("pay_shift", args![batch]);
    assert!(reason.contains("not on active payroll"), "{reason}");

    // Atomic: Alice wasn't paid either and the treasury is untouched.
    p.treasury = before;
    assert_eq!(p.audit(alice.account, &p.auditor_secret.clone()), Some(0));
    assert_eq!(p.audit(p.payroll, &p.auditor_secret.clone()), Some(BUDGET));
    let paid: bool = p.test.call_method(p.payroll, "is_paid_this_period", args![alice.account], vec![]);
    assert!(!paid);
}

#[test]
fn the_same_employee_twice_in_one_shift_is_rejected() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    let batch = p.build_shift(&[(&alice, 1_000, "a"), (&alice, 1_000, "a again")]);
    let reason = p.employer_call_fails("pay_shift", args![batch]);
    assert!(reason.contains("already been paid"), "{reason}");
}

#[test]
fn an_empty_shift_is_rejected() {
    let mut p = setup();
    p.employer_call("open_period", args![]);
    let empty: Vec<(ComponentAddress, tari_template_test_tooling::template_lib_types::confidential::ConfidentialWithdrawProof)> = vec![];
    let reason = p.employer_call_fails("pay_shift", args![empty]);
    assert!(reason.contains("at least one paycheck"), "{reason}");
}

// ---------------------------------------------------------------- key limits and access control

#[test]
fn the_auditor_view_key_has_no_spending_power() {
    let mut p = setup();
    let alice = p.new_staff();
    let auditor = p.new_staff(); // the auditor also has an ordinary account
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);

    // Knowing every amount doesn't let the auditor move a cent: pay is employer-gated...
    let (proof, ..) = p.build_paycheck(&auditor.public, 1_000, "skim");
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![auditor.account, proof])
        .build_and_seal(&auditor.secret);
    p.test.execute_expect_failure(tx, vec![auditor.proof.clone()]);
    // ...and the treasury vault has no withdraw method at all.
    let (proof, ..) = p.build_paycheck(&auditor.public, 1_000, "skim");
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "withdraw_confidential", args![p.resource, proof])
        .build_and_seal(&auditor.secret);
    p.test.execute_expect_failure(tx, vec![auditor.proof.clone()]);
    assert_eq!(p.audit(p.payroll, &p.auditor_secret.clone()), Some(BUDGET));
}

#[test]
fn outsiders_cannot_open_periods_top_up_terminate_or_run_shifts() {
    let mut p = setup();
    let alice = p.new_staff();
    let mallory = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);

    let employer_pk = p.employer_pk.clone();
    let (statement, _m, proofs) = budget_statement(1_000_000, &employer_pk, &p.auditor_pk.clone());
    let batch = p.build_shift(&[(&mallory, 1_000, "m")]);
    let attempts = vec![
        ("open_period", args![]),
        ("terminate", args![alice.account]),
        ("top_up", args![statement, proofs]),
        ("pay_shift", args![batch]),
    ];
    for (method, a) in attempts {
        let tx = p.test.transaction().call_method(p.payroll, method, a).build_and_seal(&mallory.secret);
        p.test.execute_expect_failure(tx, vec![mallory.proof.clone()]);
    }
    let period: u64 = p.test.call_method(p.payroll, "period", args![], vec![]);
    assert_eq!(period, 1);
    let budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    assert_eq!(budget, Some(Amount::from(BUDGET)));
}

#[test]
fn internal_helpers_are_not_callable() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    let (proof, ..) = p.build_paycheck(&alice.public, 1_000, "x");
    // Even the employer can't reach the internal paycheck routine directly.
    p.employer_call_fails("issue_paycheck", args![alice.account, proof]);
}

#[test]
fn paying_before_any_period_is_opened_fails() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    let (proof, ..) = p.build_paycheck(&alice.public, 1_000, "x");
    let reason = p.employer_call_fails("pay", args![alice.account, proof]);
    assert!(reason.contains("No pay period is open"), "{reason}");
}

#[test]
fn hiring_twice_fails_but_rehiring_after_termination_works() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    let reason = p.employer_call_fails("hire", args![alice.account, "night nurse"]);
    assert!(reason.contains("already on staff"), "{reason}");
    p.employer_call("terminate", args![alice.account]);
    p.employer_call("hire", args![alice.account, "head nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 1_000, "back");
    let roster: Vec<(ComponentAddress, String, bool, u64)> = p.test.call_method(p.payroll, "roster", args![], vec![]);
    assert_eq!(roster, vec![(alice.account, "head nurse".to_string(), true, 1)]);
}

// ---------------------------------------------------------------- whole-ledger checks

#[test]
fn a_salary_never_appears_anywhere_in_the_transaction_receipt() {
    // A distinctive salary: 0x0BADF00D. We scan the whole receipt (substate diff, events, logs)
    // for it in decimal and as little-endian bytes.
    const SALARY: u64 = 0x0BAD_F00D;
    let mut p = setup_with_budget(SALARY + 1_000);
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    let (proof, ..) = p.build_paycheck(&alice.public, SALARY, "pay");
    let result = p.employer_tx("pay", args![alice.account, proof]);

    let receipt = format!("{result:?}");
    assert!(!receipt.contains(&SALARY.to_string()), "decimal salary in receipt");
    let le = SALARY.to_le_bytes();
    let le_debug = format!("{}, {}, {}, {}", le[0], le[1], le[2], le[3]);
    assert!(!receipt.contains(&le_debug), "LE salary bytes in receipt");
    let hex: String = le.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert!(!receipt.contains(&hex[..8]), "hex salary in receipt");
    // The stub still decrypts for Alice.
    assert_eq!(p.read_stubs(alice.account, &alice.secret.clone())[0].amount, SALARY);
}

#[test]
fn books_reconcile_across_a_whole_month_of_shifts() {
    let mut p = setup();
    let alice = p.new_staff();
    let bob = p.new_staff();
    let carol = p.new_staff();
    for (s, role) in [(&alice, "night nurse"), (&bob, "security guard"), (&carol, "gravedigger")] {
        p.employer_call("hire", args![s.account, role]);
    }
    let employer_pk = p.employer_pk.clone();
    let auditor_pk = p.auditor_pk.clone();
    let mut paid = 0u64;

    // Week 1 and 2: everybody.
    for week in 1..=2 {
        p.employer_call("open_period", args![]);
        let batch = p.build_shift(&[(&alice, 1_000, "a"), (&bob, 800, "b"), (&carol, 600, "c")]);
        p.employer_call("pay_shift", args![batch]);
        paid += 2_400;
        let _ = week;
    }
    // Week 3: Bob leaves, Alice gets a raise, and the employer tops up.
    p.employer_call("terminate", args![bob.account]);
    let (statement, mask, proofs) = budget_statement(5_000, &employer_pk, &auditor_pk);
    p.employer_call("top_up", args![statement, proofs]);
    p.treasury.push((mask, 5_000));
    p.employer_call("open_period", args![]);
    let batch = p.build_shift(&[(&alice, 1_200, "raise!"), (&carol, 600, "c")]);
    p.employer_call("pay_shift", args![batch]);
    paid += 1_800;

    let a = p.audit(alice.account, &p.auditor_secret.clone()).unwrap();
    let b = p.audit(bob.account, &p.auditor_secret.clone()).unwrap();
    let c = p.audit(carol.account, &p.auditor_secret.clone()).unwrap();
    let t = p.audit(p.payroll, &p.auditor_secret.clone()).unwrap();
    assert_eq!((a, b, c), (3_200, 1_600, 1_800));
    assert_eq!(a + b + c, paid);
    let budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    assert_eq!(Some(Amount::from(a + b + c + t)), budget, "Σ paychecks + treasury == public budget");
    let issued: u64 = p.test.call_method(p.payroll, "paychecks_issued", args![], vec![]);
    assert_eq!(issued, 8);
    // Alice's wallet shows three stubs, newest memo included.
    let stubs = p.read_stubs(alice.account, &alice.secret.clone());
    assert_eq!(stubs.len(), 3);
    assert!(stubs.iter().any(|s| s.memo.as_deref() == Some("raise!")));
}
