use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{
    collections::LookupMap,
    env,
    json_types::{U128, U64},
    near, require, AccountId, BorshStorageKey, PanicOnDefault, PromiseOrValue,
};

pub mod asset;
use asset::FungibleAsset;

pub mod vault;
use vault::Vault;

#[derive(BorshStorageKey, Debug, Clone, Copy, Hash)]
#[near(serializers = [borsh])]
enum StorageKey {
    Vaults,
    Vault(u64),
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct TemplarProtocol {
    next_id: u64,
    vaults: LookupMap<u64, Vault>,
}

#[near]
impl TemplarProtocol {
    #[init]
    pub fn new() -> Self {
        Self {
            next_id: 0,
            vaults: LookupMap::new(StorageKey::Vaults),
        }
    }

    fn generate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    #[payable]
    pub fn create_vault(
        &mut self,
        loan_asset_id: AccountId,
        collateral_asset_id: AccountId,
        min_collateral_ratio: (u8, u8),
    ) -> U64 {
        let vault_id = self.generate_id();
        let vault = Vault::new(
            StorageKey::Vault(vault_id),
            FungibleAsset::Nep141(loan_asset_id),
            FungibleAsset::Nep141(collateral_asset_id),
            min_collateral_ratio,
        );

        self.vaults.insert(&vault_id, &vault);

        vault_id.into()
    }

    pub fn get_vault(&self, vault_id: U64) -> ViewVault {
        let vault = self.vaults.get(&vault_id.0).unwrap();
        ViewVault {
            vault_id,
            loan_asset_id: vault.loan_asset_id,
            collateral_asset_id: vault.collateral_asset_id,
            min_collateral_ratio: vault.min_collateral_ratio,
        }
    }
}

#[near(serializers = [json])]
pub struct ViewVault {
    pub vault_id: U64,
    pub loan_asset_id: FungibleAsset,
    pub collateral_asset_id: FungibleAsset,
    pub min_collateral_ratio: (u8, u8),
}

#[near(serializers = [json])]
pub struct FtOnTransferMsg {
    vault_id: U64,
    kind: DepositKind,
}

#[near(serializers = [json])]
pub enum DepositKind {
    Loan,
    Collateralize,
    Repay,
}

#[near]
impl FungibleTokenReceiver for TemplarProtocol {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let predecessor_id = env::predecessor_account_id();

        let (vault_id, deposit_kind) = {
            let parsed = serde_json::from_str::<FtOnTransferMsg>(&msg).unwrap();
            (parsed.vault_id.0, parsed.kind)
        };
        let mut vault = self.vaults.get(&vault_id).unwrap();

        let refund = match deposit_kind {
            DepositKind::Loan => {
                require!(
                    FungibleAsset::Nep141(predecessor_id) == vault.loan_asset_id,
                    "Vault does not support this asset for loans",
                );

                let lender_id = sender_id;

                let block_height = env::block_height();
                vault.update_fees_earned(&lender_id, block_height);
                vault.record_lender_deposit(&lender_id, amount.0);
                0
            }
            DepositKind::Collateralize => {
                require!(
                    FungibleAsset::Nep141(predecessor_id) == vault.collateral_asset_id,
                    "Vault does not support this asset as collateral",
                );

                let borrower_id = sender_id;
                vault.record_borrower_collateral_deposit(&borrower_id, amount.0);
                0
            }
            DepositKind::Repay => {
                require!(
                    FungibleAsset::Nep141(predecessor_id) == vault.loan_asset_id,
                    "Vault does not support this asset for loans",
                );

                let borrower_id = sender_id;
                let repay_amount = u128::min(
                    amount.0,
                    vault
                        .get_borrower_entry(&borrower_id)
                        .map_or(0, |entry| entry.loan_asset_borrowed),
                );
                vault.record_borrower_loan_asset_repay(&borrower_id, repay_amount);
                amount.0 - repay_amount
            }
        };

        self.vaults.insert(&vault_id, &vault);

        PromiseOrValue::Value(U128(refund))
    }
}
