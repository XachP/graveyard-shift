//! Client for running Graveyard Shift on the live Ootle testnet (Esmeralda) through a local
//! `tari_ootle_walletd`.
//!
//! The wallet daemon holds the employer key and pays fees; this tool plays the parts a payroll
//! app would: it builds confidential budget and paycheck proofs, keeps the employer's treasury
//! bookkeeping, and holds the auditor and demo employee keys. It prints unsigned transactions as
//! JSON for `transactions.submit`, and reads pay stubs and auditor totals back from substate
//! JSON fetched from the network.
//!
//! ```bash
//! GS_STATE=/path/outside/repo/state.json cargo run --example live_esmeralda -- init
//! cargo run --example live_esmeralda -- new   <template_hex> <employer_account> <max_epoch> <max_fee>
//! cargo run --example live_esmeralda -- shift <component>    <employer_account> <max_epoch> <max_fee>
//! cargo run --example live_esmeralda -- commit
//! cargo run --example live_esmeralda -- read  <vault_substates.json>
//! ```
//!
//! The state file holds test-only secrets (auditor key, demo employee keys, treasury masks).
//! Keep it outside the repo.

#[path = "../tests/common/mod.rs"]
mod common;

use std::{fs, str::FromStr};

use common::*;
use serde_json::{Value, json};
use tari_crypto::tari_utilities::ByteArray;
use tari_template_test_tooling::{
    byte_type::ToByteType,
    crypto::{PublicKey as _, RistrettoPublicKey, RistrettoSecretKey, SecretKey as _},
    engine_types::{component::derive_component_address_from_public_key, substate::SubstateId},
    template_lib_types::{
        Amount,
        ComponentAddress,
        TemplateAddress,
        crypto::RistrettoPublicKeyBytes,
    },
    transaction::{Epoch, Network, Transaction, args},
    byte_type::FromByteType,
    engine_types::crypto::OutputBody,
    support::GenerateValueLookup,
    template_lib_types::crypto::PedersenCommitmentBytes,
    wallet_crypto::StealthCryptoApi,
    wallet_crypto::{MaskAndValue, confidential},
};

const BUDGET_LIVE: u64 = 10_000;
const CREW: [(&str, &str, u64, &str); 2] = [
    ("night nurse", "Night nurse", 3_000, "October night shift: 12 nights, ward 13"),
    ("gravedigger", "Gravedigger", 2_500, "October night shift: 9 plots, lantern allowance"),
];

fn state_path() -> String {
    std::env::var("GS_STATE").unwrap_or_else(|_| "/workspace/crypto-earn/wallet/gs_live/state.json".into())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn sk(s: &str) -> RistrettoSecretKey {
    RistrettoSecretKey::from_canonical_bytes(&unhex(s)).unwrap()
}

fn pk_of(s: &RistrettoSecretKey) -> RistrettoPublicKey {
    RistrettoPublicKey::from_secret_key(s)
}

fn pk_hex(s: &str) -> RistrettoPublicKey {
    RistrettoPublicKey::from_canonical_bytes(&unhex(s)).unwrap()
}

fn account_of(pk: &RistrettoPublicKey) -> ComponentAddress {
    let b: RistrettoPublicKeyBytes = pk.to_byte_type();
    derive_component_address_from_public_key(&TemplateAddress::from_array([0; 32]), &b)
}

fn load() -> Value {
    serde_json::from_str(&fs::read_to_string(state_path()).expect("run `init` first")).unwrap()
}

fn save(v: &Value) {
    let p = state_path();
    fs::write(&p, serde_json::to_string_pretty(v).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn new_secret() -> String {
    let (s, _) = keypair();
    hex(s.as_bytes())
}

fn treasury(v: &Value, key: &str) -> Vec<(RistrettoSecretKey, u64)> {
    v[key]
        .as_array()
        .map(|a| a.iter().map(|t| (sk(t["mask"].as_str().unwrap()), t["value"].as_u64().unwrap())).collect())
        .unwrap_or_default()
}

fn treasury_json(t: &[(RistrettoSecretKey, u64)]) -> Value {
    Value::Array(t.iter().map(|(m, v)| json!({"mask": hex(m.as_bytes()), "value": v})).collect())
}

fn print_tx(tx: &tari_template_test_tooling::transaction::UnsignedTransaction) {
    println!("{}", serde_json::to_string(tx).unwrap());
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("init") => {
            let employer_pk = a.get(1).expect("init <employer_public_key_hex>");
            let mut v = fs::read_to_string(state_path())
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(json!({}));
            if v.get("auditor_secret").is_none() {
                v["auditor_secret"] = json!(new_secret());
                v["employees"] = Value::Array(CREW.iter().map(|c| json!({"role": c.0, "secret": new_secret()})).collect());
            }
            v["employer_public_key"] = json!(employer_pk);
            save(&v);
            let auditor = pk_of(&sk(v["auditor_secret"].as_str().unwrap()));
            let mut out = json!({"auditor_view_key": hex(auditor.as_bytes()), "employees": []});
            for e in v["employees"].as_array().unwrap() {
                let pk = pk_of(&sk(e["secret"].as_str().unwrap()));
                out["employees"].as_array_mut().unwrap().push(json!({
                    "role": e["role"], "public_key": hex(pk.as_bytes()), "account": account_of(&pk).to_string()
                }));
            }
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        },
        Some("new") => {
            let mut v = load();
            let template = TemplateAddress::from_array(unhex(&a[1]).try_into().unwrap());
            let fee_account = ComponentAddress::from_str(&a[2]).unwrap();
            let max_epoch = Epoch(a[3].parse().unwrap());
            let max_fee: u64 = a[4].parse().unwrap();
            let auditor = pk_of(&sk(v["auditor_secret"].as_str().unwrap()));
            let employer = pk_hex(v["employer_public_key"].as_str().unwrap());
            let (statement, mask, proofs) = budget_statement(BUDGET_LIVE, &employer, &auditor);
            let auditor_vk: RistrettoPublicKeyBytes = auditor.to_byte_type();
            let mut b = Transaction::builder(Network::Esmeralda, max_epoch).pay_fee_from_component(fee_account, Amount::from(max_fee));
            for e in v["employees"].as_array().unwrap() {
                let pk = pk_of(&sk(e["secret"].as_str().unwrap()));
                b = b.create_account(pk.to_byte_type());
            }
            let tx = b
                .call_function(template, "new", args![auditor_vk, statement, proofs])
                .build_unsigned();
            v["treasury_next"] = treasury_json(&[(mask, BUDGET_LIVE)]);
            save(&v);
            print_tx(&tx);
        },
        Some("shift") => {
            let mut v = load();
            let component = ComponentAddress::from_str(&a[1]).unwrap();
            let fee_account = ComponentAddress::from_str(&a[2]).unwrap();
            let max_epoch = Epoch(a[3].parse().unwrap());
            let max_fee: u64 = a[4].parse().unwrap();
            let hire = a.get(5).map(|s| s != "nohire").unwrap_or(true);
            let auditor = pk_of(&sk(v["auditor_secret"].as_str().unwrap()));
            let employer = pk_hex(v["employer_public_key"].as_str().unwrap());
            let mut tre = treasury(&v, "treasury");
            let mut b = Transaction::builder(Network::Esmeralda, max_epoch).pay_fee_from_component(fee_account, Amount::from(max_fee));
            let mut batch = Vec::new();
            for (i, e) in v["employees"].as_array().unwrap().iter().enumerate() {
                let pk = pk_of(&sk(e["secret"].as_str().unwrap()));
                let acct = account_of(&pk);
                let (_, label, salary, memo) = CREW[i];
                if hire {
                    b = b.call_method(component, "hire", args![acct, label.to_string()]);
                }
                let total: u64 = tre.iter().map(|t| t.1).sum();
                let change = total - salary;
                let pay_mask = RistrettoSecretKey::random(&mut rand::rng());
                let change_mask = RistrettoSecretKey::random(&mut rand::rng());
                let out = sealed_witness(salary, &pay_mask, &pk, Some(memo), &auditor);
                let chg = sealed_witness(change, &change_mask, &employer, Some("treasury"), &auditor);
                let inputs: Vec<_> = tre.iter().map(|(m, val)| MaskAndValue { value: *val, mask: m.clone() }).collect();
                let proof = confidential::create_withdraw_proof(&inputs, Amount::zero(), Some(&out), Amount::zero(), Some(&chg), Amount::zero())
                    .expect("withdraw proof");
                tre = vec![(change_mask, change)];
                batch.push((acct, proof));
            }
            // Confidential outputs a vault spends are inputs the wallet cannot infer; pass them in.
            let extra: Vec<SubstateId> = std::env::var("GS_INPUTS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| SubstateId::from_str(s).expect("substate id"))
                .collect();
            let tx = b
                .with_unversioned_inputs(extra)
                .call_method(component, "open_period", args![])
                .call_method(component, "pay_shift", args![batch])
                .build_unsigned();
            v["treasury_next"] = treasury_json(&tre);
            save(&v);
            print_tx(&tx);
        },
        Some("commit") => {
            let mut v = load();
            v["treasury"] = v["treasury_next"].take();
            save(&v);
            println!("treasury committed: {}", treasury(&v, "treasury").iter().map(|t| t.1).sum::<u64>());
        },
        Some("verify") => {
            // verify <indexer_url> <resource> <label=vault_id>...  Reads public chain state only.
            let v = load();
            let indexer = a[1].trim_end_matches('/').to_string();
            let resource = &a[2];
            let fetch = |id: &str| -> Value {
                let out = std::process::Command::new("curl")
                    .args(["-s", "-m", "30", &format!("{indexer}/substates/{id}")])
                    .output()
                    .expect("curl");
                serde_json::from_slice(&out.stdout).expect("indexer json")
            };
            let auditor = sk(v["auditor_secret"].as_str().unwrap());
            let keys: Vec<(String, RistrettoSecretKey)> = v["employees"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| (e["role"].as_str().unwrap().to_string(), sk(e["secret"].as_str().unwrap())))
                .collect();
            let lookup = GenerateValueLookup::new(0..=BUDGET_LIVE * 4);
            let mut audited_total = 0u64;
            let mut report = Vec::new();
            for pair in &a[3..] {
                let (label, vault) = pair.split_once('=').unwrap();
                let vj = fetch(vault);
                let commitments = vj["state"]["up"]["substate"]["Vault"]["resource_container"]["Confidential"]["commitments"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for c in commitments {
                    let c = c.as_str().unwrap();
                    let cj = fetch(&format!("coutput_{}_{}", resource.trim_start_matches("resource_"), c));
                    let body: OutputBody =
                        serde_json::from_value(cj["state"]["up"]["substate"]["ConfidentialOutput"]["output"].clone()).expect("output body");
                    let audited = decrypt_sum(std::slice::from_ref(&body), &auditor, &lookup).unwrap_or(0);
                    audited_total += audited;
                    let commitment: PedersenCommitmentBytes = PedersenCommitmentBytes::from(<[u8; 32]>::try_from(unhex(c)).unwrap());
                    let nonce: RistrettoPublicKey = body.public_nonce.try_from_byte_type().unwrap();
                    let mut stub = json!(null);
                    for (role, key) in &keys {
                        if let Ok(d) = StealthCryptoApi::new().decrypt_utxo_data(&body.encrypted_data, &commitment, key, &nonce, false) {
                            stub = json!({"opened_by": role, "amount": d.mask_and_value.value,
                                "memo": d.memo.as_ref().and_then(|m| m.as_memo_message().map(str::to_string))});
                        }
                    }
                    report.push(json!({"holder": label, "commitment": c, "auditor_view_key_reads": audited, "pay_stub": stub}));
                }
            }
            println!("{}", serde_json::to_string_pretty(&json!({"outputs": report, "auditor_total": audited_total})).unwrap());
        },
        Some("account-of") => println!("{}", account_of(&pk_hex(&a[1]))),
        Some("whoami") => {
            let v = load();
            for e in v["employees"].as_array().unwrap() {
                let pk = pk_of(&sk(e["secret"].as_str().unwrap()));
                println!("{} {}", e["role"].as_str().unwrap(), account_of(&pk));
            }
        },
        _ => eprintln!("usage: init <employer_pk> | new ... | shift ... | commit | whoami"),
    }
}
