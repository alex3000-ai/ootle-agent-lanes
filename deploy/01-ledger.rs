// Creates the admin-free LaneLedger over the Ootle Agent Directory: tTARI payments, a 6-epoch close
// window. Nothing can change these later, and the component has no owner.
//   scripts/submit.py deploy/01-ledger.rs --signer-index <i> --var account=<component> --var directory=<directory component> [--live]
use template_b20c12261abf5ca6f00ded5756f41d9ec6dab4ffba95ce840cfd62c635d04b62 as LaneLedger;

fn fee_main() {
    let account = var!["account"];
    account.pay_fee(var!["max_fee"]);
}

fn main() {
    let directory = var!["directory"];
    let ledger = LaneLedger::new(directory, TARI, 6u64);
}
