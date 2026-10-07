use anchor_lang::prelude::*;

#[error_code]
pub enum VaultError {
    #[msg("Signer is not allowed to do this")]
    Unauthorized,
    #[msg("The protocol is paused")]
    Paused,
    #[msg("Deposits into this vault are closed")]
    DepositsClosed,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Division by zero")]
    DivisionByZero,
    #[msg("Mint does not match the vault")]
    InvalidMint,
    #[msg("Token account does not match the vault")]
    InvalidTokenAccount,
    #[msg("Phoenix account does not match the vault")]
    InvalidPhoenixAccount,
    #[msg("Phoenix trader index accounts are missing")]
    MissingPhoenixAccounts,
    #[msg("Phoenix instruction could not be built or read")]
    PhoenixInstruction,
    #[msg("Phoenix returned an unusable price")]
    InvalidPrice,
    #[msg("The vault's position would pass its cap")]
    MaxPositionReached,
    #[msg("The vault's short would pass its share of the market's open interest")]
    OpenInterestShareReached,
    #[msg("The deposit is larger than the vault takes at once")]
    DepositTooLarge,
    #[msg("The market is trading after hours: deposits wait for the session")]
    MarketAfterHours,
    #[msg("The asset's token has an extension the vault does not support")]
    UnsupportedMint,
    #[msg("The vault owes more than it holds: deposits would pay its debt")]
    VaultInsolvent,
    #[msg("The keeper's rebalancing would pass its daily limit")]
    RebalanceLimitReached,
    #[msg("The token's multiplier is changing: deposits wait until it has settled")]
    MultiplierChanging,
    #[msg("The swap route is not a Jupiter route the vault can size")]
    UnsupportedRoute,
    #[msg("The vault's margin would fall below its target")]
    MarginTooThin,
    #[msg("Fewer shares than the minimum")]
    SharesTooLow,
    #[msg("Payout is below the minimum")]
    PayoutTooLow,
    #[msg("The deposit added no value")]
    NothingDeposited,
    #[msg("Phoenix lets nothing leave the vault right now")]
    NothingToWithdraw,
    #[msg("Not enough shares")]
    NotEnoughShares,
    #[msg("Trade price is outside the allowed band around the mark price")]
    PriceOutOfBand,
    #[msg("This vault kind does not take this account")]
    WrongVaultKind,
    #[msg("Swap accounts are missing or wrong")]
    InvalidSwapAccounts,
    #[msg("This deposit or withdrawal needs no swap")]
    SwapNotNeeded,
    #[msg("The swap did not spend the planned amount")]
    SwapAmountOff,
    #[msg("The swap paid less than the mark price allows")]
    SwapPriceTooLow,
    #[msg("The swap touched vault funds it was not given")]
    SwapMovedVaultFunds,
    #[msg("Invalid parameter")]
    InvalidParameter,
}
