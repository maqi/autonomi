use crate::opt::Config;
use crate::utils::{random_address, random_u64, usize_to_u8_array};
use crate::version_file::fetch_min_package_version;
use autonomi::client::quote::DataTypes;
use autonomi::networking::version::PackageVersion;
use autonomi::networking::{Multiaddr, NetworkAddress, PeerId, PeerInfo};
use autonomi::{Amount, Client, QuoteHash, RewardsAddress, Wallet};
use chrono::Local;
use futures::future::join_all;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
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
    pub distribution_percentages: HashMap<RewardsAddress, f64>,
}

/// Run the service.
pub async fn run(config: Config, wallet: Wallet, is_observer_mode: bool) -> eyre::Result<()> {
    let shared_client: Arc<RwLock<Client>> =
        Arc::new(RwLock::new(create_client_with_retries(&config).await?));

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
                let min_package_version_clone = *min_package_version.read().await;

                tokio::spawn(async move {
                   let _ = payout_rewards(wallet_clone, rewards_distribution_rounds_clone, is_observer_mode, &config_clone, &min_package_version_clone).await
                        .inspect_err(|err| tracing::error!("Error paying out rewards: {err:?}"));

                    tracing::info!("Rewards paid out.");

                    match create_client_with_retries(&config_clone).await {
                        Ok(client) => {
                            *shared_client_clone.write().await = client;
                            tracing::info!("Updated client.");
                        }
                        Err(err) => {
                            tracing::error!("Failed to update client after payout: {err:?}. Skipping client replacement.");
                        }
                    }
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

pub async fn create_client_with_retries(config: &Config) -> eyre::Result<Client> {
    let mut attempts = 0;

    loop {
        attempts += 1;

        match create_client(config).await {
            Ok(client) => {
                return Ok(client);
            }
            Err(err) => {
                tracing::error!("Failed to create client: {err:?}. Attempt {attempts} / 4");

                if attempts >= 4 {
                    return Err(eyre::eyre!(
                        "Failed to create client after {attempts} attempts: {err:?}"
                    ));
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

    let mut distribution_percentages = HashMap::new();

    // Calculate percentage for each address
    if total_amount > Amount::ZERO {
        for (address, amount) in combined_rewards.iter() {
            let percentage = (f64::from(amount) / f64::from(total_amount)) * 100.0;
            distribution_percentages.insert(*address, percentage);
        }
    }

    DistributionStatistics {
        distribution_percentages,
    }
}

/// Flush distribution statistics to a CSV file.
/// Creates a folder structure: distribution_stats/YYYYMMDD/YYYYMMDD_HHMMSS.csv
fn flush_distribution_stats_to_disk(
    stats: &DistributionStatistics,
    combined_rewards: &HashMap<RewardsAddress, Amount>,
    mut timestamp_nanos: u128,
    min_package_version: &PackageVersion,
    total_expected_emissions: Amount,
) -> eyre::Result<()> {
    // Build CSV content
    let mut csv_content = String::new();

    // CSV header
    csv_content.push_str("timestamp_nanos,reward_address,amount,percentage,min_package_version,total_expected_emissions\n");

    // Sort by percentage descending for better readability
    let mut sorted_stats: Vec<_> = stats.distribution_percentages.iter().collect();
    sorted_stats.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Write data rows
    for (address, percentage) in sorted_stats {
        let amount = combined_rewards.get(address).unwrap_or(&Amount::ZERO);
        timestamp_nanos += 1;
        csv_content.push_str(&format!(
            "{},{},{},{:.4},{},{}\n",
            timestamp_nanos,
            address,
            amount,
            percentage,
            min_package_version,
            total_expected_emissions
        ));
    }

    // Flush to disk
    let file_path = flush_csv_to_disk("distribution_stats", csv_content)?;

    tracing::info!(
        "Wrote distribution statistics to CSV file: {:?}",
        file_path
    );

    Ok(())
}

/// Helper function to flush CSV content to disk.
/// Creates folder structure: <report_type>/YYYYMMDD/YYYYMMDD_HHMMSS.csv
/// Also writes to <report_type>.csv in the current directory (overwriting old content)
fn flush_csv_to_disk(report_type: &str, csv_content: String) -> eyre::Result<PathBuf> {
    let now = Local::now();

    // Create date folder in format YYYYMMDD
    let date_folder = now.format("%Y%m%d").to_string();
    let base_path = PathBuf::from(report_type);
    let date_path = base_path.join(&date_folder);

    // Create directories if they don't exist
    fs::create_dir_all(&date_path)?;

    // Create filename with timestamp in format YYYYMMDD_HHMMSS
    let timestamp = now.format("%Y%m%d_%H%M%S").to_string();
    let filename = format!("{}.csv", timestamp);
    let file_path = date_path.join(filename);

    // Write content to timestamped archive file
    fs::write(&file_path, &csv_content)?;

    // Also write to current file in the current directory
    let current_file_path = PathBuf::from(format!("{}.csv", report_type));
    fs::write(&current_file_path, &csv_content)?;

    Ok(file_path)
}

/// Calculate distribution statistics and flush to disk.
/// This is a convenience function that combines both operations.
pub fn calculate_and_flush_distribution_statistics(
    combined_rewards: &HashMap<RewardsAddress, Amount>,
    config: &Config,
    min_package_version: &PackageVersion,
) -> eyre::Result<()> {
    let timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Calculate total expected emissions
    // Formula: (payout_interval_secs / reward_interval_secs) * reward_peers_amount * reward_amount
    let rounds_per_payout = config.payout_interval_secs / config.reward_interval_secs;
    let total_expected_emissions = config
        .reward_amount
        .saturating_mul(Amount::from(config.reward_peers_amount as u64))
        .saturating_mul(Amount::from(rounds_per_payout as u64));

    let stats = calculate_distribution_statistics(combined_rewards);

    if let Err(err) = flush_distribution_stats_to_disk(
        &stats,
        combined_rewards,
        timestamp_nanos,
        min_package_version,
        total_expected_emissions,
    ) {
        tracing::error!(
            "Failed to write distribution statistics to disk: {:?}",
            err
        );
    }

    Ok(())
}

/// Execute quote payments for the combined rewards.
/// Returns failed payments that should be retried.
async fn execute_quote_payments(
    wallet: &Wallet,
    combined_rewards: HashMap<RewardsAddress, Amount>,
) -> eyre::Result<Option<RewardDistribution>> {
    let total_amount: Amount = combined_rewards
        .values()
        .copied()
        .fold(Amount::ZERO, |acc, amount| acc.saturating_add(amount));
    tracing::debug!("Total amount to be paid out: {}", total_amount);

    let token_balance = wallet.balance_of_tokens().await?;
    if token_balance < total_amount {
        tracing::warn!("Not enough tokens to pay out rewards. Skipping payout.");

        eprintln!(
            "Not enough tokens to pay out rewards. Need: {}, have: {}. Please top up wallet: {}",
            total_amount,
            token_balance,
            wallet.address()
        );

        // Return None to indicate the entire payment should be retried
        return Ok(None);
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

        return Ok(Some(retry_rewards_round));
    }

    Ok(Some(HashMap::new()))
}

/// Pays out the rewards in the rewards map and then resets all rewards again.
pub async fn payout_rewards(
    wallet: Wallet,
    rewards_distribution_rounds: RewardDistributionRounds,
    is_observer_mode: bool,
    config: &Config,
    min_package_version: &PackageVersion,
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

    // Calculate distribution statistics and flush to disk
    calculate_and_flush_distribution_statistics(&combined_rewards, config, min_package_version)?;

    // Observers to carry out network scan only shall not execute the following payout code block
    if is_observer_mode {
        return Ok(());
    }

    // Execute the payment
    let payment_result = execute_quote_payments(&wallet, combined_rewards).await?;

    match payment_result {
        None => {
            // Insufficient balance - add all rounds back for retry
            rewards_distribution_rounds
                .lock()
                .await
                .extend(reward_rounds_to_payout);
        }
        Some(retry_round) if !retry_round.is_empty() => {
            // Partial failure - add failed payments back for retry
            rewards_distribution_rounds
                .lock()
                .await
                .extend(vec![retry_round]);
        }
        Some(_) => {
            // Success - nothing to retry
        }
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

/// Peer information for CSV export
#[derive(Debug, Clone)]
struct PeerCsvEntry {
    reward_address: RewardsAddress,
    peer_id: PeerId,
    peer_addrs: Vec<Multiaddr>,
    node_version: String,
    version_check_passed: bool,
    finally_selected: bool,
}

/// Check if an IP address is a local/private address
fn is_local_ip(ip: &str) -> bool {
    if let Ok(addr) = ip.parse::<std::net::Ipv4Addr>() {
        let octets = addr.octets();
        // Check for common private IP ranges
        match octets[0] {
            10 => true,                                      // 10.0.0.0/8
            172 if (16..=31).contains(&octets[1]) => true,   // 172.16.0.0/12
            192 if octets[1] == 168 => true,                 // 192.168.0.0/16
            127 => true,                                      // 127.0.0.0/8 (localhost)
            _ => false,
        }
    } else {
        false
    }
}

/// Parse IP address from a list of multiaddresses
/// Returns:
/// - The public IP address if available
/// - "Relayed" if all addresses contain p2p-circuit
/// - "Local" if only local IP addresses are present
fn parse_ip_from_multiaddrs(addrs: &[Multiaddr]) -> String {
    let mut public_ips = Vec::new();
    let mut local_ips = Vec::new();
    let mut all_relayed = true;
    let mut has_non_relayed = false;

    for addr in addrs {
        let addr_str = addr.to_string();
        
        // Check if this address is relayed
        let is_relayed = addr_str.contains("p2p-circuit");
        
        if !is_relayed {
            has_non_relayed = true;
            all_relayed = false;
            
            // Extract IP from address like "/ip4/116.202.83.229/udp/..."
            if let Some(ip) = extract_ip_from_addr(&addr_str) {
                if is_local_ip(&ip) {
                    local_ips.push(ip);
                } else {
                    public_ips.push(ip);
                }
            }
        }
    }

    // If all addresses are relayed, return "0.0.0.0"
    if all_relayed && !has_non_relayed {
        return "0.0.0.0".to_string();
    }

    // Prefer public IPs over local IPs
    if let Some(ip) = public_ips.first() {
        return ip.clone();
    }

    // If only local IPs are present, return empty string
    if let Some(_ip) = local_ips.first() {
        return "".to_string();
    }

    // Fallback
    "1.1.1.1".to_string()
}

/// Extract IP address from a multiaddr string
fn extract_ip_from_addr(addr: &str) -> Option<String> {
    // Look for /ip4/xxx.xxx.xxx.xxx/ pattern
    if let Some(start) = addr.find("/ip4/") {
        let ip_start = start + 5; // length of "/ip4/"
        let remaining = &addr[ip_start..];
        
        if let Some(end) = remaining.find('/') {
            return Some(remaining[..end].to_string());
        }
    }
    None
}

/// Write peer addresses to a separate file.
/// Creates a folder structure: peers_addrs/YYYYMMDD/YYYYMMDD_HHMMSS.csv
fn write_peer_addrs_to_file(peers_data: &[PeerCsvEntry]) -> eyre::Result<()> {
    // Build CSV content
    let mut csv_content = String::new();

    // CSV header
    csv_content.push_str("timestamp,peer_id,reward_address,maxLibP2P,addresses\n");

    let mut timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Write data rows
    for entry in peers_data {
        let max_libp2p = entry.peer_addrs.len();
        let first_addr = entry
            .peer_addrs
            .first()
            .map(|addr| addr.to_string())
            .unwrap_or_default();

        timestamp_nanos += 1;
        csv_content.push_str(&format!(
            "{},{},{},{},{}\n",
            timestamp_nanos,
            entry.peer_id,
            entry.reward_address,
            max_libp2p,
            first_addr
        ));
    }

    // Flush to disk
    flush_csv_to_disk("peers_addrs", csv_content)?;

    Ok(())
}

/// Write peers with quotes data to a CSV file.
/// Creates a folder structure: peers_data/YYYYMMDD/YYYYMMDD_HHMMSS.csv
fn write_peers_to_csv(peers_data: &[PeerCsvEntry]) -> eyre::Result<()> {
    // Build CSV content
    let mut csv_content = String::new();

    // CSV header
    csv_content.push_str("timestamp_nanos,reward_address,peer_id,node_ip,node_version,version_check_passed,finally_selected\n");

    let mut timestamp_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Write data rows
    for entry in peers_data {
        let ip = parse_ip_from_multiaddrs(&entry.peer_addrs);
        timestamp_nanos += 1;
        csv_content.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            timestamp_nanos,
            entry.reward_address,
            entry.peer_id,
            ip,
            entry.node_version,
            if entry.version_check_passed { 1 } else { 0 },
            if entry.finally_selected { 1 } else { 0 }
        ));
    }

    // Flush to disk
    flush_csv_to_disk("peers_data", csv_content)?;

    // Write peer addresses to separate file
    if let Err(err) = write_peer_addrs_to_file(peers_data) {
        tracing::error!("Failed to write peer addresses to file: {:?}", err);
    }

    Ok(())
}

/// Pick random peers currently on the network.
pub async fn pick_random_network_peer_reward_addresses(
    client: &Client,
    amount: usize,
    min_version_pack: &PackageVersion,
) -> eyre::Result<Vec<RewardsAddress>> {
    // Generate a random nonce for each close_group
    let random_network_addresses: Vec<(XorName, u64)> = (0..amount).map(|_| (random_address(), random_u64())).collect();

    // Parallelize get_closest_to_address calls
    let closest_nodes_futures = random_network_addresses
        .into_iter()
        .map(|(rna, nonce)| async move {
            match client.get_closest_to_address(rna, Some(7)).await {
                Ok(closest_nodes) => Some(((rna, nonce), closest_nodes)),
                Err(_) => None,
            }
        });

    let closest_nodes_groups: Vec<_> = join_all(closest_nodes_futures)
        .await
        .into_iter()
        .flatten()
        .collect();

    // Parallelize get_storage_proofs_from_peer calls
    let storage_proof_futures = closest_nodes_groups
        .into_iter()
        .flat_map(|((rna, nonce), closest_nodes)| {
            closest_nodes
                .into_iter()
                .map(move |peer| (rna, nonce, peer))
        })
        .map(|(rna, nonce, peer)| async move {
            let peer_id = peer.peer_id;
            let peer_addrs = peer.addrs.clone();
            
            match client
                .get_storage_proofs_from_peer(rna, peer, nonce, 5, DataTypes::Chunk, MAX_CHUNK_SIZE)
                .await
            {
                Ok((Some(quote), storage_proofs)) => {
                    // Extract chunk addresses from storage proofs
                    let chunk_addresses: Vec<NetworkAddress> = storage_proofs
                        .into_iter()
                        .filter_map(|(addr, result)| {
                            result.ok().map(|_| addr)
                        })
                        .collect();
                    
                    Some((peer_id, peer_addrs, quote.rewards_address, chunk_addresses, rna))
                }
                _ => None,
            }
        });

    let peers_with_storage_proofs: Vec<_> = join_all(storage_proof_futures)
        .await
        .into_iter()
        .flatten()
        .collect();

    let pre_filtered_amount = peers_with_storage_proofs.len();

    // Group storage proofs by close_group and calculate scores
    let mut close_group_chunks: HashMap<XorName, Vec<(PeerId, Vec<NetworkAddress>)>> = HashMap::new();
    for (peer_id, _, _, chunk_addresses, rna) in &peers_with_storage_proofs {
        close_group_chunks
            .entry(*rna)
            .or_default()
            .push((*peer_id, chunk_addresses.clone()));
    }
    
    // Calculate chunk scores for each close_group
    let mut peer_scores: HashMap<PeerId, i64> = HashMap::new();
    
    for (_rna, peers_chunks) in close_group_chunks {
        // Count chunk appearances in this close_group
        let mut chunk_counts: HashMap<NetworkAddress, usize> = HashMap::new();
        for (_, chunks) in &peers_chunks {
            for chunk in chunks {
                *chunk_counts.entry(chunk.clone()).or_insert(0) += 1;
            }
        }
        
        // Create expectation_list with scores: score = (appear_times - 1) * 10
        let chunk_scores: HashMap<NetworkAddress, i64> = chunk_counts
            .into_iter()
            .map(|(chunk, count)| (chunk, (count.saturating_sub(1) * 10) as i64))
            .collect();
        
        // Assign scores to each peer based on their reported chunks
        for (peer_id, chunks) in peers_chunks {
            let peer_score: i64 = chunks
                .iter()
                .filter_map(|chunk| chunk_scores.get(chunk))
                .sum();
            *peer_scores.entry(peer_id).or_insert(0) += peer_score;
        }
    }

    // Check version for all peers and collect results (parallelized).
    let version_checks = join_all(peers_with_storage_proofs.into_iter().map(
        |(peer_id, peer_addrs, rewards_address, _chunk_addresses, _rna)| {
            let client = client.clone();
            let min_version_pack = *min_version_pack;
            async move {
                // Timeout after 5 seconds.
                let version_result = tokio::time::timeout(
                    Duration::from_secs(5),
                    client.get_node_version(PeerInfo {
                        peer_id,
                        addrs: peer_addrs.clone(),
                    }),
                )
                .await;

                let (version_str, version_check_passed) = match version_result {
                    Ok(Ok(version)) => {
                        let passed = version.is_minimum(&min_version_pack);
                        (version.to_string(), passed)
                    }
                    // Peer communication errors
                    Ok(Err(_err)) => ("".to_string(), false),
                    // Timeout of the 5s
                    Err(_) => ("".to_string(), false),
                };

                (peer_id, peer_addrs, rewards_address, version_str, version_check_passed)
            }
        },
    ))
    .await;

    // Separate eligible nodes and prepare CSV entries
    let mut eligible_nodes = Vec::new();
    let mut all_peer_entries: HashMap<PeerId, PeerCsvEntry> = HashMap::new();

    for (peer_id, peer_addrs, rewards_address, version_str, version_check_passed) in version_checks {
        // Create CSV entry (initially not selected)
        let entry = PeerCsvEntry {
            reward_address: rewards_address,
            peer_id,
            peer_addrs,
            node_version: version_str,
            version_check_passed,
            finally_selected: false,
        };
        all_peer_entries.insert(peer_id, entry);

        if version_check_passed {
            eligible_nodes.push((peer_id, rewards_address));
        }
    }

    let post_filtered_amount = eligible_nodes.len();

    tracing::info!(
        "Eligible nodes amount: {}. Filtered out on version: {}.",
        post_filtered_amount,
        pre_filtered_amount - post_filtered_amount
    );

    // Select final reward addresses based on scores
    let reward_addresses: Vec<RewardsAddress> = if eligible_nodes.len() >= amount {
        use xor_name::rand::{seq::SliceRandom, thread_rng};
        
        // Sort nodes by score (descending)
        let mut scored_nodes: Vec<(PeerId, RewardsAddress, i64)> = eligible_nodes
            .into_iter()
            .map(|(peer_id, rewards_address)| {
                let score = peer_scores.get(&peer_id).copied().unwrap_or(0);
                (peer_id, rewards_address, score)
            })
            .collect();
        
        scored_nodes.sort_by(|a, b| b.2.cmp(&a.2)); // Sort by score descending
        
        let mut selected_nodes = Vec::new();
        let mut remaining = amount;
        let mut idx = 0;
        
        while remaining > 0 && idx < scored_nodes.len() {
            let current_score = scored_nodes[idx].2;
            
            // Find all nodes with the same score
            let same_score_end = scored_nodes[idx..]
                .iter()
                .position(|(_, _, score)| *score != current_score)
                .map(|pos| idx + pos)
                .unwrap_or(scored_nodes.len());
            
            let same_score_nodes = &mut scored_nodes[idx..same_score_end];
            
            if same_score_nodes.len() <= remaining {
                // Take all nodes with this score
                for (peer_id, rewards_address, _) in same_score_nodes.iter() {
                    selected_nodes.push((*peer_id, *rewards_address));
                }
                remaining -= same_score_nodes.len();
                idx = same_score_end;
            } else {
                // Randomly select from tied nodes to fill remaining spots
                let mut rng = thread_rng();
                same_score_nodes.shuffle(&mut rng);
                for (peer_id, rewards_address, _) in same_score_nodes.iter().take(remaining) {
                    selected_nodes.push((*peer_id, *rewards_address));
                }
                remaining = 0;
            }
        }
        
        selected_nodes
            .into_iter()
            .map(|(peer_id, rewards_address)| {
                // Mark as selected
                if let Some(entry) = all_peer_entries.get_mut(&peer_id) {
                    entry.finally_selected = true;
                }
                rewards_address
            })
            .collect()
    } else {
        // If we don't have enough nodes, use all available
        tracing::error!("Could not get the requested amount of random nodes. Will continue with the set that we got of length: {}.", eligible_nodes.len());

        eligible_nodes
            .into_iter()
            .map(|(peer_id, rewards_address)| {
                // Mark as selected
                if let Some(entry) = all_peer_entries.get_mut(&peer_id) {
                    entry.finally_selected = true;
                }
                rewards_address
            })
            .collect()
    };

    // Write peers data to CSV file
    let csv_entries: Vec<PeerCsvEntry> = all_peer_entries.into_values().collect();
    if let Err(err) = write_peers_to_csv(&csv_entries) {
        tracing::error!("Failed to write peers data to CSV: {:?}", err);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_local_ip() {
        // Private IP ranges
        assert!(is_local_ip("192.168.1.1"));
        assert!(is_local_ip("192.168.0.100"));
        assert!(is_local_ip("10.0.0.1"));
        assert!(is_local_ip("10.255.255.254"));
        assert!(is_local_ip("172.16.0.1"));
        assert!(is_local_ip("172.31.255.254"));
        assert!(is_local_ip("127.0.0.1"));

        // Public IPs
        assert!(!is_local_ip("8.8.8.8"));
        assert!(!is_local_ip("116.202.83.229"));
        assert!(!is_local_ip("45.139.197.105"));
        assert!(!is_local_ip("78.46.46.54"));
        assert!(!is_local_ip("172.15.0.1")); // Just outside 172.16-31 range
        assert!(!is_local_ip("172.32.0.1")); // Just outside 172.16-31 range
    }

    #[test]
    fn test_extract_ip_from_addr() {
        assert_eq!(
            extract_ip_from_addr("/ip4/116.202.83.229/udp/36821/quic-v1"),
            Some("116.202.83.229".to_string())
        );
        assert_eq!(
            extract_ip_from_addr("/ip4/192.168.2.1/udp/16132/quic-v1/p2p/12D3KooW..."),
            Some("192.168.2.1".to_string())
        );
        assert_eq!(
            extract_ip_from_addr("/ip4/78.46.46.54/udp/16132/quic-v1/p2p/12D3KooWM7QMZeXUrU5i1JxM8JygWwMsBT8kguogzop8a8Ri6Sj6"),
            Some("78.46.46.54".to_string())
        );
    }

    #[test]
    fn test_parse_ip_normal_case() {
        // Normal case: multiple addresses with same public IP
        let addrs: Vec<Multiaddr> = vec![
            "/ip4/116.202.83.229/udp/36821/quic-v1/p2p/12D3KooWRLVqhk3T1wkZBFitppp7f5TqYJ4YtgQiZS8tEJBnvDyG"
                .parse()
                .unwrap(),
            "/ip4/116.202.83.229/udp/41019/quic-v1/p2p/12D3KooWRLVqhk3T1wkZBFitppp7f5TqYJ4YtgQiZS8tEJBnvDyG"
                .parse()
                .unwrap(),
        ];
        assert_eq!(parse_ip_from_multiaddrs(&addrs), "116.202.83.229");
    }

    #[test]
    fn test_parse_ip_all_relayed() {
        // All addresses contain p2p-circuit
        let addrs: Vec<Multiaddr> = vec![
            "/ip4/45.139.197.105/udp/39344/quic-v1/p2p/12D3KooWBRowi9xBbqzm8ABYGxFKU1ggVCshoGo43XRt5BocgPv1/p2p-circuit/p2p/12D3KooWKjAsC2qVmGDXZvXvevTAXxXyE1ijZ5ZCdGmaWyqFv3hJ"
                .parse()
                .unwrap(),
            "/ip4/213.91.236.32/udp/57986/quic-v1/p2p/12D3KooWFVxFyPxy8nadeJvHGhc1bAsyU6hMGTTXNjhxop99NH1s/p2p-circuit/p2p/12D3KooWKjAsC2qVmGDXZvXvevTAXxXyE1ijZ5ZCdGmaWyqFv3hJ"
                .parse()
                .unwrap(),
        ];
        assert_eq!(parse_ip_from_multiaddrs(&addrs), "Relayed");
    }

    #[test]
    fn test_parse_ip_mixed_relayed() {
        // One relayed, one not relayed - should use the non-relayed IP
        let addrs: Vec<Multiaddr> = vec![
            "/ip4/45.139.197.105/udp/39344/quic-v1/p2p/12D3KooWBRowi9xBbqzm8ABYGxFKU1ggVCshoGo43XRt5BocgPv1/p2p-circuit/p2p/12D3KooWKjAsC2qVmGDXZvXvevTAXxXyE1ijZ5ZCdGmaWyqFv3hJ"
                .parse()
                .unwrap(),
            "/ip4/78.46.46.54/udp/16132/quic-v1/p2p/12D3KooWM7QMZeXUrU5i1JxM8JygWwMsBT8kguogzop8a8Ri6Sj6"
                .parse()
                .unwrap(),
        ];
        assert_eq!(parse_ip_from_multiaddrs(&addrs), "78.46.46.54");
    }

    #[test]
    fn test_parse_ip_with_local_and_public() {
        // Mix of local and public IPs - should prefer public
        let addrs: Vec<Multiaddr> = vec![
            "/ip4/78.46.46.54/udp/16132/quic-v1/p2p/12D3KooWM7QMZeXUrU5i1JxM8JygWwMsBT8kguogzop8a8Ri6Sj6"
                .parse()
                .unwrap(),
            "/ip4/192.168.2.1/udp/16132/quic-v1/p2p/12D3KooWM7QMZeXUrU5i1JxM8JygWwMsBT8kguogzop8a8Ri6Sj6"
                .parse()
                .unwrap(),
        ];
        assert_eq!(parse_ip_from_multiaddrs(&addrs), "78.46.46.54");
    }

    #[test]
    fn test_parse_ip_only_local() {
        // Only local IP addresses
        let addrs: Vec<Multiaddr> = vec![
            "/ip4/192.168.2.1/udp/43761/quic-v1/p2p/12D3KooWEuzcwb4YVYo4uoWBvX8aaCYwGK4qfCpM1UhE4LKYKmFQ"
                .parse()
                .unwrap(),
        ];
        assert_eq!(parse_ip_from_multiaddrs(&addrs), "Local");
    }

    #[test]
    fn test_parse_ip_multiple_local() {
        // Multiple local IP addresses
        let addrs: Vec<Multiaddr> = vec![
            "/ip4/192.168.1.1/udp/43761/quic-v1/p2p/12D3KooWEuzcwb4YVYo4uoWBvX8aaCYwGK4qfCpM1UhE4LKYKmFQ"
                .parse()
                .unwrap(),
            "/ip4/10.0.0.5/udp/43761/quic-v1/p2p/12D3KooWEuzcwb4YVYo4uoWBvX8aaCYwGK4qfCpM1UhE4LKYKmFQ"
                .parse()
                .unwrap(),
        ];
        assert_eq!(parse_ip_from_multiaddrs(&addrs), "Local");
    }
}
