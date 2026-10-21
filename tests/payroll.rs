//! End-to-end tests for Graveyard Shift, run against the real Ootle engine through `tari_template_test_tooling`.
//!
//! Cast:
//! * employer: the test harness default key; it instantiates the payroll.
//! * alice and bob: employees, each with their own account and key.
//! * auditor: holds the secret view key and nothing else.
//! * mallory: an outsider who tries to pay herself.

use std::collections::BTreeMap;

use tari_template_test_tooling::{
    TemplateTest,
    byte_type::ToByteType,
    crypto::{PublicKey as _, RistrettoPublicKey, RistrettoSecretKey},
    engine_types::crypto::{ElgamalVerifiableBalance, OutputBody, ValueLookup},
    support::{
        GenerateValueLookup,
        confidential::{generate_confidential_proof_with_view_key, generate_withdraw_proof_with_view_key},
        value_proof::value_proofs_for_commitment,
    },
    template_lib_types::{
        Amount,
        ComponentAddress,
        ResourceAddress,
        confidential::ConfidentialOutputStatement,
        crypto::{CommitmentValueProof, PedersenCommitmentBytes, RistrettoPublicKeyBytes},
    },
    transaction::{Transaction, args},
};

type ValueProofs = BTreeMap<PedersenCommitmentBytes, CommitmentValueProof>;

const BUDGET: u64 = 10_000;

struct Payroll {
    test: TemplateTest,
    payroll: ComponentAddress,
    resource: ResourceAddress,
    auditor_secret: RistrettoSecretKey,
    auditor_pk: RistrettoPublicKey,
    /// Mask of the single treasury commitment, tracked like the employer's wallet would.
    treasury_mask: RistrettoSecretKey,
    treasury_value: u64,
}

fn budget_statement(amount: u64, view_key: &RistrettoPublicKey) -> (ConfidentialOutputStatement, RistrettoSecretKey, ValueProofs) {
    let (statement, mask, _change) = generate_confidential_proof_with_view_key(amount, None, view_key);
    let proofs = value_proofs_for_commitment(amount, &mask);
    (statement, mask, proofs)
}

fn setup() -> Payroll {
    let mut test = TemplateTest::my_crate();
    let (auditor_secret, auditor_pk) = RistrettoPublicKey::random_keypair(&mut rand::rng());
    let auditor_vk: RistrettoPublicKeyBytes = auditor_pk.to_byte_type();

    let (statement, mask, proofs) = budget_statement(BUDGET, &auditor_pk);
    let payroll: ComponentAddress =
        test.call_function("GraveyardShift", "new", args![auditor_vk, statement, proofs], vec![]);
    let resource: ResourceAddress = test.call_method(payroll, "payroll_resource", args![], vec![]);

    Payroll {
        test,
        payroll,
        resource,
        auditor_secret,
        auditor_pk,
        treasury_mask: mask,
        treasury_value: BUDGET,
    }
}

impl Payroll {
    fn employer_call(&mut self, method: &str, args: Vec<tari_template_test_tooling::transaction::builder::named_args::NamedArg>) {
        let tx = self
            .test
            .transaction()
            .call_method(self.payroll, method, args)
            .build_and_seal(self.test.secret_key());
        let proof = self.test.owner_proof();
        self.test.execute_expect_success(tx, vec![proof]);
    }

    /// Builds a confidential paycheck of `salary` from the treasury commitment, sends change back to the treasury.
    /// Returns the paycheck's output mask, which the employer would hand to the employee off chain (a real wallet
    /// puts it in the output's encrypted data).
    fn pay(&mut self, employee: ComponentAddress, salary: u64) -> RistrettoSecretKey {
        let change = self.treasury_value - salary;
        let proof = generate_withdraw_proof_with_view_key(
            &self.treasury_mask,
            self.treasury_value,
            salary,
            Some(change),
            0u64,
            &self.auditor_pk,
        );
        self.employer_call("pay", args![employee, proof.proof]);
        self.treasury_mask = proof.change_mask.expect("change mask");
        self.treasury_value = change;
        proof.output_mask
    }

    fn outputs_of(&self, component: ComponentAddress) -> Vec<OutputBody> {
        let store = self.test.read_only_state_store();
        store
            .get_vaults_for_component(component)
            .unwrap()
            .into_values()
            .filter(|v| *v.resource_address() == self.resource)
            .flat_map(|v| store.get_confidential_output_bodies(&v).unwrap())
            .collect()
    }

    /// What the auditor can recover with the secret view key.
    fn audit(&self, component: ComponentAddress, key: &RistrettoSecretKey) -> Option<u64> {
        decrypt_sum(&self.outputs_of(component), key, &GenerateValueLookup::new(0..=BUDGET * 2))
    }
}

fn decrypt_sum<L: ValueLookup>(outputs: &[OutputBody], key: &RistrettoSecretKey, lookup: &L) -> Option<u64> {
    let balances = outputs
        .iter()
        .filter_map(|o| o.viewable_balance.as_ref().map(|vb| vb.try_into().unwrap()))
        .collect::<Vec<_>>();
    if balances.is_empty() {
        return Some(0);
    }
    let decrypted = ElgamalVerifiableBalance::decrypt_many(key, &balances, lookup).ok()?;
    decrypted.into_iter().sum()
}

#[test]
fn salaries_are_hidden_but_the_auditor_can_reconcile_the_books() {
    let mut p = setup();
    let (alice, _alice_proof, _alice_key) = p.test.create_empty_account();
    let (bob, _bob_proof, _bob_key) = p.test.create_empty_account();

    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("hire", args![bob, "security guard"]);
    p.employer_call("open_period", args![]);
    p.pay(alice, 3_000);
    p.pay(bob, 2_500);

    // Public view: the budget and who got paid are visible.
    let public_budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    assert_eq!(public_budget, Some(Amount::from(BUDGET)));
    let issued: u64 = p.test.call_method(p.payroll, "paychecks_issued", args![], vec![]);
    assert_eq!(issued, 2);
    let alice_paid: bool = p.test.call_method(p.payroll, "is_paid_this_period", args![alice], vec![]);
    assert!(alice_paid);

    // Each employee vault holds one confidential commitment: no revealed balance anywhere.
    for who in [alice, bob] {
        let outputs = p.outputs_of(who);
        assert_eq!(outputs.len(), 1, "one paycheck commitment per employee");
    }

    // The auditor decrypts every salary and the treasury with the view key...
    let alice_pay = p.audit(alice, &p.auditor_secret.clone());
    let bob_pay = p.audit(bob, &p.auditor_secret.clone());
    let treasury = p.audit(p.payroll, &p.auditor_secret.clone());
    assert_eq!(alice_pay, Some(3_000));
    assert_eq!(bob_pay, Some(2_500));
    assert_eq!(treasury, Some(4_500));
    // ...and reconciles: paychecks + treasury == public budget.
    assert_eq!(alice_pay.unwrap() + bob_pay.unwrap() + treasury.unwrap(), BUDGET);

    // Anyone else, e.g. a curious coworker with their own key, can't.
    let (nosy_secret, _) = RistrettoPublicKey::random_keypair(&mut rand::rng());
    assert_eq!(p.audit(alice, &nosy_secret), None, "a non-auditor key must not decrypt salaries");
}

#[test]
fn paycheck_events_never_contain_amounts() {
    let mut p = setup();
    let (alice, _, _) = p.test.create_empty_account();
    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("open_period", args![]);

    let proof = generate_withdraw_proof_with_view_key(&p.treasury_mask, BUDGET, 4_321, Some(BUDGET - 4_321), 0u64, &p.auditor_pk);
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![alice, proof.proof])
        .build_and_seal(p.test.secret_key());
    let owner = p.test.owner_proof();
    let result = p.test.execute_expect_success(tx, vec![owner]);

    let paycheck_events: Vec<_> = result
        .finalize
        .events
        .iter()
        .filter(|e| e.topic().ends_with("PaycheckIssued"))
        .collect();
    assert_eq!(paycheck_events.len(), 1);
    let rendered = format!("{:?}", paycheck_events[0].payload());
    assert!(!rendered.contains("4321"), "salary leaked into event: {rendered}");
    assert!(rendered.contains("period"));
}

#[test]
fn employee_can_spend_their_paycheck() {
    let mut p = setup();
    let (alice, alice_proof, alice_key) = p.test.create_empty_account();
    let (landlord, _, _) = p.test.create_empty_account();
    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("open_period", args![]);
    let paycheck_mask = p.pay(alice, 3_000);

    // Alice pays rent of 1,200 out of her paycheck, confidentially, keeping 1,800.
    let rent = generate_withdraw_proof_with_view_key(&paycheck_mask, 3_000, 1_200, Some(1_800), 0u64, &p.auditor_pk);
    let tx = Transaction::builder_localnet(Default::default())
        .call_method(alice, "withdraw_confidential", args![p.resource, rent.proof])
        .put_last_instruction_output_on_workspace("rent")
        .call_method(landlord, "deposit", args![Workspace("rent")])
        .build_and_seal(&alice_key);
    p.test.execute_expect_success(tx, vec![alice_proof]);

    assert_eq!(p.audit(alice, &p.auditor_secret.clone()), Some(1_800));
    assert_eq!(p.audit(landlord, &p.auditor_secret.clone()), Some(1_200));
}

#[test]
fn cannot_pay_the_same_employee_twice_in_one_period() {
    let mut p = setup();
    let (alice, _, _) = p.test.create_empty_account();
    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("open_period", args![]);
    p.pay(alice, 1_000);

    let proof = generate_withdraw_proof_with_view_key(&p.treasury_mask, p.treasury_value, 1_000, Some(p.treasury_value - 1_000), 0u64, &p.auditor_pk);
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![alice, proof.proof])
        .build_and_seal(p.test.secret_key());
    let owner = p.test.owner_proof();
    let reason = p.test.execute_expect_failure(tx, vec![owner]);
    assert!(format!("{reason:?}").contains("already been paid"), "{reason:?}");

    // Next period it works again.
    p.employer_call("open_period", args![]);
    p.pay(alice, 1_000);
    assert_eq!(p.audit(alice, &p.auditor_secret.clone()), Some(2_000));
}

#[test]
fn revealed_paychecks_are_rejected_so_salaries_cannot_leak() {
    let mut p = setup();
    let (alice, _, _) = p.test.create_empty_account();
    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("open_period", args![]);

    // A proof that reveals 1,000 in the clear (would publish the salary).
    let proof = generate_withdraw_proof_with_view_key(&p.treasury_mask, BUDGET, 0, Some(BUDGET - 1_000), 1_000u64, &p.auditor_pk);
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![alice, proof.proof])
        .build_and_seal(p.test.secret_key());
    let owner = p.test.owner_proof();
    let reason = p.test.execute_expect_failure(tx, vec![owner]);
    assert!(format!("{reason:?}").contains("fully confidential"), "{reason:?}");
}

#[test]
fn outsiders_cannot_hire_or_pay() {
    let mut p = setup();
    let (mallory, mallory_proof, mallory_key) = p.test.create_empty_account();
    p.employer_call("open_period", args![]);

    // Mallory tries to add herself to payroll.
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "hire", args![mallory, "definitely staff"])
        .build_and_seal(&mallory_key);
    p.test.execute_expect_failure(tx, vec![mallory_proof.clone()]);

    // Even if she somehow knew the treasury mask, she can't call pay.
    let proof = generate_withdraw_proof_with_view_key(&p.treasury_mask, BUDGET, 5_000, Some(5_000), 0u64, &p.auditor_pk);
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![mallory, proof.proof])
        .build_and_seal(&mallory_key);
    p.test.execute_expect_failure(tx, vec![mallory_proof]);
    assert_eq!(p.audit(mallory, &p.auditor_secret.clone()), Some(0));
}

#[test]
fn terminated_staff_are_not_paid() {
    let mut p = setup();
    let (alice, _, _) = p.test.create_empty_account();
    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("terminate", args![alice]);
    p.employer_call("open_period", args![]);

    let proof = generate_withdraw_proof_with_view_key(&p.treasury_mask, BUDGET, 1_000, Some(BUDGET - 1_000), 0u64, &p.auditor_pk);
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![alice, proof.proof])
        .build_and_seal(p.test.secret_key());
    let owner = p.test.owner_proof();
    let reason = p.test.execute_expect_failure(tx, vec![owner]);
    assert!(format!("{reason:?}").contains("not on active payroll"), "{reason:?}");
    let staff: u64 = p.test.call_method(p.payroll, "staff_count", args![], vec![]);
    assert_eq!(staff, 0);
}

#[test]
fn top_up_grows_the_public_budget_only() {
    let mut p = setup();
    let (statement, _mask, proofs) = budget_statement(5_000, &p.auditor_pk);
    p.employer_call("top_up", args![statement, proofs]);
    let public_budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    assert_eq!(public_budget, Some(Amount::from(BUDGET + 5_000)));
    assert_eq!(p.audit(p.payroll, &p.auditor_secret.clone()), Some(BUDGET + 5_000));
    let commitments: u64 = p.test.call_method(p.payroll, "treasury_commitment_count", args![], vec![]);
    assert_eq!(commitments, 2);
}

#[test]
fn employer_cannot_hide_a_paycheck_from_the_auditor() {
    use tari_template_test_tooling::support::confidential::generate_withdraw_proof;
    let mut p = setup();
    let (alice, _, _) = p.test.create_empty_account();
    p.employer_call("hire", args![alice, "night nurse"]);
    p.employer_call("open_period", args![]);

    // Same paycheck, but built without the auditor's view key (no viewable balance proof on the outputs).
    let proof = generate_withdraw_proof(&p.treasury_mask, 1_000, Some(BUDGET - 1_000), 0u64);
    let tx = p
        .test
        .transaction()
        .call_method(p.payroll, "pay", args![alice, proof.proof])
        .build_and_seal(p.test.secret_key());
    let owner = p.test.owner_proof();
    let reason = p.test.execute_expect_failure(tx, vec![owner]);
    assert!(
        format!("{reason:?}").contains("ViewableBalanceProof is required"),
        "engine must refuse paychecks the auditor cannot read: {reason:?}"
    );
}
