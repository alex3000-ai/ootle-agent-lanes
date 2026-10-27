// The provider registers itself (signed by the provider's key, fee from its own account).
fn fee_main() {
    let provider = var!["provider"];
    provider.pay_fee(amount!(20000));
}

fn main() {
    let registry = var!["registry"];
    registry.register_provider("demo: Ootle Agent Lanes smoke test", 0u8);
}
