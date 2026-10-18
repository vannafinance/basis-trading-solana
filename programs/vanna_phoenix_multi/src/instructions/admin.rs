use crate::constants::*;
use crate::errors::VaultError;
use crate::events::{DepositsSet, LegAdded, LegRetired, VaultCreated};
use crate::instructions::common::{asset_mint_decimals, create_ata_idempotent, multiplier_range};
use crate::instructions::vault_core::*;
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
    pub program: Program<'info, crate::program::VannaPhoenixMultiVault>,
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
/// the vault's assets; it is fixed here rather than trusted to the admin key.
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

/// The vault's caps, fees and trading limits; see `Vault` for each.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct VaultParams {
    pub max_tvl_usd: u64,
    pub max_deposit_usd: u64,
    pub entry_fee_bps: u16,
    pub exit_fee_bps: u16,
    pub buffer_bps: u16,
    pub margin_bps: u16,
    pub slippage_bps: u16,
    pub max_turnover_usd: u64,
    pub deposits_open: bool,
}

impl VaultParams {
    fn apply(&self, vault: &mut Vault) -> Result<()> {
        // A band under `MIN_SLIPPAGE_BPS` would let no order fill.
        require!(
            (MIN_SLIPPAGE_BPS..=MAX_SLIPPAGE_BPS).contains(&self.slippage_bps)
                && self.margin_bps > 0
                && self.entry_fee_bps <= MAX_FEE_BPS
                && self.exit_fee_bps <= MAX_FEE_BPS
                && self.buffer_bps as u128 <= BPS_SCALE,
            VaultError::InvalidParameter
        );
        vault.max_tvl_usd = self.max_tvl_usd;
        vault.max_deposit_usd = self.max_deposit_usd;
        vault.entry_fee_bps = self.entry_fee_bps;
        vault.exit_fee_bps = self.exit_fee_bps;
        vault.buffer_bps = self.buffer_bps;
        vault.margin_bps = self.margin_bps;
        vault.slippage_bps = self.slippage_bps;
        vault.max_turnover_usd = self.max_turnover_usd;
        vault.deposits_open = self.deposits_open;
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(id: u16, params: VaultParams, authority_nonce: u64)]
pub struct CreateVault<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(
        init,
        payer = admin,
        space = 8 + Vault::INIT_SPACE,
        seeds = [VAULT_SEED, &id.to_le_bytes()],
        bump,
    )]
    pub vault: Box<Account<'info, Vault>>,
    /// CHECK: the vault's data-less authority PDA, under the admin's nonce.
    #[account(
        seeds = [AUTHORITY_SEED, vault.key().as_ref(), &authority_nonce.to_le_bytes()],
        bump,
    )]
    pub authority: UncheckedAccount<'info>,
    #[account(
        init,
        payer = admin,
        seeds = [SHARE_MINT_SEED, vault.key().as_ref()],
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
    /// CHECK: the authority's share account (pending requests' escrow), created here.
    #[account(mut)]
    pub escrow_shares: UncheckedAccount<'info>,
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
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Creates the vault with its share mint and token accounts, and registers its Phoenix
/// trader. Phoenix must then onboard the trader before the vault can trade. It starts with no
/// legs: `add_leg` adds each market.
///
/// The authority's address takes `authority_nonce`, a number the admin picks (at random):
/// Phoenix lets anyone register a trader account for any authority, so with a fixed address
/// someone could register the vault's trader first and block the vault forever.
pub fn create_vault(
    ctx: Context<CreateVault>,
    id: u16,
    params: VaultParams,
    authority_nonce: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    a.config.require_admin(a.admin.key)?;
    let vault_key = a.vault.key();
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
    let atas = [
        (a.vault_usdc.to_account_info(), a.usdc_mint.to_account_info()),
        (a.vault_phoenix.to_account_info(), a.phoenix_mint.to_account_info()),
        (a.escrow_shares.to_account_info(), a.share_mint.to_account_info()),
    ];
    for (ata, mint) in &atas {
        require_keys_eq!(
            ata.key(),
            get_associated_token_address_with_program_id(&authority, mint.key, token_program.key),
            VaultError::InvalidTokenAccount
        );
        create_ata_idempotent(
            &admin,
            ata,
            &authority_info,
            mint,
            &system_program,
            &token_program,
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

    let mut vault = Vault {
        id,
        share_mint: a.share_mint.key(),
        authority,
        usdc_account: a.vault_usdc.key(),
        phoenix_account: a.vault_phoenix.key(),
        escrow_shares: a.escrow_shares.key(),
        trader_account,
        leg_count: 0,
        next_request_id: 0,
        pending_shares: 0,
        max_tvl_usd: 0,
        max_deposit_usd: 0,
        entry_fee_bps: 0,
        exit_fee_bps: 0,
        buffer_bps: 0,
        margin_bps: 0,
        slippage_bps: 0,
        max_turnover_usd: 0,
        turnover_window_start: 0,
        turnover_window_usd: 0,
        deposits_open: false,
        bump: ctx.bumps.vault,
        authority_nonce: authority_nonce.to_le_bytes(),
        authority_bump: ctx.bumps.authority,
    };
    params.apply(&mut vault)?;
    emit!(VaultCreated {
        vault: vault_key,
        share_mint: vault.share_mint,
        trader_account,
    });
    ctx.accounts.vault.set_inner(vault);
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateVault<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
}

pub fn update_vault(ctx: Context<UpdateVault>, params: VaultParams) -> Result<()> {
    ctx.accounts.config.require_admin(ctx.accounts.admin.key)?;
    params.apply(&mut ctx.accounts.vault)
}

/// The Phoenix market a leg shorts: its asset id and lot and tick sizes.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct MarketParams {
    pub asset_id: u32,
    pub base_lot_decimals: u8,
    pub tick_size: u64,
}

/// A leg's caps; see `Leg` for each.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct LegParams {
    pub active: bool,
    pub max_notional_usd: u64,
    pub max_asset_units: u64,
    pub max_oi_share_bps: u16,
    pub max_nav_share_bps: u16,
    pub after_hours_trading: bool,
}

impl LegParams {
    fn apply(&self, leg: &mut Leg) -> Result<()> {
        require!(
            self.max_oi_share_bps > 0
                && self.max_oi_share_bps as u128 <= BPS_SCALE
                && self.max_nav_share_bps > 0
                && self.max_nav_share_bps as u128 <= BPS_SCALE,
            VaultError::InvalidParameter
        );
        leg.active = self.active;
        leg.max_notional_usd = self.max_notional_usd;
        leg.max_asset_units = self.max_asset_units;
        leg.max_oi_share_bps = self.max_oi_share_bps;
        leg.max_nav_share_bps = self.max_nav_share_bps;
        leg.after_hours_trading = self.after_hours_trading;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct AddLeg<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
    #[account(
        init,
        payer = admin,
        space = 8 + Leg::INIT_SPACE,
        seeds = [LEG_SEED, vault.key().as_ref(), &[vault.leg_count]],
        bump,
    )]
    pub leg: Box<Account<'info, Leg>>,
    /// CHECK: the vault's authority.
    #[account(address = vault.authority)]
    pub authority: UncheckedAccount<'info>,
    /// CHECK: the asset's mint, or the native mint for the SOL leg; read in the handler.
    pub asset_mint: UncheckedAccount<'info>,
    /// CHECK: the authority's asset account (spot legs), created here.
    #[account(mut)]
    pub vault_asset: Option<UncheckedAccount<'info>>,
    /// CHECK: the market's orderbook.
    #[account(owner = PHOENIX_PROGRAM_ID)]
    pub orderbook: UncheckedAccount<'info>,
    /// CHECK: the market's spline collection.
    #[account(owner = PHOENIX_PROGRAM_ID)]
    pub spline_collection: UncheckedAccount<'info>,
    /// CHECK: Phoenix's perp asset map, where the market must be listed.
    #[account(address = config.perp_asset_map)]
    pub perp_asset_map: UncheckedAccount<'info>,
    /// CHECK: the asset mint's owner, SPL Token or Token-2022; checked in the handler. SPL
    /// Token for the SOL leg.
    pub asset_token_program: UncheckedAccount<'info>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Adds a market to the vault: the asset it holds and the Phoenix perp it shorts. The SOL leg
/// holds native SOL as Phoenix collateral; any other leg holds the asset as tokens (Token-2022
/// for tokenized stocks; see `asset_mint_decimals` for the extensions it may have).
///
/// The market is checked against Phoenix's listing: its order book, spline collection, tick
/// size and lot size. `remaining_accounts` are the vault's existing legs, in order: no two live
/// legs may trade one market or hold one asset (they would read the same position or tokens
/// twice), and at most `MAX_LIVE_LEGS` may be live.
pub fn add_leg<'info>(
    ctx: Context<'info, AddLeg<'info>>,
    kind: LegKind,
    market: MarketParams,
    params: LegParams,
) -> Result<()> {
    let a = &ctx.accounts;
    a.config.require_admin(a.admin.key)?;
    let vault = &a.vault;
    require!(vault.leg_count < MAX_LEGS, VaultError::TooManyLegs);
    let vault_key = vault.key();
    let asset_mint = a.asset_mint.key();
    let asset_token_program = a.asset_token_program.key();

    require!(
        ctx.remaining_accounts.len() == vault.leg_count as usize,
        VaultError::InvalidLegAccounts
    );
    let mut live = 0;
    for (index, info) in ctx.remaining_accounts.iter().enumerate() {
        require_keys_eq!(*info.owner, crate::ID, VaultError::InvalidLegAccounts);
        let other = Leg::try_deserialize(&mut &info.try_borrow_data()?[..])
            .map_err(|_| VaultError::InvalidLegAccounts)?;
        require!(
            other.vault == vault_key && other.index as usize == index,
            VaultError::InvalidLegAccounts
        );
        if other.retired {
            continue;
        }
        live += 1;
        require!(
            other.asset_id != market.asset_id && other.asset_mint != asset_mint,
            VaultError::DuplicateLeg
        );
    }
    require!(live < MAX_LIVE_LEGS, VaultError::TooManyLegs);

    let decimals = match kind {
        LegKind::NativeSol => {
            require_keys_eq!(asset_mint, NATIVE_MINT, VaultError::InvalidMint);
            require_keys_eq!(
                asset_token_program,
                anchor_spl::token::ID,
                VaultError::InvalidMint
            );
            require!(a.vault_asset.is_none(), VaultError::WrongLegKind);
            NATIVE_SOL_DECIMALS
        }
        LegKind::Spot => {
            let info = a.asset_mint.to_account_info();
            require_keys_eq!(*info.owner, asset_token_program, VaultError::InvalidMint);
            // The vault's USDC, Phoenix-token, escrow and wrapped-SOL accounts must not double
            // as a leg's asset account.
            require!(
                asset_mint != a.config.usdc_mint
                    && asset_mint != a.config.phoenix_mint
                    && asset_mint != vault.share_mint
                    && asset_mint != NATIVE_MINT,
                VaultError::InvalidMint
            );
            require!(a.vault_asset.is_some(), VaultError::WrongLegKind);
            let decimals = asset_mint_decimals(&info)?;
            // A multiplier the vault cannot read would stop every deposit and withdrawal the
            // moment the leg is added.
            if asset_token_program == anchor_spl::token_2022::ID {
                multiplier_range(&info, Clock::get()?.unix_timestamp)?;
            }
            decimals
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

    let authority = a.authority.key();
    let asset_account = match &a.vault_asset {
        Some(vault_asset) => {
            require_keys_eq!(
                vault_asset.key(),
                get_associated_token_address_with_program_id(
                    &authority,
                    &asset_mint,
                    &asset_token_program
                ),
                VaultError::InvalidTokenAccount
            );
            create_ata_idempotent(
                &a.admin.to_account_info(),
                &vault_asset.to_account_info(),
                &a.authority.to_account_info(),
                &a.asset_mint.to_account_info(),
                &a.system_program.to_account_info(),
                &a.asset_token_program.to_account_info(),
                &[],
            )?;
            vault_asset.key()
        }
        None => Pubkey::default(),
    };

    let index = vault.leg_count;
    let mut leg = Leg {
        vault: vault_key,
        index,
        kind,
        asset_mint,
        asset_decimals: decimals,
        asset_token_program,
        asset_account,
        asset_id: market.asset_id,
        orderbook: a.orderbook.key(),
        spline_collection: a.spline_collection.key(),
        base_lot_decimals: market.base_lot_decimals,
        tick_size: market.tick_size,
        active: false,
        retired: false,
        max_notional_usd: 0,
        max_asset_units: 0,
        max_oi_share_bps: 0,
        max_nav_share_bps: 0,
        after_hours_trading: false,
        bump: ctx.bumps.leg,
    };
    params.apply(&mut leg)?;
    emit!(LegAdded {
        vault: vault_key,
        leg: ctx.accounts.leg.key(),
        index,
        asset_mint,
        asset_id: market.asset_id,
    });
    ctx.accounts.leg.set_inner(leg);
    ctx.accounts.vault.leg_count = index + 1;
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateLeg<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
    #[account(
        mut,
        seeds = [LEG_SEED, vault.key().as_ref(), &[leg.index]],
        bump = leg.bump,
    )]
    pub leg: Box<Account<'info, Leg>>,
}

/// Changes a leg's caps, or stops the keeper deploying into it (`active = false`): it can
/// still be unwound.
pub fn update_leg(ctx: Context<UpdateLeg>, params: LegParams) -> Result<()> {
    ctx.accounts.config.require_admin(ctx.accounts.admin.key)?;
    require!(!ctx.accounts.leg.retired, VaultError::LegRetired);
    params.apply(&mut ctx.accounts.leg)
}

#[derive(Accounts)]
pub struct RetireLeg<'info> {
    pub admin: Signer<'info>,
    pub core: VaultCore<'info>,
    #[account(
        mut,
        seeds = [LEG_SEED, core.vault.key().as_ref(), &[leg.index]],
        bump = leg.bump,
    )]
    pub leg: Box<Account<'info, Leg>>,
}

/// Retires a flat leg: short nothing on Phoenix, and holding at most `max_units` of its asset,
/// worth at most `RETIRE_DUST_USD` (so a dust donation cannot stop it). From then on it takes
/// one account in every leg list and no Phoenix call, so its market (delisted, halted, a broken
/// token) can no longer stop the vault's deposits and withdrawals; whatever dust it held is left
/// out of the NAV. It cannot come back: add a new leg for the market instead.
/// `remaining_accounts`: Phoenix's index accounts, then every leg (`load_legs`).
///
/// Only this leg is read, and not valued if it cannot be: retiring is the way out once a leg's
/// token or market has broken, which stops every snapshot. A leg that no longer prices (its
/// multiplier unreadable, its mark at zero) must have been deactivated first, and is held to
/// `max_units`; one whose market Phoenix removed from its asset map holds no position there.
pub fn retire_leg<'info>(
    ctx: Context<'info, RetireLeg<'info>>,
    gti: u8,
    atb: u8,
    max_units: u64,
) -> Result<()> {
    let a = &ctx.accounts;
    let core = &a.core;
    core.config.require_admin(a.admin.key)?;
    require!(!a.leg.retired, VaultError::LegRetired);
    let vault_key = core.vault.key();
    let start = gti as usize + atb as usize;
    let (legs, _) = load_legs(
        &vault_key,
        &core.vault,
        ctx.remaining_accounts.get(start..).unwrap_or_default(),
    )?;
    let leg = legs
        .get(a.leg.index as usize)
        .ok_or(VaultError::InvalidLegAccounts)?;
    require_keys_eq!(leg.key, a.leg.key(), VaultError::InvalidLegAccounts);
    let trader = core.trader(ctx.remaining_accounts, gti, atb, &[], None, None)?;
    // Phoenix checks the index accounts the views below only read.
    trader.sync_native()?;
    let units = core.units(leg)?;
    require!(units <= max_units, VaultError::LegNotFlat);
    let listed = market_state(&core.perp_asset_map, leg.leg.asset_id).is_ok();
    let mark_ticks = if listed {
        let view = trader.view_asset(leg.leg.asset_id)?;
        require!(view.base_lots == 0, VaultError::LegNotFlat);
        view.mark_price_ticks
    } else {
        0
    };
    let now = Clock::get()?.unix_timestamp;
    let value = core
        .market(leg, now, Pricing::Mark)
        .and_then(|market| market.price(mark_ticks))
        .and_then(|price| price.usd_value(units as u128, leg.leg.asset_decimals, false));
    match value {
        Ok(value) => require!(value <= RETIRE_DUST_USD, VaultError::LegNotFlat),
        Err(_) => require!(!a.leg.active, VaultError::RetireActiveLeg),
    }
    emit!(LegRetired {
        vault: vault_key,
        leg: a.leg.key(),
        index: a.leg.index,
    });
    let leg = &mut ctx.accounts.leg;
    leg.retired = true;
    leg.active = false;
    Ok(())
}

#[derive(Accounts)]
pub struct SetDeposits<'info> {
    /// The keeper or the admin.
    pub signer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [VAULT_SEED, &vault.id.to_le_bytes()], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,
}

/// Opens or closes the vault's deposits: the keeper's guard for when funding across its
/// markets turns poor and new money would only dilute what the holders earn.
pub fn set_deposits_open(ctx: Context<SetDeposits>, open: bool) -> Result<()> {
    let signer = ctx.accounts.signer.key();
    ctx.accounts.config.require_keeper(&signer)?;
    let vault = &mut ctx.accounts.vault;
    vault.deposits_open = open;
    emit!(DepositsSet {
        vault: vault.key(),
        open,
        by: signer,
    });
    Ok(())
}
