use autonomi::RewardsAddress;
use xor_name::{rand, XorName};

/// Generate a random XorName.
pub fn random_address() -> XorName {
    let mut rng = rand::thread_rng();
    XorName::random(&mut rng)
}

#[allow(dead_code)]
/// Generate a random RewardsAddress.
pub fn random_rewards_address() -> RewardsAddress {
    let array: [u8; 20] = rand::random();
    RewardsAddress::from(array)
}
