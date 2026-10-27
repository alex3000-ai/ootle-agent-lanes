// The provider claims a voucher: this is the second claim from the 2026-10-26 Esmeralda run
// (cumulative 350000 microtari on lane 1, so it paid 150000). Make your own with:
//   node client/voucher.mjs sign --key-file <voucher key> <ledger> <lane id> <spent_cum>
// and paste its `manifest_arg` as the third argument to `claim`.
fn fee_main() {
    let provider = var!["provider"];
    provider.pay_fee(amount!(20000));
}

fn main() {
    let provider = var!["provider"];
    let ledger = var!["ledger"];
    let payout = ledger.claim(1u64, amount!(350000), cbor!([0, [[public_key!("d49103c702233c32746a2443f4ad81feb30514b7ac6cc6cd6f5e99065d7f6d21"), hex_bytes!("a5e25b5d136fd6c58b0b91b540e7c09b3fd65c90f87a84fdc7a3a4bc013e8500")]]]));
    provider.deposit(payout);
}
