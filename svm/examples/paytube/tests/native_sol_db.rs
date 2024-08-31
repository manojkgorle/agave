mod setup;

use {
    setup::system_account,
    solana_sdk::{signature::Keypair, signer::Signer},
};
#[test]

fn test_native_sol_db() {
    let alice = Keypair::new();
    let bob = Keypair::new();
    let will = Keypair::new();

    let alice_pubkey = alice.pubkey();
    let bob_pubkey = bob.pubkey();
    let will_pubkey = will.pubkey();

    let accounts = vec![
        (alice_pubkey, system_account(10_000_000)),
        (bob_pubkey, system_account(10_000_000)),
        (will_pubkey, system_account(10_000_000)),
    ];
}
