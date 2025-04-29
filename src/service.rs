use crate::opt::Config;
use crate::utils::{random_address, usize_to_u8_array};
use crate::version_file::fetch_min_package_version;
use autonomi::client::quote::DataTypes;
use autonomi::{Amount, Client, PackageVersion, QuoteHash, RewardsAddress, Wallet};
use futures::future::join_all;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{self, Duration};
use xor_name::XorName;

const BASE_GAS_FEE: u64 = 100_000_000_000_000;
const UPDATE_MIN_VERSION_INTERVAL_SECS: u64 = 60 * 60; // Hourly

/// Represents a single rewards distribution round.
pub type RewardDistribution = HashMap<RewardsAddress, Amount>;
pub type RewardDistributionRounds = Arc<Mutex<VecDeque<RewardDistribution>>>;

/// Run the service.
pub async fn run(config: Config, wallet: Wallet) -> eyre::Result<()> {
    let shared_client: Arc<RwLock<Client>> =
        Arc::new(RwLock::new(create_client_with_retries(&config).await));

    let rewards_distribution_rounds: RewardDistributionRounds = Default::default();

    let min_package_version = Arc::new(RwLock::new(PackageVersion {
        year: 2025,
        month: 4,
        cycle: 1,
        cycle_counter: 1,
    }));

    let mut reward_interval =
        time::interval(Duration::from_secs(config.reward_interval_secs as u64));
    let mut payout_interval =
        time::interval(Duration::from_secs(config.payout_interval_secs as u64));
    let mut update_min_version =
        time::interval(Duration::from_secs(UPDATE_MIN_VERSION_INTERVAL_SECS));

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

                let shared_client_clone = shared_client.clone();
                let config_clone = config.clone();
                let rewards_distribution_rounds_clone = rewards_distribution_rounds.clone();
                let min_package_version_clone = *min_package_version.read().await;

                tokio::spawn(async move {
                    let client = shared_client_clone.read().await.clone();

                    let _ = start_reward_distribution_round(client, config_clone, rewards_distribution_rounds_clone, &min_package_version_clone).await
                        .inspect_err(|err| tracing::error!("Error during reward distribution: {err:?}"));
                });
            }
            _ = payout_interval.tick() => {
                tracing::info!("Paying out rewards..");

                let shared_client_clone = shared_client.clone();
                let config_clone = config.clone();
                let wallet_clone = wallet.clone();
                let rewards_distribution_rounds_clone = rewards_distribution_rounds.clone();

                tokio::spawn(async move {
                   let _ = payout_rewards(wallet_clone, rewards_distribution_rounds_clone).await
                        .inspect_err(|err| tracing::error!("Error paying out rewards: {err:?}"));

                    tracing::info!("Rewards paid out.");

                    let client = create_client_with_retries(&config_clone).await;
                    *shared_client_clone.write().await = client;

                    tracing::info!("Updated client.");
                });
            }
            _ = update_min_version.tick() => {
                let min_package_version_clone = min_package_version.clone();

                tokio::spawn(async move {
                    update_min_package_version(min_package_version_clone).await;
                });
            }
        }
    }

    Ok(())
}

pub async fn create_client(config: &Config) -> eyre::Result<Client> {
    match config.local {
        true => Ok(Client::init_local().await?),
        false => Ok(Client::init().await?),
    }
}

pub async fn create_client_with_retries(config: &Config) -> Client {
    let mut attempts = 0;

    loop {
        attempts += 1;

        match create_client(config).await {
            Ok(client) => {
                break client;
            }
            Err(err) => {
                tracing::error!("Failed to create client: {err:?}. Attempt {attempts} / 4");

                // Should never happen.
                if attempts >= 4 {
                    panic!("Failed to create client after {attempts} attempts");
                }

                // Wait for a short duration before retrying
                time::sleep(Duration::from_secs(5_u64.pow(attempts - 1))).await;
            }
        }
    }
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
        .enumerate()
        .map(|(i, (address, amount))| (QuoteHash::from(usize_to_u8_array(i)), address, amount))
        .collect();

    // todo: use a transfer batching contract here instead of paying for quotes.
    // Pay out all the rewards.
    if let Err(err) = wallet.pay_for_quotes(quote_payments.clone()).await {
        tracing::error!("Error paying for quotes: {:?}", err.0);
        tracing::error!(
            "{} quote(s) were successfully paid. {} quote(s) failed",
            err.1.len(),
            quote_payments.len() - err.1.len()
        );

        let failed_quote_payments: Vec<_> = quote_payments
            .into_iter()
            .filter(|(qh, _, _)| !err.1.contains_key(qh))
            .collect();

        // Create a new rewards round that consists of the failed (per address combined) payments.
        let retry_rewards_round: RewardDistribution = failed_quote_payments
            .into_iter()
            .map(|(_, address, amount)| (address, amount))
            .collect();

        // Add the distribution rounds back in.
        rewards_distribution_rounds
            .lock()
            .await
            .extend(vec![retry_rewards_round]);
    }

    Ok(())
}

/// Starts a reward distribution round.
pub async fn start_reward_distribution_round(
    client: Client,
    config: Config,
    rewards_distribution_rounds: RewardDistributionRounds,
    min_version_pack: &PackageVersion,
) -> eyre::Result<()> {
    let start_time = std::time::Instant::now();

    let peer_reward_addresses = pick_random_network_peer_reward_addresses(
        &client,
        config.reward_peers_amount,
        min_version_pack,
    )
    .await?;

    let mut reward_distribution = RewardDistribution::default();

    for peer_reward_address in peer_reward_addresses {
        let entry = reward_distribution.entry(peer_reward_address).or_default();
        *entry = entry.saturating_add(config.reward_amount);
    }

    rewards_distribution_rounds
        .lock()
        .await
        .push_back(reward_distribution);

    tracing::info!(
        "Reward distribution round completed in {} seconds.",
        start_time.elapsed().as_secs()
    );

    Ok(())
}

/// Pick random peers currently on the network.
pub async fn pick_random_network_peer_reward_addresses(
    client: &Client,
    amount: usize,
    min_version_pack: &PackageVersion,
) -> eyre::Result<Vec<RewardsAddress>> {
    let random_network_addresses: Vec<XorName> = (0..amount).map(|_| random_address()).collect();

    // Get all closest nodes.
    let results =
        join_all(random_network_addresses.into_iter().map(|rna| async move {
            client.get_closest_to_address(rna).await.unwrap_or_default()
        }))
        .await;

    // Get the peer version of every node.
    let results_with_peer_version = join_all(results.into_iter().map(|closest_nodes| async move {
        join_all(
            closest_nodes
                .into_iter()
                .map(|(peer, addresses)| async move {
                    let version = client.get_node_version(peer, addresses.clone()).await;
                    (peer, addresses, version)
                }),
        )
        .await
    }))
    .await;

    let pre_filtered_amount = results_with_peer_version.iter().flatten().count();

    // Filter out ineligible nodes based on their version.
    let mut eligible_nodes: Vec<Vec<_>> = results_with_peer_version
        .into_iter()
        .map(|nodes_with_versions| {
            nodes_with_versions
                .iter()
                .filter_map(|(peer, addresses, version)| {
                    if let Ok(version) = version {
                        if version.is_minimum(min_version_pack) {
                            return Some((*peer, addresses.clone()));
                        }
                    }

                    None
                })
                .collect()
        })
        .collect();

    let post_filtered_amount = eligible_nodes.iter().flatten().count();

    tracing::info!(
        "Eligible nodes amount: {}. Filtered out on version: {}.",
        post_filtered_amount,
        pre_filtered_amount - post_filtered_amount
    );

    let mut picked_nodes = vec![];

    // Pick 100 nodes.
    while picked_nodes.len() < amount {
        let mut popped = false;

        for closest_nodes in &mut eligible_nodes {
            if let Some((peer, addresses)) = closest_nodes.pop() {
                picked_nodes.push((peer, addresses));
                popped = true;
            }
        }

        // No more nodes left.
        if !popped {
            tracing::error!("Could not get the requested amount of random nodes. Will continue with the set that we got of length: {}.", picked_nodes.len());
            break;
        }
    }

    let random_address = random_address();

    // Fetch the reward addresses for the picked nodes.
    // There is no query to get the rewards address yet, so as a workaround we fetch a quote.
    let reward_addresses = join_all(picked_nodes.into_iter().map(|(peer, addresses)| {
        let client = client.clone();
        async move {
            let result = client
                .get_raw_quote_from_node(random_address, DataTypes::Chunk, peer, addresses)
                .await;

            if let Ok(Some((_peer, quote))) = result {
                return Some(quote.rewards_address);
            }

            None
        }
    }))
    .await
    .into_iter()
    .flatten()
    .collect();

    Ok(reward_addresses)
}

pub async fn update_min_package_version(min_package_version: Arc<RwLock<PackageVersion>>) {
    if let Ok(fetched_min_package_version) = fetch_min_package_version().await {
        let current_min_package_version = *min_package_version.read().await;

        tracing::info!("Fetched minimum package version: {fetched_min_package_version}");
        tracing::info!("Current minimum package version: {current_min_package_version}");

        if fetched_min_package_version.is_minimum(&current_min_package_version)
            && !fetched_min_package_version.is_exact(&current_min_package_version)
        {
            *min_package_version.write().await = fetched_min_package_version;

            tracing::info!("Updated minimum package version to: {fetched_min_package_version}");
        }
    } else {
        tracing::error!("Failed to fetch minimum package version file.");
    }
}
