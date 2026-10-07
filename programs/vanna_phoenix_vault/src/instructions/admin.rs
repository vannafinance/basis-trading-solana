use crate::constants::*;
use crate::errors::VaultError;
use crate::events::{DepositsSet, VaultCreated};
use crate::instructions::common::{asset_mint_decimals, create_ata_idempotent};
use crate::nav::Market;
use crate::phoenix::*;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::{get_associated_token_address_with_program_id, AssociatedToken};
use anchor_spl::token::{Mint, Token};

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// The program's upgrade authority: whoever deployed it, so nobody can take the admin role
    /// by initializing a fresh deployment first.
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ VaultError::Unauthorized)]
    pub program: Program<'info, crate::program::VannaPhoenixVault>,
    #[account(constraint = program_data.upgrade_authority_address == Some(admin.key()) @ VaultError::Unauthorized)]
    pub program_data: Account<'info, ProgramData>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Box<Account<'info, Config>>,
    pub usdc_mint: Box<Account<'info, Mint>>,
    /// Phoenix's collateral token (PhUSD).
    pub phoenix_mint: Box<Account<'info, Mint>>,
    /// CHECK: Phoenix's perp asset map.
    #[account(owner = PHOENIX_PROGRAM_ID)]
    pub perp_asset_map: UncheckedAccount<'info>,
    /// CHECK: Phoenix's withdraw queue.
    #[account(owner = PHOENIX_PROGRAM_ID)]
    pub withdraw_queue: UncheckedAccount<'info>,
    /// CHECK: Phoenix's token vault for its collateral token, a token account it owns.
    pub global_vault: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

/// The swap venues the vault authority may sign for: Jupiter v6, or none (swaps off). The
/// venue runs with the authority's signature, so an arbitrary one could mint shares or move
/// the vault's Phoenix collateral; it is fixed here rather than trusted to the admin key.
fn check_swap_program(swap_program: &Pubkey) -> Result<()> {
    require!(
        *swap_program == JUPITER_PROGRAM_ID || *swap_program == Pubkey::default(),
        VaultError::InvalidParameter
    );
    Ok(())
}

pub fn initialize(ctx: Context<Initialize>, keeper: Pubkey, swap_program: Pubkey) -> Result<()> {
    check_swap_program(&swap_program)?;
    let a = &ctx.accounts;
    let global_vault = Pubkey::find_program_address(
        &[b"vault", a.phoenix_mint.key().as_ref()],
        &PHOENIX_PROGRAM_ID,
    )
    .0;
    require_keys_eq!(
        a.global_vault.key(),
        global_vault,
        VaultError::InvalidPhoenixAccount
    );
    let config = Config {
        admin: a.admin.key(),
        keeper,
        usdc_mint: a.usdc_mint.key(),
        phoenix_mint: a.phoenix_mint.key(),
        perp_asset_map: a.perp_asset_map.key(),
        withdraw_queue: a.withdraw_queue.key(),
        global_vault,
        swap_program,
        paused: false,
        bump: ctx.bumps.config,
    };
    ctx.accounts.config.set_inner(config);
    Ok(())
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
}

pub fn set_config(
    ctx: Context<AdminOnly>,
    admin: Pubkey,
    keeper: Pubkey,
    swap_program: Pubkey,
    paused: bool,
) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.require_admin(ctx.accounts.admin.key)?;
    require!(admin != Pubkey::default(), VaultError::InvalidParameter);
    check_swap_program(&swap_program)?;
    config.admin = admin;
    config.keeper = keeper;
    config.swap_program = swap_program;
    config.paused = paused;
    Ok(())
}

/// A vault's caps and trading settings; see `BasisVault` for each.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct VaultParams {
    pub max_position_usd: u64,
    pub max_asset_units: u64,
    pub max_oi_share_bps: u16,
    pub max_deposit_usd: u64,
    pub slippage_bps: u16,
    pub keeper_band_bps: u16,
    pub max_rebalance_usd: u64,
    pub margin_bps: u16,
    pub target_margin_bps: u16,
    pub deposits_open: bool,
    pub after_hours_deposits: bool,
}

impl VaultParams {
    fn apply(&self, vault: &mut BasisVault) -> Result<()> {
        // A band under `MIN_SLIPPAGE_BPS` would let no order fill and nobody leave.
        require!(
            (MIN_SLIPPAGE_BPS..=1_000).contains(&self.slippage_bps)
                && self.keeper_band_bps > 0
                && self.keeper_band_bps <= self.slippage_bps
                && self.margin_bps > 0
                && self.max_oi_share_bps > 0
                && self.max_oi_share_bps as u128 <= BPS_SCALE,
            VaultError::InvalidParameter
        );
        // A spot vault's deposits must land above its margin floor.
        require!(
            vault.kind == VaultKind::NativeSol || self.target_margin_bps > self.margin_bps,
            VaultError::InvalidParameter
        );
        vault.max_position_usd = self.max_position_usd;
        vault.max_asset_units = self.max_asset_units;
        vault.max_oi_share_bps = self.max_oi_share_bps;
        vault.max_deposit_usd = self.max_deposit_usd;
        vault.slippage_bps = self.slippage_bps;
        vault.keeper_band_bps = self.keeper_band_bps;
        vault.max_rebalance_usd = self.max_rebalance_usd;
        vault.margin_bps = self.margin_bps;
        vault.target_margin_bps = self.target_margin_bps;
        vault.deposits_open = self.deposits_open;
        vault.after_hours_deposits = self.after_hours_deposits;
        Ok(())
    }
}

/// The Phoenix market a vault shorts: its asset id and lot and tick sizes.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct MarketParams {
    pub asset_id: u32,
    pub base_lot_decimals: u8,
    pub tick_size: u64,
}

#[derive(Accounts)]
#[instruction(kind: VaultKind, market: MarketParams, params: VaultParams, authority_nonce: u64)]
pub struct CreateVault<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: the asset's mint, or the native mint for the SOL vault; read in the handler.
    pub asset_mint: UncheckedAccount<'info>,
    #[account(
        init,
        payer = admin,
        space = 8 + BasisVault::INIT_SPACE,
        seeds = [BASIS_VAULT_SEED, asset_mint.key().as_ref()],
        bump,
    )]
    pub basis_vault: Box<Account<'info, BasisVault>>,
    /// CHECK: the vault's data-less authority PDA, under the admin's nonce.
    #[account(
        seeds = [AUTHORITY_SEED, basis_vault.key().as_ref(), &authority_nonce.to_le_bytes()],
        bump,
    )]
    pub authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = admin,
        seeds = [SHARE_MINT_SEED, basis_vault.key().as_ref()],
        bump,
        mint::decimals = SHARE_DECIMALS,
        mint::authority = authority,
    )]
    pub share_mint: Box<Account<'info, Mint>>,
    #[account(address = config.usdc_mint)]
    pub usdc_mint: Box<Account<'info, Mint>>,
    #[account(address = config.phoenix_mint)]
    pub phoenix_mint: Box<Account<'info, Mint>>,
    /// CHECK: the authority's USDC account, created here.
    #[account(mut)]
    pub vault_usdc: UncheckedAccount<'info>,
    /// CHECK: the authority's Phoenix-token account, created here.
    #[account(mut)]
    pub vault_phoenix: UncheckedAccount<'info>,
    /// CHECK: the authority's asset account (spot vaults), created here.
    #[account(mut)]
    pub vault_asset: Option<UncheckedAccount<'info>>,
    /// CHECK: the authority's Phoenix trader account, registered here.
    #[account(mut)]
    pub trader_account: UncheckedAccount<'info>,
    /// CHECK: Phoenix.
    #[account(address = PHOENIX_PROGRAM_ID)]
    pub phoenix_program: UncheckedAccount<'info>,
    /// CHECK: Phoenix's log authority.
    #[account(address = PHOENIX_LOG_AUTHORITY)]
    pub log_authority: UncheckedAccount<'info>,
    /// CHECK: Phoenix's global configuration.
    #[account(mut, address = PHOENIX_GLOBAL_CONFIG)]
    pub global_config: UncheckedAccount<'info>,
    /// CHECK: the market's orderbook.
    #[account(owner = PHOENIX_PROGRAM_ID)]
    pub orderbook: UncheckedAccount<'info>,
    /// CHECK: the market's spline collection.
    #[account(owner = PHOENIX_PROGRAM_ID)]
    pub spline_collection: UncheckedAccount<'info>,
    /// CHECK: Phoenix's perp asset map, where the market must be listed.
    #[account(address = config.perp_asset_map)]
    pub perp_asset_map: UncheckedAccount<'info>,
    pub token_program: Program<'info, Token>,
    /// CHECK: the asset mint's owner, SPL Token or Token-2022; checked in the handler. SPL
    /// Token for the SOL vault.
    pub asset_token_program: UncheckedAccount<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Adds the vault of an asset with its share mint and token accounts, and registers its
/// Phoenix trader. Phoenix must then onboard the trader before the vault can trade.
///
/// A spot vault's asset may be a Token-2022 token, as tokenized stocks are; see
/// `asset_mint_decimals` for the extensions it may have.
///
/// The market is checked against Phoenix's listing: its order book, spline collection, tick
/// size and lot size. The authority's address takes `authority_nonce`, a number the admin picks
/// (at random): Phoenix lets anyone register a trader account for any authority, so with a
/// fixed address someone could register the vault's trader first and block the vault forever.
pub fn create_vault(
    ctx: Context<CreateVault>,
    kind: VaultKind,
    market: MarketParams,
    params: VaultParams,
    authority_nonce: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    a.config.require_admin(a.admin.key)?;
    let asset_mint = a.asset_mint.key();
    let asset_token_program = a.asset_token_program.key();
    let decimals = match kind {
        VaultKind::NativeSol => {
            require_keys_eq!(asset_mint, NATIVE_MINT, VaultError::InvalidMint);
            require_keys_eq!(
                asset_token_program,
                anchor_spl::token::ID,
                VaultError::InvalidMint
            );
            require!(a.vault_asset.is_none(), VaultError::WrongVaultKind);
            NATIVE_SOL_DECIMALS
        }
        VaultKind::Spot => {
            let info = a.asset_mint.to_account_info();
            require_keys_eq!(*info.owner, asset_token_program, VaultError::InvalidMint);
            // The vault's USDC, Phoenix-token and wrapped-SOL accounts must not double as its
            // asset account.
            require!(
                asset_mint != a.config.usdc_mint
                    && asset_mint != a.config.phoenix_mint
                    && asset_mint != NATIVE_MINT,
                VaultError::InvalidMint
            );
            require!(a.vault_asset.is_some(), VaultError::WrongVaultKind);
            asset_mint_decimals(&info)?
        }
    };
    require!(market.tick_size > 0, VaultError::InvalidParameter);
    Market {
        decimals,
        base_lot_decimals: market.base_lot_decimals,
        tick_size: market.tick_size,
        multiplier: MULTIPLIER_SCALE,
    }
    .units_per_lot()?;
    // The market must be the one Phoenix lists under this asset id, with these sizes.
    let (orderbook, tick_size, base_lot_decimals) =
        market_listing(&a.perp_asset_map, market.asset_id)?;
    require!(
        a.orderbook.key() == orderbook
            && a.spline_collection.key() == spline_collection_address(&orderbook)
            && market.tick_size == tick_size
            && market.base_lot_decimals == base_lot_decimals,
        VaultError::InvalidPhoenixAccount
    );

    let vault_key = a.basis_vault.key();
    let authority = a.authority.key();
    let trader_account = trader_account_address(&authority);
    require_keys_eq!(
        a.trader_account.key(),
        trader_account,
        VaultError::InvalidPhoenixAccount
    );

    let admin = a.admin.to_account_info();
    let authority_info = a.authority.to_account_info();
    let system_program = a.system_program.to_account_info();
    let token_program = a.token_program.to_account_info();
    let asset_program = a.asset_token_program.to_account_info();
    let mut atas = vec![
        (
            a.vault_usdc.to_account_info(),
            a.usdc_mint.to_account_info(),
            &token_program,
        ),
        (
            a.vault_phoenix.to_account_info(),
            a.phoenix_mint.to_account_info(),
            &token_program,
        ),
    ];
    if let Some(vault_asset) = &a.vault_asset {
        atas.push((
            vault_asset.to_account_info(),
            a.asset_mint.to_account_info(),
            &asset_program,
        ));
    }
    for (ata, mint, program) in &atas {
        require_keys_eq!(
            ata.key(),
            get_associated_token_address_with_program_id(&authority, mint.key, program.key),
            VaultError::InvalidTokenAccount
        );
        create_ata_idempotent(
            &admin,
            ata,
            &authority_info,
            mint,
            &system_program,
            program,
            &[],
        )?;
    }
    register_trader(
        &a.phoenix_program,
        &a.log_authority,
        &a.global_config,
        &admin,
        &authority_info,
        &a.trader_account,
        &system_program,
        MAX_PHOENIX_POSITIONS,
    )?;

    let mut vault = BasisVault {
        kind,
        asset_mint,
        asset_decimals: decimals,
        asset_token_program,
        share_mint: a.share_mint.key(),
        authority,
        usdc_account: a.vault_usdc.key(),
        phoenix_account: a.vault_phoenix.key(),
        asset_account: a.vault_asset.as_ref().map(|v| v.key()).unwrap_or_default(),
        trader_account,
        asset_id: market.asset_id,
        orderbook: a.orderbook.key(),
        spline_collection: a.spline_collection.key(),
        base_lot_decimals: market.base_lot_decimals,
        tick_size: market.tick_size,
        max_position_usd: 0,
        max_asset_units: 0,
        max_oi_share_bps: 0,
        max_deposit_usd: 0,
        slippage_bps: 0,
        keeper_band_bps: 0,
        max_rebalance_usd: 0,
        rebalance_window_start: 0,
        rebalance_window_usd: 0,
        margin_bps: 0,
        target_margin_bps: 0,
        deposits_open: false,
        after_hours_deposits: false,
        bump: ctx.bumps.basis_vault,
        authority_nonce: authority_nonce.to_le_bytes(),
        authority_bump: ctx.bumps.authority,
    };
    params.apply(&mut vault)?;
    emit!(VaultCreated {
        basis_vault: vault_key,
        asset_mint,
        share_mint: vault.share_mint,
        trader_account,
        asset_id: market.asset_id,
    });
    ctx.accounts.basis_vault.set_inner(vault);
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateVault<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [BASIS_VAULT_SEED, basis_vault.asset_mint.as_ref()], bump = basis_vault.bump)]
    pub basis_vault: Box<Account<'info, BasisVault>>,
}

pub fn update_vault(ctx: Context<UpdateVault>, params: VaultParams) -> Result<()> {
    ctx.accounts.config.require_admin(ctx.accounts.admin.key)?;
    params.apply(&mut ctx.accounts.basis_vault)
}

#[derive(Accounts)]
pub struct SetDeposits<'info> {
    /// The keeper or the admin.
    pub signer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [BASIS_VAULT_SEED, basis_vault.asset_mint.as_ref()], bump = basis_vault.bump)]
    pub basis_vault: Box<Account<'info, BasisVault>>,
}

/// Opens or closes a vault's deposits; the keeper's funding guard. When a market's funding
/// turns poor (shorts earning little, or paying), new money would only dilute what the
/// holders earn, so the keeper closes deposits until it recovers. The admin sets the caps.
pub fn set_deposits_open(ctx: Context<SetDeposits>, open: bool) -> Result<()> {
    let signer = ctx.accounts.signer.key();
    ctx.accounts.config.require_keeper(&signer)?;
    let vault = &mut ctx.accounts.basis_vault;
    vault.deposits_open = open;
    emit!(DepositsSet {
        basis_vault: vault.key(),
        open,
        by: signer,
    });
    Ok(())
}
