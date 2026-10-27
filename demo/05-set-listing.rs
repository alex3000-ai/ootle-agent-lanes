// The provider points its registry listing at its Ootlebook service page, which is how Ootlebook
// verifies that the service's provider key belongs to whoever listed it.
fn fee_main() {
    let provider = var!["provider"];
    provider.pay_fee(amount!(20000));
}

fn main() {
    let registry = var!["registry"];
    registry.set_listing("https://ootlebook.alex-servin.workers.dev/services/lane-demo-payment-flow-smoke-test");
}
