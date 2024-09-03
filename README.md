# Templar Protocol

0% interest lending protocol on NEAR

## Getting Started

```sh
./build.sh
./dev-deploy.sh
```

```sh
cargo build --target wasm32-unknown-unknown --release

near dev-deploy ./target/wasm32-unknown-unknown/release/templar_protocol.wasm 

export G= TEMPLAR_ACCOUNT
export USDT=usdt.fakes.testnet
export ACCT= YOUR_ACCOUNT
```

```sh
near call $G new '{"lower_collateral_accounts": ["kenobi.testnet"]}' --accountId $G
```

## Operations

### Update Price

### Get Latest Price

```sh
near call $G get_latest_price --accountId $G
```

### Get all Loans

```sh
near call $G get_all_loans --accountId $G
```

### Deposit Collateral

```sh
near call $G deposit_collateral '{"amount": 1000000}' --accountId $G
```

### Borrow

```sh
near call $G borrow '{"usdt_amount": 1}' --accountId $G --gas 300000000000000
```

### Repay

```sh
near call $G repay '{"account_id": "kenobi.testnet", "usdt_amount": 50}' --accountId $ACCT --gas 300000000000000
```

### Get USDT Value of NEAR

```sh
near call $G get_usdt_value --accountId $G --gas 300000000000000

near call $G get_prices --accountId $G --gas 300000000000000
```

### Open and Close an Account

```sh
near call $G deposit_collateral '{"amount": 10}' --accountId $ACCT --deposit 10

near call $G borrow '{"usdt_amount": 500}' --accountId $ACCT --gas 300000000000000 

near call $USDT ft_transfer_call '{"receiver_id": "templar.kenobi.testnet", "amount": "1", "memo": "Test", "msg": "close"}' --accountId $ACCT --gas 300000000000000 --depositYocto 1

near call $G close '{}' --accountId $ACCT --gas 300000000000000
```
