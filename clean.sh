export ACCT = {YOUR_ACCOUNT_ID}
export TEMPLAR = "g.${ACCT}"
export USDT = "usdt.fakes.testnet"

near delete $TEMPLAR --masterAccount $ACCT 

cargo clean 