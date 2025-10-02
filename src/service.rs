use crate::opt::Config;
use crate::utils::{random_address, usize_to_u8_array};
use crate::version_file::fetch_min_package_version;
use autonomi::client::quote::DataTypes;
use autonomi::networking::version::PackageVersion;
use autonomi::networking::PeerInfo;
use autonomi::{Amount, Client, QuoteHash, RewardsAddress, Wallet};
use chrono::Local;
use futures::future::join_all;
use libp2p::{Multiaddr, PeerId};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{self, Duration};
use xor_name::XorName;

const BASE_GAS_FEE: u64 = 100_000_000_000_000;
const UPDATE_MIN_VERSION_INTERVAL_SECS: u64 = 60 * 60; // Hourly
const MAX_CHUNK_SIZE: usize = 4_194_304;

/// Represents a single rewards distribution round.
pub type RewardDistribution = HashMap<RewardsAddress, Amount>;
pub type RewardDistributionRounds = Arc<Mutex<VecDeque<RewardDistribution>>>;

/// Distribution statistics for a payout round.
#[derive(Debug, Clone)]
pub struct DistributionStatistics {
    pub total_amount: Amount,
    pub total_recipients: usize,
    pub distribution_percentages: HashMap<RewardsAddress, f64>,
}

/// Run the service.
pub async fn run(config: Config, wallet: Wallet) -> eyre::Result<()> {
    let shared_client: Arc<RwLock<Client>> =
        Arc::new(RwLock::new(create_client_with_retries(&config).await));

    let rewards_distribution_rounds: RewardDistributionRounds = Default::default();

    let min_package_version = Arc::new(RwLock::new(PackageVersion {
        year: 2025,
        month: 7,
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

/// Calculate distribution statistics from accumulated rewards.
pub fn calculate_distribution_statistics(
    combined_rewards: &HashMap<RewardsAddress, Amount>,
) -> DistributionStatistics {
    let total_amount: Amount = combined_rewards
        .values()
        .copied()
        .fold(Amount::ZERO, |acc, amount| acc.saturating_add(amount));

    let total_recipients = combined_rewards.len();
    let mut distribution_percentages = HashMap::new();

    // Calculate percentage for each address
    if total_amount > Amount::ZERO {
        for (address, amount) in combined_rewards.iter() {
            let percentage = (f64::from(amount) / f64::from(total_amount)) * 100.0;
            distribution_percentages.insert(*address, percentage);
        }
    }

    DistributionStatistics {
        total_amount,
        total_recipients,
        distribution_percentages,
    }
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

    // Calculate distribution statistics
    let stats = calculate_distribution_statistics(&combined_rewards);

    // Log distribution statistics
    tracing::info!("=== Distribution Statistics ===");
    tracing::info!("Total amount to distribute: {}", stats.total_amount);
    tracing::info!("Total recipients: {}", stats.total_recipients);
    tracing::info!("Distribution breakdown:");

    // Sort by percentage descending for better readability
    let mut sorted_stats: Vec<_> = stats.distribution_percentages.iter().collect();
    sorted_stats.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (address, percentage) in sorted_stats.iter() {
        let amount = combined_rewards.get(*address);
        tracing::info!("  {address} -> {amount:?} ({:.4}%)", percentage);
    }
    tracing::info!("================================");

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

/// Write peers with quotes data to a CSV file.
/// Creates a folder structure: peers_data/YYYYMMDD/timestamp.csv
fn write_peers_to_csv(
    peers_data: &Vec<(PeerId, Vec<Multiaddr>, RewardsAddress)>,
) -> eyre::Result<()> {
    let now = Local::now();

    // Create date folder in format YYYYMMDD
    let date_folder = now.format("%Y%m%d").to_string();
    let base_path = PathBuf::from("peers_data");
    let date_path = base_path.join(&date_folder);

    // Create directories if they don't exist
    fs::create_dir_all(&date_path)?;

    // Create filename with timestamp
    let timestamp = now.format("%Y%m%d_%H%M%S").to_string();
    let filename = format!("{}.csv", timestamp);
    let file_path = date_path.join(filename);

    // Create and write to the CSV file
    let mut file = fs::File::create(&file_path)?;

    // Write CSV header
    writeln!(file, "reward_address,peer_id,peer_addrs")?;

    // Write data rows
    for (peer_id, peer_addrs, reward_address) in peers_data {
        let addrs_str = peer_addrs
            .iter()
            .map(|addr| addr.to_string())
            .collect::<Vec<_>>()
            .join(";");

        writeln!(file, "{},{},\"{}\"", reward_address, peer_id, addrs_str)?;
    }

    tracing::info!(
        "Wrote {} peers to CSV file: {:?}",
        peers_data.len(),
        file_path
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

    // Parallelize get_closest_to_address calls
    let closest_nodes_futures = random_network_addresses
        .into_iter()
        .map(|rna| async move {
            match client.get_closest_to_address(rna).await {
                Ok(closest_nodes) => Some((rna, closest_nodes)),
                Err(_) => None,
            }
        });

    let closest_nodes_groups: Vec<_> = join_all(closest_nodes_futures)
        .await
        .into_iter()
        .flatten()
        .collect();

    // Parallelize get_raw_quote_from_peer calls
    let quote_futures = closest_nodes_groups
        .into_iter()
        .flat_map(|(rna, closest_nodes)| {
            closest_nodes
                .into_iter()
                .map(move |peer| (rna, peer))
        })
        .map(|(rna, peer)| async move {
            match client
                .get_raw_quote_from_peer(rna, peer, DataTypes::Chunk, MAX_CHUNK_SIZE)
                .await
            {
                Ok(Some((peer_id, peer_addresses, quote))) => {
                    Some((peer_id, peer_addresses.0, quote.rewards_address))
                }
                _ => None,
            }
        });

    let peers_with_quotes: Vec<_> = join_all(quote_futures)
        .await
        .into_iter()
        .flatten()
        .collect();

    // Write peers data to CSV file
    if let Err(err) = write_peers_to_csv(&peers_with_quotes) {
        tracing::error!("Failed to write peers data to CSV: {:?}", err);
    }

    let pre_filtered_amount = peers_with_quotes.len();

    // Filter out ineligible nodes based on their version (parallelized).
    let version_checks = join_all(peers_with_quotes.into_iter().map(
        |(peer_id, peer_addrs, rewards_address)| {
            let client = client.clone();
            let min_version_pack = *min_version_pack;
            async move {
                // Timeout after 5 seconds.
                let version = tokio::time::timeout(
                    Duration::from_secs(5),
                    client.get_node_version(PeerInfo {
                        peer_id,
                        addrs: peer_addrs,
                    }),
                )
                .await;

                if let Ok(Ok(version)) = version {
                    if version.is_minimum(&min_version_pack) {
                        return Some((peer_id, rewards_address));
                    }
                }

                None
            }
        },
    ))
    .await;

    let eligible_nodes: Vec<_> = version_checks.into_iter().flatten().collect();

    let post_filtered_amount = eligible_nodes.len();

    tracing::info!(
        "Eligible nodes amount: {}. Filtered out on version: {}.",
        post_filtered_amount,
        pre_filtered_amount - post_filtered_amount
    );

    // Early exit if we have enough nodes
    if eligible_nodes.len() >= amount {
        use xor_name::rand::{seq::SliceRandom, thread_rng};
        let mut rng = thread_rng();
        let mut eligible_nodes = eligible_nodes;
        eligible_nodes.shuffle(&mut rng);

        let reward_addresses: Vec<RewardsAddress> = eligible_nodes
            .into_iter()
            .take(amount)
            .map(|(_, rewards_address)| rewards_address)
            .collect();

        return Ok(reward_addresses);
    }

    // If we don't have enough nodes, use all available
    tracing::error!("Could not get the requested amount of random nodes. Will continue with the set that we got of length: {}.", eligible_nodes.len());

    let reward_addresses: Vec<RewardsAddress> = eligible_nodes
        .into_iter()
        .map(|(_, rewards_address)| rewards_address)
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
