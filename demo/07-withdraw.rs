// The buyer takes unallocated funds back out of the ledger into its account (amount in microtari).
fn fee_main() {
    let buyer = var!["buyer"];
    buyer.pay_fee(amount!(20000));
}

fn main() {
    let buyer = var!["buyer"];
    let ledger = var!["ledger"];
    let funds = ledger.withdraw(var!["amount"]);
    buyer.deposit(funds);
}
