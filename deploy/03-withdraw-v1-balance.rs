// Takes a buyer's unallocated balance back out of the v0.1 ledger (withdrawals still work after the
// pause). Used once, for the 2026-10-26 demo buyer's 0.65 tTARI.
//   scripts/submit.py deploy/03-withdraw-v1-balance.rs --signer-index <buyer index> --var buyer=<buyer account> \
//     --var ledger=<v0.1 ledger> [--live]
fn fee_main() {
    let buyer = var!["buyer"];
    buyer.pay_fee(amount!(20000));
}

fn main() {
    let buyer = var!["buyer"];
    let ledger = var!["ledger"];
    let refund = ledger.withdraw(amount!(650000));
    buyer.deposit(refund);
}
