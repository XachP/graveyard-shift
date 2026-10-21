//! Core payroll behaviour, run against the real Ootle engine through `tari_template_test_tooling`.
//!
//! Cast: the employer (harness default key), Alice (night nurse) and Bob (security guard), the
//! auditor (holds only the view key) and Mallory (an outsider).

mod common;

use common::*;
use tari_template_test_tooling::{
    crypto::RistrettoPublicKey,
    support::confidential::generate_withdraw_proof,
    template_lib_types::Amount,
    transaction::args,
};

#[test]
fn salaries_are_hidden_but_the_auditor_can_reconcile_the_books() {
    let mut p = setup();
    let alice = p.new_staff();
    let bob = p.new_staff();

    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("hire", args![bob.account, "security guard"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "Oct wk1 night nurse");
    p.pay(&bob, 2_500, "Oct wk1 security");

    // Public view: the budget and who got paid are visible.
    let public_budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    assert_eq!(public_budget, Some(Amount::from(BUDGET)));
    let issued: u64 = p.test.call_method(p.payroll, "paychecks_issued", args![], vec![]);
    assert_eq!(issued, 2);
    let alice_paid: bool = p.test.call_method(p.payroll, "is_paid_this_period", args![alice.account], vec![]);
    assert!(alice_paid);

    for who in [alice.account, bob.account] {
        assert_eq!(p.outputs_of(who).len(), 1, "one paycheck commitment per employee");
    }

    let a = p.audit(alice.account, &p.auditor_secret.clone());
    let b = p.audit(bob.account, &p.auditor_secret.clone());
    let t = p.audit(p.payroll, &p.auditor_secret.clone());
    assert_eq!((a, b, t), (Some(3_000), Some(2_500), Some(4_500)));
    assert_eq!(a.unwrap() + b.unwrap() + t.unwrap(), BUDGET);

    let (nosy, _) = keypair();
    assert_eq!(p.audit(alice.account, &nosy), None, "a non-auditor key must not decrypt salaries");
}

#[test]
fn paycheck_events_never_contain_amounts() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);

    let (proof, ..) = p.build_paycheck(&alice.public, 4_321, "stub");
    let result = p.employer_tx("pay", args![alice.account, proof]);
    let events: Vec<_> = result
        .finalize
        .events
        .iter()
        .filter(|e| e.topic().ends_with("PaycheckIssued"))
        .collect();
    assert_eq!(events.len(), 1);
    let payload = events[0].payload();
    let keys: Vec<&str> = payload.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["employee", "period"], "no amount field in a paycheck event");
    assert_eq!(payload.get_str("period"), Some("1"));
    for (_, raw) in payload.iter() {
        assert!(!format!("{raw:?}").contains("4321"));
    }
}

#[test]
fn employee_can_spend_their_paycheck() {
    let mut p = setup();
    let alice = p.new_staff();
    let landlord = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 3_000, "Oct wk1");

    // Alice finds her paycheck on-chain with her own key, then pays 1,200 rent confidentially.
    let stub = p.read_stubs(alice.account, &alice.secret.clone()).pop().expect("stub");
    let landlord_pk = landlord.public.clone();
    p.employee_spend(&alice, &stub, 1_200, landlord.account, &landlord_pk);

    assert_eq!(p.audit(alice.account, &p.auditor_secret.clone()), Some(1_800));
    assert_eq!(p.audit(landlord.account, &p.auditor_secret.clone()), Some(1_200));
    // The landlord's wallet can read the incoming rent with the landlord's key.
    let rent = p.read_stubs(landlord.account, &landlord.secret.clone());
    assert_eq!(rent.len(), 1);
    assert_eq!(rent[0].amount, 1_200);
}

#[test]
fn cannot_pay_the_same_employee_twice_in_one_period() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(&alice, 1_000, "wk1");

    let (proof, ..) = p.build_paycheck(&alice.public, 1_000, "wk1 again");
    let reason = p.employer_call_fails("pay", args![alice.account, proof]);
    assert!(reason.contains("already been paid"), "{reason}");

    p.employer_call("open_period", args![]);
    p.pay(&alice, 1_000, "wk2");
    assert_eq!(p.audit(alice.account, &p.auditor_secret.clone()), Some(2_000));
}

#[test]
fn revealed_paychecks_are_rejected_so_salaries_cannot_leak() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);

    // A proof that reveals 1,000 in the clear (would publish the salary).
    let proof = p.plain_paycheck(0, 1_000);
    let reason = p.employer_call_fails("pay", args![alice.account, proof]);
    assert!(reason.contains("fully confidential"), "{reason}");
}

#[test]
fn outsiders_cannot_hire_or_pay() {
    let mut p = setup();
    let mallory = p.new_staff();
    p.employer_call("open_period", args![]);

    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "hire", args![mallory.account, "definitely staff"])
        .build_and_seal(&mallory.secret);
    p.test.execute_expect_failure(tx, vec![mallory.proof.clone()]);

    // Even if she somehow knew the treasury mask, she can't call pay.
    let (proof, ..) = p.build_paycheck(&mallory.public, 5_000, "lol");
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![mallory.account, proof])
        .build_and_seal(&mallory.secret);
    p.test.execute_expect_failure(tx, vec![mallory.proof.clone()]);
    assert_eq!(p.audit(mallory.account, &p.auditor_secret.clone()), Some(0));
}

#[test]
fn terminated_staff_are_not_paid() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("terminate", args![alice.account]);
    p.employer_call("open_period", args![]);

    let (proof, ..) = p.build_paycheck(&alice.public, 1_000, "x");
    let reason = p.employer_call_fails("pay", args![alice.account, proof]);
    assert!(reason.contains("not on active payroll"), "{reason}");
    let staff: u64 = p.test.call_method(p.payroll, "staff_count", args![], vec![]);
    assert_eq!(staff, 0);
}

#[test]
fn top_up_grows_the_public_budget_only() {
    let mut p = setup();
    let employer_pk = p.employer_pk.clone();
    let (statement, _mask, proofs) = budget_statement(5_000, &employer_pk, &p.auditor_pk.clone());
    p.employer_call("top_up", args![statement, proofs]);
    let public_budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    assert_eq!(public_budget, Some(Amount::from(BUDGET + 5_000)));
    assert_eq!(p.audit(p.payroll, &p.auditor_secret.clone()), Some(BUDGET + 5_000));
    let commitments: u64 = p.test.call_method(p.payroll, "treasury_commitment_count", args![], vec![]);
    assert_eq!(commitments, 2);
}

#[test]
fn employer_cannot_hide_a_paycheck_from_the_auditor() {
    let mut p = setup();
    let alice = p.new_staff();
    p.employer_call("hire", args![alice.account, "night nurse"]);
    p.employer_call("open_period", args![]);

    // Same paycheck, but built without the auditor's view key (no viewable balance proof).
    let (mask, value) = p.treasury[0].clone();
    let proof = generate_withdraw_proof(&mask, 1_000, Some(value - 1_000), 0u64);
    let reason = p.employer_call_fails("pay", args![alice.account, proof.proof]);
    assert!(
        reason.contains("ViewableBalanceProof is required"),
        "engine must refuse paychecks the auditor cannot read: {reason}"
    );
    let _ = RistrettoPublicKey::default();
}
