use autonomi::RewardsAddress;
use xor_name::{rand::{self, Rng}, XorName};

/// Generate a random XorName.
pub fn random_address() -> XorName {
    let mut rng = rand::thread_rng();
    XorName::random(&mut rng)
}

/// Generate a random u64.
pub fn random_u64() -> u64 {
    let mut rng = rand::thread_rng();
    rng.gen()
}

#[allow(dead_code)]
/// Generate a random RewardsAddress.
pub fn random_rewards_address() -> RewardsAddress {
    let array: [u8; 20] = rand::random();
    RewardsAddress::from(array)
}

/// Convert a usize to a [u8; 32].
pub fn usize_to_u8_array(value: usize) -> [u8; 32] {
    let mut array = [0u8; 32];
    let bytes = value.to_le_bytes();
    array[..bytes.len()].copy_from_slice(&bytes);
    array
}
