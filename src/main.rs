mod logging;
mod opt;
mod service;
mod utils;

use autonomi::Wallet;
use clap::Parser;
use eyre::eyre;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let opt = opt::Opt::parse();

    // Setup logging.
    logging::setup_logging(opt.log_file);

    // Tries to get the network from env first `EVM_NETWORK`.
    let network = autonomi::Network::new(false).unwrap_or_default();

    // Create a wallet for the service.
    //
    // Get the private key from ENV or generate a random new one.
    let wallet = if let Ok(private_key_str) = std::env::var("PRIVATE_KEY") {
        Wallet::new_from_private_key(network, &private_key_str).map_err(|_| {
            eyre!("Invalid private key format. Please provide a valid 64-character hex string.")
        })?
    } else {
        Wallet::new_with_random_wallet(network)
    };

    // todo: set lower wallet max fee per gas limit

    println!("Wallet address: {}", wallet.address());

    let config = opt.try_to_config()?;

    println!("Return wallet address: {}", config.return_address);

    // Start the service.
    service::run(config, wallet).await
}
