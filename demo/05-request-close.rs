// The buyer starts closing a lane. The provider can still claim during the close window (6 epochs on
// Esmeralda); after that anyone can finalize it (06-finalize-close.rs).
fn fee_main() {
    let buyer = var!["buyer"];
    buyer.pay_fee(var!["max_fee"]);
}

fn main() {
    let ledger = var!["ledger"];
    ledger.request_close(var!["lane_id"]);
}
