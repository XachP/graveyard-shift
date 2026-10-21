//! # Graveyard Shift: confidential payroll on Tari Ootle
//!
//! Payroll is one of the few things every business runs and nobody wants on a public ledger.
//! On a transparent chain every salary, raise and bonus is readable by coworkers, competitors
//! and anyone with a block explorer. Graveyard Shift keeps the paychecks in the dark:
//!
//! * **Public:** the total payroll budget the employer mints, who is on staff, and which pay
//!   periods each person has been paid for.
//! * **Secret:** every individual salary. Paychecks are Pedersen commitments, so the amount
//!   never appears on-chain, in events or in component state.
//! * **Auditable:** the payroll token is created with an auditor view key. Whoever holds the
//!   matching secret key (an accountant or a tax auditor) can decrypt every commitment and
//!   reconcile `sum(paychecks) + treasury == budget`. The view key gives them no spending power.
//!
//! The template also refuses any payment that would put a salary on-chain in the clear
//! (revealed inputs or outputs), so an employer can't leak someone's pay by accident.
//!
//! License: MIT

use std::collections::BTreeMap;

use tari_template_lib::prelude::*;

#[template]
mod graveyard_shift {
    use super::*;
    use tari_template_lib::types::{
        confidential::{ConfidentialOutputStatement, ConfidentialWithdrawProof},
        crypto::{CommitmentValueProof, PedersenCommitmentBytes},
    };

    pub struct GraveyardShift {
        /// Treasury of confidential payroll tokens. Only commitments live here.
        treasury: Vault,
        /// The employer: the key that instantiated the component.
        employer: RistrettoPublicKeyBytes,
        /// Staff roster, keyed by the employee's account component.
        staff: BTreeMap<ComponentAddress, Employee>,
        /// Current pay period. 0 means payroll has not opened yet.
        period: u64,
        /// Paychecks issued since the component was created. This counts paychecks; it holds no amounts.
        paychecks_issued: u64,
    }

    pub struct Employee {
        /// Free-form public role label, e.g. "night nurse". Never an amount.
        pub role: String,
        pub active: bool,
        /// Last period this employee was paid for. Stops double pay within one period.
        pub last_paid_period: u64,
        pub paychecks: u64,
    }

    impl GraveyardShift {
        /// Opens a payroll.
        ///
        /// * `auditor_view_key`: public view key of the auditor. It is fixed on the resource for
        ///   good, and every commitment carries a balance proof encrypted to it.
        /// * `budget`: confidential output statement for the opening budget, built client side.
        /// * `budget_value_proofs`: value proofs for each budget commitment, so the resource's
        ///   total supply (the public budget) can be tracked.
        pub fn new(
            auditor_view_key: RistrettoPublicKeyBytes,
            budget: ConfidentialOutputStatement,
            budget_value_proofs: BTreeMap<PedersenCommitmentBytes, CommitmentValueProof>,
        ) -> Component<Self> {
            assert!(
                budget.output_revealed_amount.is_zero() && budget.change_revealed_amount.is_zero(),
                "The opening budget must be confidential commitments, not revealed funds"
            );
            let employer = CallerContext::transaction_signer_public_key();
            let alloc = CallerContext::allocate_component_address(None);
            let this = alloc.get_address();

            let budget_bucket = ResourceBuilder::confidential()
                .with_token_symbol("SHIFT")
                .metadata("name", "Graveyard Shift payroll credit")
                .metadata("description", "Confidential payroll token. Salaries are hidden; the auditor holds the view key.")
                // Only this payroll component can mint more budget (via `top_up`, which is employer-gated).
                .mintable(rule!(component(this)), LOCKED)
                .burnable(rule!(deny_all), LOCKED)
                .recallable(rule!(deny_all), LOCKED)
                .with_view_key(auditor_view_key)
                .initial_supply_with_value_proofs(budget, budget_value_proofs);

            let employer_only = rule!(public_key(employer));
            Component::new(Self {
                treasury: Vault::from_bucket(budget_bucket),
                employer,
                staff: BTreeMap::new(),
                period: 0,
                paychecks_issued: 0,
            })
            .with_address_allocation(alloc)
            .with_access_rules(
                ComponentAccessRules::new()
                    .method("hire", employer_only.clone())
                    .method("terminate", employer_only.clone())
                    .method("open_period", employer_only.clone())
                    .method("pay", employer_only.clone())
                    .method("top_up", employer_only.clone())
                    .method("period", rule!(allow_all))
                    .method("is_paid_this_period", rule!(allow_all))
                    .method("staff_count", rule!(allow_all))
                    .method("paychecks_issued", rule!(allow_all))
                    .method("payroll_resource", rule!(allow_all))
                    .method("public_budget", rule!(allow_all))
                    .method("treasury_commitment_count", rule!(allow_all))
                    .default(rule!(deny_all)),
            )
            .create()
        }

        /// Adds an employee. Takes their account component (where paychecks are deposited) and a public role label.
        pub fn hire(&mut self, employee_account: ComponentAddress, role: String) {
            assert!(role.len() <= 64, "Role label too long (max 64 bytes)");
            if let Some(existing) = self.staff.get_mut(&employee_account) {
                assert!(!existing.active, "Employee is already on staff");
                existing.active = true;
                existing.role = role;
            } else {
                self.staff.insert(employee_account, Employee {
                    role,
                    active: true,
                    last_paid_period: 0,
                    paychecks: 0,
                });
            }
            emit_event("Hired", metadata!["employee" => employee_account.to_string()]);
        }

        /// Takes an employee off payroll. Their history (paycheck count) is kept.
        pub fn terminate(&mut self, employee_account: ComponentAddress) {
            let employee = self.staff.get_mut(&employee_account).expect("Not an employee");
            assert!(employee.active, "Employee already terminated");
            employee.active = false;
            emit_event("Terminated", metadata!["employee" => employee_account.to_string()]);
        }

        /// Starts a new pay period. Each active employee can be paid once per period.
        pub fn open_period(&mut self) {
            self.period += 1;
            emit_event("PeriodOpened", metadata!["period" => self.period.to_string()]);
        }

        /// Pays one employee for the current period.
        ///
        /// `paycheck` is a confidential withdraw proof the employer builds client side. It spends
        /// treasury commitments into one output commitment (the paycheck) plus optional change that
        /// stays in the treasury. Any revealed amount is rejected so no salary is ever readable on-chain.
        pub fn pay(&mut self, employee_account: ComponentAddress, paycheck: ConfidentialWithdrawProof) {
            assert!(self.period > 0, "No pay period is open; call open_period first");
            assert!(
                paycheck.revealed_input_amount().is_zero() &&
                    paycheck.revealed_output_amount().is_zero() &&
                    paycheck.revealed_change_amount().is_zero(),
                "Paychecks must be fully confidential: revealed amounts would publish a salary"
            );
            assert!(!paycheck.inputs.is_empty(), "A paycheck must spend treasury commitments");

            let period = self.period;
            let employee = self.staff.get_mut(&employee_account).expect("Not an employee");
            assert!(employee.active, "Employee is not on active payroll");
            assert!(
                employee.last_paid_period < period,
                "Employee has already been paid for this period"
            );
            employee.last_paid_period = period;
            employee.paychecks += 1;
            self.paychecks_issued += 1;

            let bucket = self.treasury.withdraw_confidential(paycheck);
            assert_eq!(
                bucket.count_confidential_commitments(),
                1,
                "A paycheck must contain exactly one confidential output"
            );
            ComponentManager::get(employee_account).invoke("deposit", args![bucket]);

            // Deliberately no amount in the event.
            emit_event("PaycheckIssued", metadata![
                "employee" => employee_account.to_string(),
                "period" => period.to_string(),
            ]);
        }

        /// Mints more confidential budget into the treasury. The amount minted becomes public
        /// (total supply), but how it gets split into salaries does not.
        pub fn top_up(
            &mut self,
            budget: ConfidentialOutputStatement,
            budget_value_proofs: BTreeMap<PedersenCommitmentBytes, CommitmentValueProof>,
        ) {
            assert!(
                budget.output_revealed_amount.is_zero() && budget.change_revealed_amount.is_zero(),
                "Top ups must be confidential commitments, not revealed funds"
            );
            let bucket = ResourceManager::get(self.treasury.resource_address())
                .mint_confidential(budget, budget_value_proofs);
            self.treasury.deposit(bucket);
            emit_event("BudgetToppedUp", metadata!["period" => self.period.to_string()]);
        }

        pub fn period(&self) -> u64 {
            self.period
        }

        pub fn is_paid_this_period(&self, employee_account: ComponentAddress) -> bool {
            self.staff
                .get(&employee_account)
                .is_some_and(|e| self.period > 0 && e.last_paid_period == self.period)
        }

        pub fn staff_count(&self) -> u64 {
            self.staff.values().filter(|e| e.active).count() as u64
        }

        pub fn paychecks_issued(&self) -> u64 {
            self.paychecks_issued
        }

        pub fn payroll_resource(&self) -> ResourceAddress {
            self.treasury.resource_address()
        }

        /// Total budget ever minted. This is public by design (supply tracking).
        pub fn public_budget(&self) -> Option<Amount> {
            ResourceManager::get(self.treasury.resource_address()).total_supply_opt()
        }

        pub fn treasury_commitment_count(&self) -> u64 {
            u64::from(self.treasury.commitment_count())
        }
    }
}
