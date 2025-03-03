# Liquidation

> [!NOTE]
> Until price oracle support is fully implemented, the test version of the contract uses price data provided as an argument to the function call.

Liquidation is the process by which the asset collateralizing certain positions may be reappropriated e.g. to recover assets for an undercollateralized position.

A liquidator is a third party willing to send a quantity of a market's borrow asset (usually a stablecoin) to the market in exchange for the amount of collateral asset supporting a specific account's position. As compensation for this service, the liquidator receives an exchange rate that is slightly better than the current rate. This difference in rates is called the "liquidator spread," and the maximum liquidator spread is configurable on a per-market basis.

A liquidator will follow this high-level workflow:

1. The liquidator obtains a list of accounts borrowing from the market by calling `list_borrows`.
2. The liquidator checks the status of each account by calling `get_borrow_status(account_id)`.
3. If an account's status is `Liquidation`, that means the liquidator can obtain a spread by sending an amount of borrow asset to the market. The maximum spread is specified in the market configuration, which can be obtained by calling `get_configuration`.
4. To perform the liquidation, the liquidator transfers the appropriate amount of borrow asset to the market via [`ft_transfer_call`](https://docs.near.org/build/primitives/ft#attaching-fts-to-a-call). That is to say, the liquidator calls `ft_transfer_call` on _the borrow asset's smart contract_, specifying the market as the receiver. The `msg` parameter indicates 1) that the transfer is for a liquidation, and 2) which account is to be liquidated.

Thus, the arguments to a liquidation call might look something like this:

```json
{
  "amount": "42",
  "msg": {
    "Liquidate": {
      "account_id": "account-to-liquidate.testnet"
    }
  },
  "receiver_id": "templar_market.testnet"
}
```

> [!NOTE]
> It is the responsibility of the liquidator to calculate the optimal amount of tokens to attach to a liquidation call. The market will either completely accept or completely reject the liquidation attempt&mdash;no refunds!
