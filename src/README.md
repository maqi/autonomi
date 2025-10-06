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

## Observor Usage

The application can be used to simulate the emission network scanning, but without payout.
To do that, just no to setup the `PRIVATE_KEY` env, and launch service as normal.

## Emission Usage

The application accepts various command-line arguments to configure the reward distribution functionality:

```bash
USAGE:
    ant_rewards_service [OPTIONS]

OPTIONS:
        --local
            Connect to a local Autonomi network for testing.

        --return_address <ADDRESS>
            Wallet address to return leftover funds to (in hexadecimal format).

        --reward_interval_secs <SECONDS> 
            Interval in seconds between reward distribution rounds.
            [Default: 60]

        --reward_amount <AMOUNT>
            Reward per peer in Atto.
            [Default: 100000000000]

        --reward_peers <PEER_COUNT>
            Number of peers rewarded per distribution round.
            [Default: 100]

        --payout_interval_secs <SECONDS>
            Interval in seconds for payout scheduling.
            [Default: 21600]
```

### Example Commands

1. **Connect to Local Network**:
   ```bash
   your-app-name --local
   ```

2. **Configure with All Parameters**:
   ```bash
   your-app-name \
       --return_address 0x1234567890abcdef1234567890abcdef12345678 \
       --reward_interval_secs 60 \
       --reward_amount 100000000000 \
       --reward_peers 100 \
       --payout_interval_secs 21600
   ```