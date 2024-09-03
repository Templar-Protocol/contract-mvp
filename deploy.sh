
export ACCT = {YOUR_ACCOUNT_ID}
export TEMPLAR = "g.${ACCT}"
export USDT = "usdt.fakes.testnet"

# Deploy
cargo build --target wasm32-unknown-unknown --release
near create-account $TEMPLAR --masterAccount $ACCT --initialBalance 10
near deploy --accountId $TEMPLAR --wasmFile ./target/wasm32-unknown-unknown/release/templar_protocol.wasm 


# Register and provide USDT token
near call $USDT register_account '{"account_id": "'$TEMPLAR'"}' --accountId $ACCT --amount 0.125 --gas 300000000000000
near call $USDT ft_transfer '{"receiver_id": "'$TEMPLAR'", "amount": "10"}' --accountId $ACCT --amount 0.125 --gas 300000000000000