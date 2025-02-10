use autonomi::{Amount, RewardsAddress};
use clap::Parser;

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub local: bool,
    pub return_address: RewardsAddress,
    pub reward_interval_secs: u32,
    pub reward_amount: Amount,
    pub reward_peers_amount: usize,
    pub payout_interval_secs: u32,
}

#[derive(Parser)]
#[command(disable_version_flag = true)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Opt {
    #[clap(long, default_value_t = false)]
    /// Connect to a local Autonomi network for testing.
    pub local: bool,
    #[clap(long)]
    /// Wallet address to return leftover funds to.
    pub return_address: String,
    #[clap(long, default_value_t = 60)]
    /// Interval in secs of when the service should start a reward distribution round.
    pub reward_interval_secs: u32,
    #[clap(long, default_value_t = Amount::from(100_000_000_000_u64))]
    /// Reward per peer in Atto.
    pub reward_amount: Amount,
    #[clap(long, default_value_t = 100)]
    /// Amount of peers being rewarded per distribution round.
    pub reward_peers: usize,
    #[clap(long, default_value_t = 21600)]
    /// Interval in secs of when the rewards will be paid out.
    pub payout_interval_secs: u32,
}

impl Opt {
    pub fn try_to_config(&self) -> eyre::Result<Config> {
        let return_address_str = self.return_address.trim_start_matches("0x");
        let return_address_bytes: [u8; 20] = hex::decode(return_address_str)?
            .try_into()
            .map_err(|_| eyre::eyre!("Invalid return address. Expected 20 bytes."))?;

        Ok(Config {
            local: self.local,
            return_address: RewardsAddress::from(&return_address_bytes),
            reward_interval_secs: self.reward_interval_secs,
            reward_amount: self.reward_amount,
            reward_peers_amount: self.reward_peers,
            payout_interval_secs: self.payout_interval_secs,
        })
    }
}
