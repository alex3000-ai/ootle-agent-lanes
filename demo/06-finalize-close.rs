// Anyone finalizes a lane once its close window has passed; the unclaimed escrow returns to the
// buyer's ledger balance.
fn fee_main() {
    let account = var!["account"];
    account.pay_fee(var!["max_fee"]);
}

fn main() {
    let ledger = var!["ledger"];
    ledger.finalize_close(var!["lane_id"]);
}
