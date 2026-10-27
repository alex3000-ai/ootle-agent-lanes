// Creates AgentRegistry (admin = the governance badge) and LaneLedger (paid in tTARI).
use template_2dbadfde1744d6c58df8638a71596d32c118fd96bbc7a9b4dc473c4356273838 as AgentRegistry;
use template_66b602222bc44ee0ed14cb9286a4bb65e12d15af44d6b7b3ae0e43e118dece00 as LaneLedger;

fn fee_main() {
    let account = var!["account"];
    account.pay_fee(amount!(20000));
}

fn main() {
    let governance = var!["governance"];
    let badge = var!["badge"];
    let registry = AgentRegistry::new(badge);
    let ledger = LaneLedger::new(governance, registry, TARI);
}
