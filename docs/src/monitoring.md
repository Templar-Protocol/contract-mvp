# Monitoring and Risk Management

Templar Protocol uses available tools and established practices for monitoring protocol health and managing risks.

## Protocol Monitoring Tools

### Available Monitoring

#### Bot Infrastructure
The protocol includes operational bots for automated tasks:

- **Liquidation Bot**: Monitors positions and executes liquidations
  - Configurable intervals and concurrency
  - Market registry monitoring  
  - Oracle price feed integration
  - Automated liquidation execution

- **Accumulator Bot**: Handles interest accumulation
  - Periodic interest calculations
  - Multi-market support
  - Configurable execution parameters

#### Gas Usage Monitoring  
Gas analysis tools provide performance insights:

```bash
./script/gas-report.sh
```

This generates detailed reports on:
- Function execution costs
- Snapshot iteration limits
- Performance bottlenecks

### Manual Monitoring Procedures

#### Protocol Health Checks
Regular checks can be performed using:

1. **Market Status**: Query market configurations and states
   ```bash
   # Get market configuration
   near contract call-function as-read-only <market-address> get_configuration json-args {} network-config mainnet now
   
   # Check current market snapshot
   near contract call-function as-read-only <market-address> get_current_snapshot json-args {} network-config mainnet now
   
   # Get borrow asset metrics
   near contract call-function as-read-only <market-address> get_borrow_asset_metrics json-args {} network-config mainnet now
   
   # List all deployed markets from registry
   near contract call-function as-read-only v1.tmplr.near list_deployments json-args '{"offset": 0, "count": 100}' network-config mainnet now
   ```

   `paid_to_fees` is the decimal-string amount of repaid interest and fees
   currently available to pay supplier yield. It is a market-wide,
   first-come-first-served pool: it is not accrued yield, per-user reserved
   cash, or a guarantee that a withdrawal will execute. A raw JSON response
   that omits this field is from a legacy deployment and differs from an
   upgraded market explicitly reporting `"0"`; Rust consumers deserialize an
   omitted field as zero for compatibility and therefore lose that distinction.
   The field appears on live markets only after a released version containing it
   has been deployed or upgraded.

2. **Oracle Health**: Verify price feed freshness and accuracy
   ```bash
   # Check oracle prices
   near contract call-function as-read-only pyth-oracle.near get_price json-args '{"price_identifier": "<asset-price-id>"}' network-config mainnet now
   
   # Check LST oracle adapter
   near contract call-function as-read-only lst.oracle.tmplr.near get_price_data json-args '{}' network-config mainnet now
   ```
   
   **Price Feed Status**: Monitor price feed health at [Pyth Network Price Feeds](https://insights.pyth.network/price-feeds)

#### Market Data Analysis
Using available view functions:

- **Supply Positions**: Monitor individual and aggregate supply positions
  ```bash
  # Get supply positions
  near contract call-function as-read-only <market-address> list_supply_positions json-args '{"offset": 0, "count": 100}' network-config mainnet now
  ```

- **Withdrawal Queue**: Check pending withdrawal requests
  ```bash
  # Check withdrawal queue status
  near contract call-function as-read-only <market-address> get_supply_withdrawal_queue_status json-args {} network-config mainnet now
  ```

- **Historical Snapshots**: Analyze market history
  ```bash
  # Get finalized snapshots for historical analysis
  near contract call-function as-read-only <market-address> list_finalized_snapshots json-args '{"offset": 0, "count": 10}' network-config mainnet now
  ```

- **Total Value Locked**: Monitor TVL at [DefiLlama - Templar Protocol](https://defillama.com/protocol/tvl/templar-protocol)

- **Utilization Rate**: Calculate from borrow asset metrics
  ```bash
  # Get borrow asset metrics to calculate utilization (borrowed / available)
  near contract call-function as-read-only <market-address> get_borrow_asset_metrics json-args {} network-config mainnet now
  ```

- **Supplier Yield Availability**: `min(accrued_yield, paid_to_fees)` is the
  largest yield-only request that can be paid without consuming principal. Such
  a request meets the configured minimum only when
  `min(accrued_yield, paid_to_fees) >= supply_withdrawal_range.minimum`. This is
  not a general withdrawal-eligibility check: principal may cover the remainder
  of a larger request. Existing eligibility checks still apply, and other
  withdrawals can consume the shared pool before execution.

- **Current Interest Rate**: Monitor current rate for supply positions
  ```bash
  # Get current yield rate for suppliers
  near contract call-function as-read-only <market-address> get_last_yield_rate json-args {} network-config mainnet now
  ```
  *Note: Historical interest rate analysis requires an indexer for time-series data*

## Risk Management

### Economic Risk Assessment

#### Available Analysis Tools
- **Market Configuration Review**: Analyze MCR ratios and interest rate models *TODO: Get parameters from each market deployed*
- **Oracle Price Monitoring**: Track price volatility and feed reliability
  - Individual feeds: [Pyth Network Price Feeds](https://insights.pyth.network/price-feeds)
  - Overall status: [Pyth Network Status](https://status.pyth.network/)
- **Liquidation Efficiency**: Monitor liquidation success rates *TODO: Get data from liquidator*
- **Position Analysis**: Assess individual and aggregate position health
  - Individual positions: [My Account](https://app.templarfi.org/borrow-supply/my-account)
  - Aggregate analysis: *TODO: Create from contract data*

#### Risk Mitigation Strategies
- **Conservative Parameters**: Well-tested collateralization ratios
- **Oracle Integration**: Multiple validation layers for price feeds
- **Liquidation Incentives**: Economic incentives for timely liquidations
- **Interest Rate Models**: Dynamic models responding to market conditions

### Operational Monitoring

For operational monitoring procedures, refer to:

- **Smart Contract Health**: See [Protocol Health Checks](#protocol-health-checks) for contract monitoring procedures
- **Gas Efficiency**: See [Gas Usage Monitoring](#gas-usage-monitoring) for performance analysis tools
- **Network Dependencies**: Monitor external service health using the links below:
  - **NEAR Network Performance**: [NEAR Status](https://status.near.org/)
  - **Oracle Provider Status**: [Pyth Network Status](https://status.pyth.network/)

### Criminal Activity Monitoring

#### SEVERE Account Labels
- **SEVERE Account Detection**: Telegram notification of SEVERE accounts interacting with Templar contracts
- **Monitoring Repository**: Details available at [templar-monitoring](https://github.com/Templar-Protocol/templar-monitoring)
