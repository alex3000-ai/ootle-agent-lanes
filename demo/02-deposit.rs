// The buyer deposits tTARI into the ledger. Then open a lane to a provider by its directory name with
// the client (the ledger reads the provider's card, which a manifest can't declare):
//   node client/lanes.mjs open-lane --account <buyer> <provider name> <microtari> <voucher public key> --live
fn fee_main() {
    let buyer = var!["buyer"];
    buyer.pay_fee(amount!(20000));
}

fn main() {
    let buyer = var!["buyer"];
    let ledger = var!["ledger"];
    let funds = buyer.withdraw(TARI, amount!(1000000));
    ledger.deposit(funds);
}
