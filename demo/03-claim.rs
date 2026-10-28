// The provider claims a voucher: this is the claim from the 2026-10-28 Esmeralda run (cumulative
// 200000 microtari on lane 1). Make your own with:
//   node client/voucher.mjs sign --key-file <voucher key> <ledger> <lane id> <spent_cum>
// and paste its `manifest_arg` as the third argument to `claim`.
fn fee_main() {
    let provider = var!["provider"];
    provider.pay_fee(amount!(20000));
}

fn main() {
    let provider = var!["provider"];
    let ledger = var!["ledger"];
    let payout = ledger.claim(1u64, amount!(200000), cbor!([0, [[public_key!("dc9ed4809832210d142aeaaaf358921cecc6a435603d34f96c351efcfd62492e"), hex_bytes!("841b4329e06ef5cb304bdce6873c099bae4f05f3cd908acbf518cb94fd526c0a")]]]));
    provider.deposit(payout);
}
