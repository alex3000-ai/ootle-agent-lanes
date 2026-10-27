// Creates LanesGovernance and deposits the admin badge into the admin account.
use template_7f7aa6eb23f44d5cf850aef47191e0fa1b919c204d93f57a2499b4a878296fed as LanesGovernance;

fn fee_main() {
    let account = var!["account"];
    account.pay_fee(amount!(20000));
}

fn main() {
    let account = var!["account"];
    let (governance, badge) = LanesGovernance::new(0u16, 6u64);
    account.deposit(badge);
}
