// The provider releases the lane: the unclaimed escrow returns to the buyer's ledger balance.
fn fee_main() {
    let provider = var!["provider"];
    provider.pay_fee(amount!(20000));
}

fn main() {
    let ledger = var!["ledger"];
    ledger.release_lane(var!["lane_id"]);
}
