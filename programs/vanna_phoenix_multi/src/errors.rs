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
    #[msg("Mint does not match")]
    InvalidMint,
    #[msg("Token account does not match")]
    InvalidTokenAccount,
    #[msg("Phoenix account does not match")]
    InvalidPhoenixAccount,
    #[msg("Phoenix trader index accounts are missing")]
    MissingPhoenixAccounts,
    #[msg("Phoenix instruction could not be built or read")]
    PhoenixInstruction,
    #[msg("Phoenix returned an unusable price")]
    InvalidPrice,
    #[msg("Every leg of the vault must be passed, in order, with its accounts")]
    InvalidLegAccounts,
    #[msg("The vault has as many legs as it may")]
    TooManyLegs,
    #[msg("A leg already trades this market or holds this asset")]
    DuplicateLeg,
    #[msg("The leg is not active: the keeper may only unwind it")]
    LegInactive,
    #[msg("The leg is retired")]
    LegRetired,
    #[msg("Only a leg holding nothing and short nothing can be retired")]
    LegNotFlat,
    #[msg("The leg's short would pass its cap")]
    MaxPositionReached,
    #[msg("The leg's short would pass its share of the market's open interest")]
    OpenInterestShareReached,
    #[msg("The leg would pass its share of the vault's value")]
    LegShareReached,
    #[msg("The vault would pass its total value cap")]
    MaxTvlReached,
    #[msg("The deposit is larger than the vault takes at once")]
    DepositTooLarge,
    #[msg("The market is trading after hours")]
    MarketAfterHours,
    #[msg("The asset's token has an extension the vault does not support")]
    UnsupportedMint,
    #[msg("The vault owes more than it holds")]
    VaultInsolvent,
    #[msg("A token the vault holds is changing its multiplier: wait until it has settled")]
    MultiplierChanging,
    #[msg("The vault's margin would fall below its target")]
    MarginTooThin,
    #[msg("The keeper would take the vault's USDC below its buffer and pending withdrawals")]
    BufferTooThin,
    #[msg("The keeper's trading would pass its daily limit")]
    TurnoverLimitReached,
    #[msg("Fewer shares than the minimum")]
    SharesTooLow,
    #[msg("Payout is below the minimum")]
    PayoutTooLow,
    #[msg("The deposit added no value")]
    NothingDeposited,
    #[msg("Nothing to withdraw")]
    NothingToWithdraw,
    #[msg("Not enough USDC in the vault's buffer: request the withdrawal instead")]
    NotEnoughLiquidity,
    #[msg("Not enough shares")]
    NotEnoughShares,
    #[msg("Not enough of the asset")]
    NotEnoughAsset,
    #[msg("This leg kind does not take this account")]
    WrongLegKind,
    #[msg("Swap accounts are missing or wrong")]
    InvalidSwapAccounts,
    #[msg("The swap route is not a Jupiter route the vault can size")]
    UnsupportedRoute,
    #[msg("The swap did not spend the planned amount")]
    SwapAmountOff,
    #[msg("The swap paid less than the mark price allows")]
    SwapPriceTooLow,
    #[msg("The swap touched vault funds it was not given")]
    SwapMovedVaultFunds,
    #[msg("Invalid parameter")]
    InvalidParameter,
    #[msg("The keeper returned this request: its user can only take the shares back")]
    RequestReturned,
    #[msg("A leg that can no longer be priced must be deactivated before it is retired")]
    RetireActiveLeg,
}
