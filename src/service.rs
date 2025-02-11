use crate::opt::Config;
use crate::utils::random_address;
use autonomi::client::quote::DataTypes;
use autonomi::{Amount, Client, QuoteHash, RewardsAddress, Wallet};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{self, Duration};
use xor_name::XorName;

const BASE_GAS_FEE: u64 = 100_000_000_000_000;

/// Represents a single rewards distribution round.
pub type RewardDistribution = HashMap<RewardsAddress, Amount>;
pub type RewardDistributionRounds = Arc<Mutex<VecDeque<RewardDistribution>>>;

/// Run the service.
pub async fn run(config: Config, wallet: Wallet) -> eyre::Result<()> {
    let client = match config.local {
        true => Client::init_local().await?,
        false => Client::init().await?,
    };

    let rewards_distribution_rounds: RewardDistributionRounds = Default::default();

    let mut reward_interval =
        time::interval(Duration::from_secs(config.reward_interval_secs as u64));
    let mut payout_interval =
        time::interval(Duration::from_secs(config.payout_interval_secs as u64));

    // Skip the immediate execution.
    payout_interval.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Shutting down.");
                tracing::info!("Sending all funds to return address..");
                let _ = send_all_funds_to_return_address(wallet.clone(), config.return_address).await;
                break;
            }
            _ = reward_interval.tick() => {
                tracing::info!("Starting reward distribution round.");

                let client_clone = client.clone();
                let config_clone = config.clone();
                let rewards_distribution_rounds_clone = rewards_distribution_rounds.clone();

                tokio::spawn(async move {
                   let _ = start_reward_distribution_round(client_clone, config_clone, rewards_distribution_rounds_clone).await
                        .inspect_err(|err| tracing::error!("Error during reward distribution: {err:?}"));
                });
            }
            _ = payout_interval.tick() => {
                tracing::info!("Paying out rewards..");

                let wallet_clone = wallet.clone();
                let rewards_distribution_rounds_clone = rewards_distribution_rounds.clone();

                tokio::spawn(async move {
                   let _ = payout_rewards(wallet_clone, rewards_distribution_rounds_clone).await
                        .inspect_err(|err| tracing::error!("Error paying out rewards: {err:?}"));
                    tracing::info!("Rewards paid out.");
                });
            }
        }
    }

    Ok(())
}

/// Send all funds to the return address.
pub async fn send_all_funds_to_return_address(
    wallet: Wallet,
    return_address: RewardsAddress,
) -> eyre::Result<()> {
    if wallet.address() == return_address {
        return Ok(());
    }

    // Return tokens.
    let token_balance = wallet.balance_of_tokens().await?;

    if token_balance > Amount::ZERO {
        let _ = wallet
            .transfer_tokens(return_address, token_balance)
            .await
            .inspect_err(|err| tracing::error!("Error transferring ANT tokens: {err:?}"));
    }

    // Return gas.
    let mut gas_balance = wallet.balance_of_gas_tokens().await?;

    // Leave a margin to pay for the transaction gas.
    gas_balance = gas_balance.saturating_sub(Amount::from(BASE_GAS_FEE));

    if gas_balance > Amount::from(BASE_GAS_FEE) {
        let _ = wallet
            .transfer_gas_tokens(return_address, gas_balance)
            .await
            .inspect_err(|err| tracing::error!("Error transferring gas tokens: {err:?}"));
    }

    Ok(())
}

/// Pays out the rewards in the rewards map and then resets all rewards again.
pub async fn payout_rewards(
    wallet: Wallet,
    rewards_distribution_rounds: RewardDistributionRounds,
) -> eyre::Result<()> {
    let mut rewards_distribution_rounds_lock = rewards_distribution_rounds.lock().await;

    let reward_rounds_to_payout: Vec<_> = rewards_distribution_rounds_lock.drain(..).collect();

    drop(rewards_distribution_rounds_lock);

    let mut combined_rewards: HashMap<RewardsAddress, Amount> = HashMap::new();

    // Combine rewards for the same address.
    for (address, amount) in reward_rounds_to_payout.clone().into_iter().flatten() {
        let entry = combined_rewards.entry(address).or_insert(Amount::ZERO);
        *entry = entry.saturating_add(amount);
    }

    let total_amount: Amount = combined_rewards
        .values()
        .copied()
        .fold(Amount::ZERO, |acc, amount| acc.saturating_add(amount));

    let token_balance = wallet.balance_of_tokens().await?;

    tracing::debug!("Total amount to be paid out: {}", total_amount);

    if token_balance < total_amount {
        tracing::warn!("Not enough tokens to pay out rewards. Skipping payout.");

        eprintln!(
            "Not enough tokens to pay out rewards. Need: {}, have: {}. Please top up wallet: {}",
            total_amount,
            token_balance,
            wallet.address()
        );

        // Add the distribution rounds back in.
        rewards_distribution_rounds
            .lock()
            .await
            .extend(reward_rounds_to_payout);

        return Ok(());
    }

    // Gather all the rewards as quote payments.
    let quote_payments: Vec<_> = combined_rewards
        .into_iter()
        .map(|(address, amount)| (QuoteHash::ZERO, address, amount))
        .collect();

    // todo: use a transfer batching contract here instead of paying for quotes.
    // Pay out all the rewards.
    let _ = wallet
        .pay_for_quotes(quote_payments)
        .await
        .inspect_err(|err| tracing::error!("Error paying for quotes: {err:?}"));

    Ok(())
}

/// Starts a reward distribution round.
pub async fn start_reward_distribution_round(
    client: Client,
    config: Config,
    rewards_distribution_rounds: RewardDistributionRounds,
) -> eyre::Result<()> {
    let peer_reward_addresses =
        pick_random_network_peer_reward_addresses(&client, config.reward_peers_amount).await?;

    let mut reward_distribution = RewardDistribution::default();

    for peer_reward_address in peer_reward_addresses {
        let entry = reward_distribution.entry(peer_reward_address).or_default();
        *entry = entry.saturating_add(config.reward_amount);
    }

    rewards_distribution_rounds
        .lock()
        .await
        .push_back(reward_distribution);

    Ok(())
}

/// Pick random peers currently on the network.
pub async fn pick_random_network_peer_reward_addresses(
    client: &Client,
    amount: usize,
) -> eyre::Result<Vec<RewardsAddress>> {
    let random_addresses: Vec<(XorName, usize)> =
        (0..amount).map(|_| (random_address(), 1)).collect();

    let raw_quotes = client
        .get_raw_quotes(DataTypes::Chunk, random_addresses.into_iter())
        .await;

    let mut content_addr_quotes: Vec<_> = raw_quotes.into_iter().flatten().collect();

    let mut peer_reward_addresses = vec![];

    while peer_reward_addresses.len() < amount {
        let mut popped = false;

        for (_, quotes) in &mut content_addr_quotes {
            if let Some((_, quote)) = quotes.pop() {
                peer_reward_addresses.push(quote.rewards_address);
                popped = true;
            }
        }

        // No more quotes left.
        if !popped {
            tracing::error!("Could not get the requested amount of random peer reward addresses. Will continue with the current set of peer reward addresses.");
            break;
        }
    }

    Ok(peer_reward_addresses)
}
