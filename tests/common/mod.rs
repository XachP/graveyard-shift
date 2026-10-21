//! Shared harness for the Graveyard Shift tests and the narrated demo.
//!
//! It plays the client side of every role the way a real wallet would:
//! * the **employer** builds confidential paychecks, encrypts each pay stub to the employee and the
//!   treasury change back to itself, so its treasury state can be recovered from the chain alone;
//! * the **employee** scans their vault and decrypts their own pay stub (amount, mask, memo);
//! * the **auditor** decrypts every amount with the resource view key and reconciles the books;
//! * a **verifier** (landlord, lender) checks a payslip opening against the on-chain commitment.
#![allow(dead_code)]

use std::collections::BTreeMap;

use tari_template_test_tooling::{
    TemplateTest,
    byte_type::{FromByteType, ToByteType},
    crypto::{PublicKey as _, RistrettoPublicKey, RistrettoSecretKey, SecretKey as _},
    engine_types::{
        commit_result::ExecuteResult,
        confidential::MIN_CONFIDENTIAL_OUTPUT_VALUE_PROMISE,
        crypto::{ElgamalVerifiableBalance, OutputBody, ValueLookup, commit_amount},
    },
    support::{GenerateValueLookup, confidential::generate_withdraw_proof_with_view_key, value_proof::value_proofs_for_commitment},
    template_lib_types::{
        Amount,
        ComponentAddress,
        ConfidentialOutputAddress,
        ResourceAddress,
        confidential::{ConfidentialOutputStatement, ConfidentialWithdrawProof},
        crypto::{CommitmentValueProof, PedersenCommitmentBytes, RistrettoPublicKeyBytes},
    },
    transaction::{Transaction, args, builder::named_args::NamedArg},
    wallet_crypto::{MaskAndValue, OutputWitness, StealthCryptoApi, confidential, memo::Memo},
};

pub type ValueProofs = BTreeMap<PedersenCommitmentBytes, CommitmentValueProof>;

pub const BUDGET: u64 = 10_000;

pub fn rng() -> impl rand::CryptoRng {
    rand::rng()
}

pub fn keypair() -> (RistrettoSecretKey, RistrettoPublicKey) {
    RistrettoPublicKey::random_keypair(&mut rand::rng())
}

/// A decrypted pay stub, as the employee's wallet recovers it from the chain.
#[derive(Debug, Clone)]
pub struct PayStub {
    pub commitment: PedersenCommitmentBytes,
    pub amount: u64,
    pub mask: RistrettoSecretKey,
    pub memo: Option<String>,
}

/// What an employee hands to a landlord: the opening of one paycheck commitment.
#[derive(Debug, Clone)]
pub struct Payslip {
    pub employee: ComponentAddress,
    pub commitment: PedersenCommitmentBytes,
    pub amount: u64,
    pub mask: RistrettoSecretKey,
}

pub struct Staff {
    pub account: ComponentAddress,
    pub secret: RistrettoSecretKey,
    pub public: RistrettoPublicKey,
    pub proof: tari_template_test_tooling::template_lib_types::NonFungibleAddress,
}

pub struct Payroll {
    pub test: TemplateTest,
    pub payroll: ComponentAddress,
    pub resource: ResourceAddress,
    pub auditor_secret: RistrettoSecretKey,
    pub auditor_pk: RistrettoPublicKey,
    pub employer_secret: RistrettoSecretKey,
    pub employer_pk: RistrettoPublicKey,
    /// The employer wallet's view of the treasury: (mask, value) of each live commitment.
    pub treasury: Vec<(RistrettoSecretKey, u64)>,
}

/// Builds an output witness whose encrypted data is DH-encrypted to `recipient`.
pub fn sealed_witness(
    amount: u64,
    mask: &RistrettoSecretKey,
    recipient: &RistrettoPublicKey,
    memo: Option<&str>,
    view_key: &RistrettoPublicKey,
) -> OutputWitness {
    let (nonce_secret, nonce_public) = keypair();
    let memo = memo.map(|m| Memo::new_message(m).expect("memo fits"));
    let encrypted_data = StealthCryptoApi::new()
        .encrypt_value_and_mask(amount, mask, recipient, &nonce_secret, memo.as_ref())
        .expect("encrypt pay stub");
    OutputWitness {
        amount,
        mask: mask.clone(),
        sender_public_nonce: nonce_public,
        minimum_value_promise: MIN_CONFIDENTIAL_OUTPUT_VALUE_PROMISE,
        encrypted_data,
        resource_view_key: Some(view_key.clone()),
    }
}

/// Opening budget / top-up statement, encrypted to the employer so its wallet can recover it.
pub fn budget_statement(
    amount: u64,
    employer: &RistrettoPublicKey,
    view_key: &RistrettoPublicKey,
) -> (ConfidentialOutputStatement, RistrettoSecretKey, ValueProofs) {
    let mask = RistrettoSecretKey::random(&mut rand::rng());
    let witness = sealed_witness(amount, &mask, employer, Some("treasury"), view_key);
    let statement = confidential::create_output_statement(Some(&witness), Amount::zero(), None, Amount::zero())
        .expect("budget statement");
    let proofs = value_proofs_for_commitment(amount, &mask);
    (statement, mask, proofs)
}

pub fn setup() -> Payroll {
    setup_with_budget(BUDGET)
}

pub fn setup_with_budget(budget: u64) -> Payroll {
    let mut test = TemplateTest::my_crate();
    let (auditor_secret, auditor_pk) = keypair();
    let auditor_vk: RistrettoPublicKeyBytes = auditor_pk.to_byte_type();
    let employer_secret = test.secret_key().clone();
    let employer_pk = RistrettoPublicKey::from_secret_key(&employer_secret);

    let (statement, mask, proofs) = budget_statement(budget, &employer_pk, &auditor_pk);
    let payroll: ComponentAddress =
        test.call_function("GraveyardShift", "new", args![auditor_vk, statement, proofs], vec![]);
    let resource: ResourceAddress = test.call_method(payroll, "payroll_resource", args![], vec![]);

    Payroll {
        test,
        payroll,
        resource,
        auditor_secret,
        auditor_pk,
        employer_secret,
        employer_pk,
        treasury: vec![(mask, budget)],
    }
}

impl Payroll {
    pub fn new_staff(&mut self) -> Staff {
        let (account, proof, secret) = self.test.create_empty_account();
        let public = RistrettoPublicKey::from_secret_key(&secret);
        Staff { account, secret, public, proof }
    }

    pub fn employer_tx(&mut self, method: &str, args: Vec<NamedArg>) -> ExecuteResult {
        let tx = self
            .test
            .transaction()
            .call_method(self.payroll, method, args)
            .build_and_seal(self.test.secret_key());
        let proof = self.test.owner_proof();
        self.test.execute_expect_success(tx, vec![proof])
    }

    pub fn employer_call(&mut self, method: &str, args: Vec<NamedArg>) {
        self.employer_tx(method, args);
    }

    pub fn employer_call_fails(&mut self, method: &str, args: Vec<NamedArg>) -> String {
        let tx = self
            .test
            .transaction()
            .call_method(self.payroll, method, args)
            .build_and_seal(self.test.secret_key());
        let proof = self.test.owner_proof();
        format!("{:?}", self.test.execute_expect_failure(tx, vec![proof]))
    }

    pub fn treasury_value(&self) -> u64 {
        self.treasury.iter().map(|(_, v)| v).sum()
    }

    /// Builds one fully confidential paycheck that spends the whole tracked treasury, pays
    /// `salary` to `employee` (stub encrypted to them) and returns change to the treasury
    /// (encrypted to the employer). Returns the proof, the paycheck mask and the change mask.
    pub fn build_paycheck(
        &self,
        employee: &RistrettoPublicKey,
        salary: u64,
        memo: &str,
    ) -> (ConfidentialWithdrawProof, RistrettoSecretKey, RistrettoSecretKey, u64) {
        let total = self.treasury_value();
        let change = total.checked_sub(salary).expect("treasury too small");
        let pay_mask = RistrettoSecretKey::random(&mut rand::rng());
        let change_mask = RistrettoSecretKey::random(&mut rand::rng());
        let output = sealed_witness(salary, &pay_mask, employee, Some(memo), &self.auditor_pk);
        let change_out = sealed_witness(change, &change_mask, &self.employer_pk, Some("treasury"), &self.auditor_pk);
        let inputs = self
            .treasury
            .iter()
            .map(|(mask, value)| MaskAndValue { value: *value, mask: mask.clone() })
            .collect::<Vec<_>>();
        let proof = confidential::create_withdraw_proof(
            &inputs,
            Amount::zero(),
            Some(&output),
            Amount::zero(),
            Some(&change_out),
            Amount::zero(),
        )
        .expect("withdraw proof");
        (proof, pay_mask, change_mask, change)
    }

    /// Pays one employee with an encrypted pay stub. Returns the paycheck mask.
    pub fn pay(&mut self, staff: &Staff, salary: u64, memo: &str) -> RistrettoSecretKey {
        let (proof, pay_mask, change_mask, change) = self.build_paycheck(&staff.public, salary, memo);
        self.employer_call("pay", args![staff.account, proof]);
        self.treasury = vec![(change_mask, change)];
        pay_mask
    }

    /// Builds the paychecks for a whole shift, chaining treasury change from one to the next.
    pub fn build_shift(&mut self, shift: &[(&Staff, u64, &str)]) -> Vec<(ComponentAddress, ConfidentialWithdrawProof)> {
        let mut batch = Vec::new();
        for (staff, salary, memo) in shift {
            let (proof, _pay_mask, change_mask, change) = self.build_paycheck(&staff.public, *salary, memo);
            self.treasury = vec![(change_mask, change)];
            batch.push((staff.account, proof));
        }
        batch
    }

    pub fn outputs_with_commitments(&self, component: ComponentAddress) -> Vec<(PedersenCommitmentBytes, OutputBody)> {
        let store = self.test.read_only_state_store();
        let mut out = Vec::new();
        for vault in store.get_vaults_for_component(component).unwrap().into_values() {
            if *vault.resource_address() != self.resource {
                continue;
            }
            for commitment in vault.get_confidential_commitments().into_iter().flatten() {
                let addr = ConfidentialOutputAddress::new(self.resource, *commitment);
                let body = store.get_confidential_output(addr).unwrap().into_output();
                out.push((*commitment, body));
            }
        }
        out
    }

    pub fn outputs_of(&self, component: ComponentAddress) -> Vec<OutputBody> {
        self.outputs_with_commitments(component).into_iter().map(|(_, b)| b).collect()
    }

    /// Auditor view: sum of all amounts in `component` decryptable with `key` (the view key).
    pub fn audit(&self, component: ComponentAddress, key: &RistrettoSecretKey) -> Option<u64> {
        decrypt_sum(&self.outputs_of(component), key, &GenerateValueLookup::new(0..=BUDGET * 4))
    }

    /// Auditor view, per commitment.
    pub fn audit_each(&self, component: ComponentAddress, key: &RistrettoSecretKey) -> Option<Vec<u64>> {
        self.outputs_of(component)
            .iter()
            .map(|o| decrypt_sum(std::slice::from_ref(o), key, &GenerateValueLookup::new(0..=BUDGET * 4)))
            .collect()
    }

    /// Wallet view: every pay stub in `component` that `secret` can open.
    pub fn read_stubs(&self, component: ComponentAddress, secret: &RistrettoSecretKey) -> Vec<PayStub> {
        self.outputs_with_commitments(component)
            .into_iter()
            .filter_map(|(commitment, body)| {
                let nonce: RistrettoPublicKey = body.public_nonce.try_from_byte_type().ok()?;
                let decrypted = StealthCryptoApi::new()
                    .decrypt_utxo_data(&body.encrypted_data, &commitment, secret, &nonce, false)
                    .ok()?;
                Some(PayStub {
                    commitment,
                    amount: decrypted.mask_and_value.value,
                    mask: decrypted.mask_and_value.mask.clone(),
                    memo: decrypted.memo.as_ref().and_then(|m| m.as_memo_message().map(str::to_string)),
                })
            })
            .collect()
    }

    /// Rebuilds the employer's treasury bookkeeping from chain state and the employer key alone.
    pub fn recover_treasury_from_chain(&mut self) {
        let stubs = self.read_stubs(self.payroll, &self.employer_secret.clone());
        self.treasury = stubs.into_iter().map(|s| (s.mask, s.amount)).collect();
    }

    /// Landlord / lender check: does this payslip open a commitment that really sits in the
    /// employee's vault? Uses only public chain state plus the payslip.
    pub fn verify_payslip(&self, slip: &Payslip) -> bool {
        let Some(c) = commit_amount(&slip.mask, Amount::from(slip.amount)) else {
            return false;
        };
        let c: PedersenCommitmentBytes = c.to_byte_type();
        c == slip.commitment &&
            self.outputs_with_commitments(slip.employee)
                .iter()
                .any(|(onchain, _)| *onchain == slip.commitment)
    }

    pub fn employee_spend(
        &mut self,
        staff: &Staff,
        stub: &PayStub,
        amount: u64,
        to: ComponentAddress,
        to_pk: &RistrettoPublicKey,
    ) -> RistrettoSecretKey {
        let change = stub.amount - amount;
        let out_mask = RistrettoSecretKey::random(&mut rand::rng());
        let change_mask = RistrettoSecretKey::random(&mut rand::rng());
        let output = sealed_witness(amount, &out_mask, to_pk, Some("rent"), &self.auditor_pk);
        let change_out = sealed_witness(change, &change_mask, &staff.public, Some("change"), &self.auditor_pk);
        let proof = confidential::create_withdraw_proof(
            &[MaskAndValue { value: stub.amount, mask: stub.mask.clone() }],
            Amount::zero(),
            Some(&output),
            Amount::zero(),
            Some(&change_out),
            Amount::zero(),
        )
        .unwrap();
        let tx = Transaction::builder_localnet(Default::default())
            .call_method(staff.account, "withdraw_confidential", args![self.resource, proof])
            .put_last_instruction_output_on_workspace("rent")
            .call_method(to, "deposit", args![Workspace("rent")])
            .build_and_seal(&staff.secret);
        self.test.execute_expect_success(tx, vec![staff.proof.clone()]);
        out_mask
    }

    /// Old-style helper kept for the original tests: a paycheck with no encrypted stub.
    pub fn plain_paycheck(&self, salary: u64, revealed: u64) -> ConfidentialWithdrawProof {
        let (mask, value) = self.treasury[0].clone();
        generate_withdraw_proof_with_view_key(&mask, value, salary, Some(value - salary - revealed), revealed, &self.auditor_pk).proof
    }
}

pub fn decrypt_sum<L: ValueLookup>(outputs: &[OutputBody], key: &RistrettoSecretKey, lookup: &L) -> Option<u64> {
    let balances = outputs
        .iter()
        .filter_map(|o| o.viewable_balance.as_ref().map(|vb| vb.try_into().unwrap()))
        .collect::<Vec<ElgamalVerifiableBalance>>();
    if balances.is_empty() {
        return Some(0);
    }
    let decrypted = ElgamalVerifiableBalance::decrypt_many(key, &balances, lookup).ok()?;
    decrypted.into_iter().sum()
}
