# Autonomi Rewards Service

This service distributes ANT token rewards to random nodes on the Autonomi network.

## Installation

1. Ensure you have [Rust](https://www.rust-lang.org/) installed on your system. If not, install it
   using [rustup](https://rustup.rs/).
2. Clone this repository.

   ```bash
   git clone git@github.com:maidsafe/ant-rewards-service.git
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
            
        --local
            Connect to a local Autonomi network for testing.
            
        --log-file
            Log to a file in `./logs/app.log` (must have set a log level using `RUST_LOG`).    
```

The application also accepts the following optional environment variables:

- `ANT_PEERS`: Set the bootstrap peer(s).
- `PRIVATE_KEY`: Start the service with a pre-determined wallet private key. Note that even when you specify a private
  key, all funds when closing the application will still be sent to the return address.

If you do not pass a private key, the service will create an ephemeral wallet and print the wallet address.

### Example Commands

1. **Connect to Local Network**:
   ```bash
   ./ant_rewards_service --local
   ```

2. **Connect to Mainnet and set all optional parameters**:
   ```bash
   ANT_PEERS=/ip4/209.97.181.193/udp/57402/quic-v1/p2p/12D3KooWHygG9a7inESky2KpvHQmbX5o2UC8D29B5njdshAcv1p6
   ./ant_rewards_service \
       --return-address 0x1234567890abcdef1234567890abcdef12345678 \
       --reward-interval_secs 60 \
       --reward-amount 100000000000 \
       --reward-peers 100 \
       --payout-interval_secs 21600 \
       --log-file service-1.log 
   ```

### Example Output

```
Wallet address: 0xC62A48054abaC7B40ebfb9dac503dF14cE691186 // Will be random
Return wallet address: 0x1234567890abcdef1234567890abcdef12345678
```

### Funding

The service needs to be funded with Arbitrum One ETH and ANT tokens to do payouts. You can send the tokens to the wallet
address in the output after starting the service.

### Shutting Down

The service uses an ephemeral wallet, so when the service terminates, the wallet is lost. This is where the
`--return-address` param comes in handy. When you terminate the service, it will try to send all its ETH and ANT tokens
to the return address. To safely exit the service, simply press `CTRL-C` ONCE! And wait for it to exit.

### Rewards Requirements

Only nodes operating on a package version higher or the same as the specified minimum package version are eligible for
rewards. The service grabs the minimum
required package version from the file
here: https://github.com/maidsafe/gists/blob/main/rewards-service-min-package-version.json every hour. When editing the
minimum package version, you must submit it to the `main` branch and the package version must be higher than the
previous version.

### Debugging

You can also run the application with additional logging:

   ```bash
   RUST_LOG=ant_rewards_service=all ./ant_rewards_service
   ```

If `--log-file` is passed, the log file will be stored in `logs/`.

### Current Deployment

There are currently four emission services deployed on a DigitalOcean droplet.

To connect to the droplet, you can run:
> Might have to add your SSH key to the droplet on DigitalOcean first.

```
ssh root@178.128.41.193
```

The services are all started on their own tmux window:
> Using eight windows so that we can start new services without having to kill the old services during their payout
> procedure. So the services can run either on the first four tmux windows or the last four.

```
tmux a -t emission-service-1
tmux a -t emission-service-2
tmux a -t emission-service-3
tmux a -t emission-service-4

tmux a -t emission-service-5
tmux a -t emission-service-6
tmux a -t emission-service-7
tmux a -t emission-service-8
```

This is the command used to start a service:
> Note the `<SERVICE NUMBER>`.

```
ANT_PEERS=/ip4/209.97.181.193/udp/57402/quic-v1/p2p/12D3KooWHygG9a7inESky2KpvHQmbX5o2UC8D29B5njdshAcv1p6 
RUST_LOG=ant_rewards_service=trace,autonomi=debug 
./target/release/ant_rewards_service 
    --return-address 0xdA4f3aF146f86850DE8e0D6FaE6EEe051Ad0AA44 
    --reward-interval-secs 120 --reward-amount 56624628542436808 
    --payout-interval-secs 43200 
    --log-file 
    --log-file-name service-<SERVICE NUMBER>.log
```




