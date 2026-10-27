// Creates the demo provider and buyer accounts on-chain with 2 tTARI each from the admin account.
fn fee_main() {
    let account = var!["account"];
    account.pay_fee(amount!(20000));
}

fn main() {
    let admin = var!["account"];
    let provider_pk = var!["provider_pk"];
    let buyer_pk = var!["buyer_pk"];
    let provider_funds = admin.withdraw(TARI, amount!(2000000));
    create_account!(provider_pk, bucket = provider_funds);
    let buyer_funds = admin.withdraw(TARI, amount!(2000000));
    create_account!(buyer_pk, bucket = buyer_funds);
}
