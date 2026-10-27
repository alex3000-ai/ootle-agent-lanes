// The buyer deposits tTARI into the ledger and opens a lane to the provider. Vouchers for the lane
// are signed by `voucher_key`, a hot key held outside the wallet. The rules hash must match
// LanesGovernance::rules (all zeros until the admin calls set_rules).
fn fee_main() {
    let buyer = var!["buyer"];
    buyer.pay_fee(amount!(20000));
}

fn main() {
    let buyer = var!["buyer"];
    let ledger = var!["ledger"];
    let funds = buyer.withdraw(TARI, amount!(1000000));
    ledger.deposit(funds);
    let lane_id = ledger.open_lane(var!["provider_pk"], amount!(500000), hex_bytes!("0000000000000000000000000000000000000000000000000000000000000000"), var!["voucher_key"]);
}
