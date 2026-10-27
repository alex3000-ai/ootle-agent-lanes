// Creates a LaneLedger against an existing governance and registry (used to replace the first
// ledger with the voucher-key version).
use template_66b602222bc44ee0ed14cb9286a4bb65e12d15af44d6b7b3ae0e43e118dece00 as LaneLedger;

fn fee_main() {
    let account = var!["account"];
    account.pay_fee(amount!(20000));
}

fn main() {
    let governance = var!["governance"];
    let registry = var!["registry"];
    let ledger = LaneLedger::new(governance, registry, TARI);
}
