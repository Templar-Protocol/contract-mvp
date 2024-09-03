const nearAPI = require('near-api-js');
const fs = require('fs');
const path = require('path');


// Initialize NEAR connection and contract variables
let near;
let walletConnection;
let contract;
let usdtContract;

// Initializing connection to NEAR
async function initNear() {
    const config = {
        networkId: 'default',
        nodeUrl: 'https://rpc.testnet.near.org',
        walletUrl: 'https://wallet.testnet.near.org',
        helperUrl: 'https://helper.testnet.near.org',
        contractName: 'dev-1688424587747-63751589033436', // Replace with your contract's account ID
        usdtContractName: 'usdt.fakes.testnet',
    };

    const { keyStores } = nearAPI;
    const homedir = require("os").homedir();
    const CREDENTIALS_DIR = ".near-credentials";
    const credentialsPath = path.join(homedir, CREDENTIALS_DIR);
    const keyStore = new keyStores.UnencryptedFileSystemKeyStore(credentialsPath);

    near = await nearAPI.connect({
        deps: {
            keyStore: keyStore
        },
        ...config
    });

    const accountId = "kenobi.testnet"; // Replace with your account ID
    const account = new nearAPI.Account(near.connection, config.contractName);

    contract = new nearAPI.Contract(account, config.contractName, {
        viewMethods: ['get_all_loans', 'get_prices', 'get_latest_price'],
        changeMethods: ['new', 'deposit_collateral', 'borrow', 'close', 'repay'],
        sender: accountId
    });

    usdtContract = new nearAPI.Contract(account, config.usdtContractName, {
        viewMethods: [
            'ft_total_supply',
            'ft_balance_of',
            'ft_metadata'
        ],
        changeMethods: [
            'ft_transfer',
            'ft_transfer_call',
            'ft_mint'
        ],
        sender: accountId
    });
}

// Call functions

async function initializeContract(lowerCollateralAccounts) {
    await contract.new({ lower_collateral_accounts: lowerCollateralAccounts });
}

async function depositCollateral(amount) {
    await contract.deposit_collateral({ amount: amount }, nearAPI.utils.format.parseNearAmount('1'));
}

async function borrow(usdtAmount) {
    await contract.borrow({ usdt_amount: usdtAmount });
}

async function closeLoan(collateral, senderId) {
    await contract.close({ collateral: collateral, sender_id: senderId });
}

async function repayLoan(usdtAmount) {
    await contract.repay({ usdt_amount: usdtAmount });
}

async function getAllLoans() {
    return await contract.get_all_loans();
}

async function getPrices() {
    return await contract.get_prices();
}

async function getLatestPrice() {
    return await contract.get_latest_price();
}

async function transferAndCall(contract, receiverId, amount, methodName, args) {
    const encodedArgs = Buffer.from(JSON.stringify(args)).toString('base64');
    const result = await contract.ft_transfer_call({
        receiver_id: receiverId,
        amount: amount,
        memo: 'Optional memo message',
        msg: encodedArgs
    });

    return result;
}

// Example usage:
(async () => {
    await initNear();
    const loans = await getAllLoans();
    console.log(loans);
})();
