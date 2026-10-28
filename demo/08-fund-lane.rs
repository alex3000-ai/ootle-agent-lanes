// The buyer adds escrow to one of its open lanes from its ledger balance (amount in microtari). Not
// allowed once the lane is closing.
fn fee_main() {
    let buyer = var!["buyer"];
    buyer.pay_fee(var!["max_fee"]);
}

fn main() {
    let ledger = var!["ledger"];
    ledger.fund_lane(var!["lane_id"], var!["amount"]);
}
