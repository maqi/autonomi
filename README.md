# Autonomi Rewards Service

This service distributes ANT token rewards to random nodes on the Autonomi network.

## Installation

1. Ensure you have [Rust](https://www.rust-lang.org/) installed on your system. If not, install it
   using [rustup](https://rustup.rs/).
2. Clone this repository.

   ```bash
   git clone https://github.com/maidsafe/ant-rewards-service
   cd ant-rewards-service
   ```

3. Build the CLI tool:

   ```bash
   cargo build --release
   ```

4. Run the binary:

   ```bash
   ./target/release/ant_rewards_service
   ```

## Usage

The application accepts various command-line arguments to configure the reward distribution functionality:

```bash
USAGE:
    ./ant_rewards_service [OPTIONS]

OPTIONS:
        --local
            Connect to a local Autonomi network for testing.
            
        --log-file
            Log to a file in `./logs/app.log` (must have set a log level using `RUST_LOG`).

        --return-address <ADDRESS>
            Wallet address to return leftover funds to (in hexadecimal format).

        --reward-interval-secs <SECONDS> 
            Interval in seconds between reward distribution rounds.
            [Default: 60]

        --reward-amount <AMOUNT>
            Reward per peer in Atto.
            [Default: 100000000000]

        --reward-peers <PEER_COUNT>
            Number of peers rewarded per distribution round.
            [Default: 100]

        --payout-interval-secs <SECONDS>
            Interval in seconds for payout scheduling.
            [Default: 21600]
```

The application also accepts the following optional environment variables:

- `PRIVATE_KEY`: Start the service with a pre-determined wallet private key. Note that even when you specify a private
  key, all funds when closing the application will still be send to the return address.

If you do not pass a private key, the service will create an ephemeral wallet and print the wallet address.

### Example Commands

1. **Connect to Local Network**:
   ```bash
   ./ant_rewards_service --local
   ```

2. **Configure with All Parameters**:
   ```bash
   ./ant_rewards_service \
       --return_address 0x1234567890abcdef1234567890abcdef12345678 \
       --reward_interval_secs 60 \
       --reward_amount 100000000000 \
       --reward_peers 100 \
       --payout_interval_secs 21600
   ```

### Debugging

You can also run the application with additional logging:

   ```bash
   RUST_LOG=ant_rewards_service=all ./ant_rewards_service
   ```