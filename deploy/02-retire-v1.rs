// Retires the v0.1 lanes deployment: pauses its governance so no new lane can open there. The v0.1
// admin badge then controls only a component nobody uses. Withdrawals from the old ledger still work.
//   scripts/submit.py deploy/02-retire-v1.rs --signer-index <admin index> --var account=<admin account> \
//     --var governance=<v0.1 governance> --var badge=<v0.1 admin badge> [--live]
fn fee_main() {
    let account = var!["account"];
    account.pay_fee(amount!(20000));
}

fn main() {
    let account = var!["account"];
    let governance = var!["governance"];
    let badge_proof = account.create_proof_for_resource(var!["badge"]);
    governance.set_paused(true);
    drop_all_proofs!();
}
