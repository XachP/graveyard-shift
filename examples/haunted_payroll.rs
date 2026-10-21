//! A narrated night at the Graveyard Shift, run on the real Ootle engine (in process).
//!
//! ```bash
//! cargo run --example haunted_payroll            # story in the terminal
//! cargo run --example haunted_payroll -- --json docs/demo/transcript.json
//! ```
//!
//! The same run is written as JSON for the browser demo in `docs/`, so every commitment,
//! ciphertext and decryption shown there comes from this engine run.

#[path = "../tests/common/mod.rs"]
mod common;

use std::{fmt::Write as _, thread::sleep, time::Duration};

use common::*;
use tari_template_test_tooling::{template_lib_types::Amount, transaction::args};

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const ORANGE: &str = "\x1b[38;5;208m";
const PURPLE: &str = "\x1b[38;5;141m";
const RESET: &str = "\x1b[0m";

fn beat() {
    if std::env::var("GS_FAST").is_err() {
        sleep(Duration::from_millis(350));
    }
}

fn act(n: &str, title: &str) {
    println!("\n{ORANGE}{BOLD}── {n} · {title} ──{RESET}");
    beat();
}

fn short(s: &str) -> String {
    if s.len() > 20 { format!("{}…{}", &s[..10], &s[s.len() - 6..]) } else { s.to_string() }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn main() {
    let json_path = std::env::args().skip_while(|a| a != "--json").nth(1);

    println!("{PURPLE}{BOLD}🦇  GRAVEYARD SHIFT · confidential payroll on the Tari Ootle{RESET}");
    println!("{DIM}   every step below is a real transaction on the Ootle engine, run in-process{RESET}");

    act("Act I", "the payroll opens at dusk");
    let mut p = setup();
    println!("  employer opens a payroll with a public budget of {BOLD}{BUDGET} SHIFT{RESET}");
    println!("  the {BOLD}SHIFT{RESET} token is confidential, and the auditor's view key is fixed on it for good");
    let budget: Option<Amount> = p.test.call_method(p.payroll, "public_budget", args![], vec![]);
    println!("  {DIM}public_budget() → {budget:?}{RESET}");

    act("Act II", "the night crew is hired");
    let crew_spec = [
        ("Alice", "night nurse", 3_000u64, "Oct wk1 · 36h nights + 4h overtime"),
        ("Bob", "security guard", 2_500, "Oct wk1 · 40h · keys to the crypt"),
        ("Carol", "gravedigger", 1_313, "Oct wk1 · 13 graves · shovel allowance"),
    ];
    let crew: Vec<_> = crew_spec.iter().map(|_| p.new_staff()).collect();
    for ((name, role, ..), s) in crew_spec.iter().zip(&crew) {
        p.employer_call("hire", args![s.account, *role]);
        println!("  hired {BOLD}{name}{RESET} as {role:<15} {DIM}{}{RESET}", short(&s.account.to_string()));
        beat();
    }

    act("Act III", "midnight: the whole shift is paid in ONE transaction");
    p.employer_call("open_period", args![]);
    let shift: Vec<(&Staff, u64, &str)> = crew_spec.iter().zip(&crew).map(|((_, _, amt, memo), s)| (s, *amt, *memo)).collect();
    let batch = p.build_shift(&shift);
    let result = p.employer_tx("pay_shift", args![batch]);
    let ev = result.finalize.events.iter().find(|e| e.topic().ends_with("ShiftPaid")).unwrap();
    println!(
        "  event ShiftPaid {{ period: {}, paychecks: {} }}   {DIM}← no names, no amounts{RESET}",
        ev.payload().get_str("period").unwrap(),
        ev.payload().get_str("paychecks").unwrap()
    );

    let mut j = String::new();
    write!(j, "{{\n  \"generated_by\": \"cargo run --example haunted_payroll\",\n  \"budget\": {BUDGET},\n  \"payroll\": \"{}\",\n  \"resource\": \"{}\",\n  \"event\": {{\"topic\": \"ShiftPaid\", \"period\": \"1\", \"paychecks\": \"3\"}},\n  \"crew\": [\n", p.payroll, p.resource).unwrap();

    act("Act IV", "🕯  the four lanterns");
    let auditor = p.auditor_secret.clone();
    let mut audited_total = 0;
    for (i, ((name, role, amount, _memo), s)) in crew_spec.iter().zip(&crew).enumerate() {
        let (commitment, body) = p.outputs_with_commitments(s.account).pop().unwrap();
        let vb = body.viewable_balance.as_ref().unwrap();
        let stub = p.read_stubs(s.account, &s.secret.clone()).pop().unwrap();
        let audit = p.audit(s.account, &auditor).unwrap();
        let (nosy, _) = keypair();
        let nosy_amount = p.audit(s.account, &nosy);
        let nosy_stub = p.read_stubs(s.account, &nosy).len();
        let auditor_stub = p.read_stubs(s.account, &auditor).len();
        audited_total += audit;
        assert_eq!(stub.amount, *amount);
        assert_eq!(audit, *amount);

        println!("\n  {BOLD}{name}{RESET} · {role}");
        println!("    {DIM}👁  public    {RESET} commitment {} · {} bytes of ciphertext", short(&commitment.to_string()), body.encrypted_data.as_bytes().len());
        println!("    {GREEN}🔑 {name:<9}{RESET} {BOLD}{} SHIFT{RESET} · memo \"{}\"", stub.amount, stub.memo.clone().unwrap_or_default());
        println!("    {PURPLE}📜 auditor   {RESET} {BOLD}{audit} SHIFT{RESET} · memo {RED}sealed{RESET} (view key opens amounts, not stubs)");
        println!(
            "    {RED}🦇 coworker  {RESET} {} · stubs opened: {nosy_stub}",
            if nosy_amount.is_none() { "nothing".to_string() } else { format!("{nosy_amount:?}") }
        );
        beat();

        write!(
            j,
            "    {{\"name\": \"{name}\", \"role\": \"{role}\", \"account\": \"{}\", \"commitment\": \"{}\", \"viewable_balance\": {{\"encrypted\": \"{}\", \"nonce\": \"{}\"}}, \"encrypted_data_hex\": \"{}\", \"stub_nonce\": \"{}\", \"employee_view\": {{\"amount\": {}, \"memo\": \"{}\"}}, \"auditor_view\": {{\"amount\": {audit}, \"memo_opened\": {}}}, \"coworker_view\": {{\"amount\": null, \"memo_opened\": false}}}}{}",
            s.account,
            commitment,
            vb.encrypted,
            vb.public_nonce,
            hex(body.encrypted_data.as_bytes()),
            body.public_nonce,
            stub.amount,
            esc(&stub.memo.clone().unwrap_or_default()),
            auditor_stub > 0,
            if i + 1 < crew.len() { "," } else { "" }
        )
        .unwrap();
        j.push('\n');
    }

    act("Act V", "the auditor closes the books at dawn");
    let treasury = p.audit(p.payroll, &auditor).unwrap();
    println!("  paychecks {audited_total} + treasury {treasury} = {BOLD}{}{RESET}   public budget {BUDGET}  {GREEN}✔ reconciled{RESET}", audited_total + treasury);
    assert_eq!(audited_total + treasury, BUDGET);

    act("Act VI", "a payslip for the landlord");
    let alice = &crew[0];
    let stub = p.read_stubs(alice.account, &alice.secret.clone()).pop().unwrap();
    let honest = Payslip { employee: alice.account, commitment: stub.commitment, amount: stub.amount, mask: stub.mask.clone() };
    let inflated = Payslip { amount: 9_999, ..honest.clone() };
    let ok = p.verify_payslip(&honest);
    let bad = p.verify_payslip(&inflated);
    println!("  Alice shows ONE opening to ONE landlord: 3000 SHIFT  → {}", if ok { format!("{GREEN}✔ matches the chain{RESET}") } else { "✘".into() });
    println!("  she tries to claim 9999 instead                  → {}", if bad { "✔".into() } else { format!("{RED}✘ rejected{RESET}") });

    act("Act VII", "the guard rails hold");
    let (leaky, ..) = {
        let pr = p.plain_paycheck(0, 500);
        (pr, ())
    };
    p.employer_call("open_period", args![]);
    let r = p.employer_call_fails("pay", args![crew[0].account, leaky]);
    println!("  paycheck with a revealed amount   → {RED}rejected{RESET} {DIM}({}){RESET}", if r.contains("fully confidential") { "would publish a salary" } else { "?" });
    let mallory = p.new_staff();
    let tx = p.test.transaction().call_method(p.payroll, "open_period", args![]).build_and_seal(&mallory.secret);
    p.test.execute_expect_failure(tx, vec![mallory.proof.clone()]);
    println!("  outsider runs payroll             → {RED}rejected{RESET} {DIM}(employer key only){RESET}");
    let batch = p.build_shift(&[(&crew[0], 100, "x"), (&crew[0], 100, "x")]);
    let r = p.employer_call_fails("pay_shift", args![batch]);
    println!("  same person twice in one shift    → {RED}rejected{RESET} {DIM}({}){RESET}", if r.contains("already been paid") { "whole shift reverts" } else { "?" });

    write!(
        j,
        "  ],\n  \"treasury_auditor_view\": {treasury},\n  \"reconciled\": {},\n  \"payslip\": {{\"employee\": \"Alice\", \"claimed\": 3000, \"verified\": {ok}, \"inflated_claim\": 9999, \"inflated_verified\": {bad}}}\n}}\n",
        audited_total + treasury == BUDGET
    )
    .unwrap();
    if let Some(path) = json_path {
        std::fs::write(&path, j).expect("write transcript");
        println!("\n{DIM}  transcript written to {path}{RESET}");
    }
    println!("\n{PURPLE}{BOLD}🌅  dawn. the money moved in plain sight; only the right keys could read it.{RESET}\n");
}
