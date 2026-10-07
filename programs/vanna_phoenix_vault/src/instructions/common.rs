use crate::constants::*;
use crate::errors::VaultError;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::associated_token;
use anchor_spl::token::{self, Burn, MintTo, Transfer};
use anchor_spl::token_2022;

/// A USDC transfer (classic SPL token).
pub fn transfer<'info>(
    from: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    if amount == 0 || from.key == to.key {
        return Ok(());
    }
    let accounts = Transfer {
        from: from.clone(),
        to: to.clone(),
        authority: authority.clone(),
    };
    token::transfer(
        CpiContext::new_with_signer(token::ID, accounts, seeds),
        amount,
    )
}

/// A transfer of the vault's asset under its own token program (SPL Token or Token-2022),
/// naming the mint as Token-2022's extensions (pausable, transfer hook) require.
#[allow(clippy::too_many_arguments)]
pub fn transfer_checked<'info>(
    token_program: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    decimals: u8,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    if amount == 0 || from.key == to.key {
        return Ok(());
    }
    let mut data = Vec::with_capacity(10);
    data.push(12);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    let ix = Instruction {
        program_id: *token_program.key,
        accounts: vec![
            AccountMeta::new(*from.key, false),
            AccountMeta::new_readonly(*mint.key, false),
            AccountMeta::new(*to.key, false),
            AccountMeta::new_readonly(*authority.key, true),
        ],
        data,
    };
    invoke_signed(
        &ix,
        &[from.clone(), mint.clone(), to.clone(), authority.clone()],
        seeds,
    )?;
    Ok(())
}

pub fn mint_to<'info>(
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let accounts = MintTo {
        mint: mint.clone(),
        to: to.clone(),
        authority: authority.clone(),
    };
    token::mint_to(
        CpiContext::new_with_signer(token::ID, accounts, seeds),
        amount,
    )
}

pub fn burn<'info>(
    mint: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    amount: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let accounts = Burn {
        mint: mint.clone(),
        from: from.clone(),
        authority: authority.clone(),
    };
    token::burn(
        CpiContext::new_with_signer(token::ID, accounts, seeds),
        amount,
    )
}

pub fn create_ata_idempotent<'info>(
    payer: &AccountInfo<'info>,
    ata: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    let accounts = associated_token::Create {
        payer: payer.clone(),
        associated_token: ata.clone(),
        authority: authority.clone(),
        mint: mint.clone(),
        system_program: system_program.clone(),
        token_program: token_program.clone(),
    };
    associated_token::create_idempotent(CpiContext::new_with_signer(
        associated_token::ID,
        accounts,
        seeds,
    ))
}

fn is_token_program(key: &Pubkey) -> bool {
    *key == token::ID || *key == token_2022::ID
}

/// Reads a field of an SPL token account (either token program) given as a raw account.
fn token_field<T>(info: &AccountInfo, read: impl FnOnce(&[u8]) -> T) -> Result<T> {
    require!(
        is_token_program(info.owner),
        VaultError::InvalidTokenAccount
    );
    let data = info.try_borrow_data()?;
    require!(data.len() >= 109, VaultError::InvalidTokenAccount);
    Ok(read(&data))
}

/// Balance of an SPL token account given as a raw account.
pub fn token_amount(info: &AccountInfo) -> Result<u64> {
    token_field(info, |data| {
        u64::from_le_bytes(data[64..72].try_into().unwrap())
    })
}

// Token-2022 mint extensions: a type-length-value list after the 165-byte base and the
// account type byte.
const MINT_EXTENSIONS: usize = 166;
const EXT_TRANSFER_FEE: u16 = 1;
const EXT_DEFAULT_ACCOUNT_STATE: u16 = 6;
const EXT_NON_TRANSFERABLE: u16 = 9;
const EXT_INTEREST_BEARING: u16 = 10;
const EXT_TRANSFER_HOOK: u16 = 14;
const EXT_SCALED_UI_AMOUNT: u16 = 25;

/// The extensions of a Token-2022 mint as (type, value); none for a classic mint.
fn mint_extensions(data: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut at = if data.len() > MINT_EXTENSIONS {
        MINT_EXTENSIONS
    } else {
        data.len()
    };
    std::iter::from_fn(move || {
        let header = data.get(at..at + 4)?;
        let kind = u16::from_le_bytes([header[0], header[1]]);
        let len = u16::from_le_bytes([header[2], header[3]]) as usize;
        let value = data.get(at + 4..at + 4 + len)?;
        at += 4 + len;
        Some((kind, value))
    })
}

/// The decimals of an asset mint the vault can hold. Token-2022 mints may not charge
/// transfer fees, accrue interest, block transfers, run a transfer hook or freeze new
/// accounts: the vault's accounting assumes a transfer moves exactly what it says.
pub fn asset_mint_decimals(mint: &AccountInfo) -> Result<u8> {
    require!(is_token_program(mint.owner), VaultError::InvalidMint);
    let data = mint.try_borrow_data()?;
    require!(data.len() >= 82 && data[45] == 1, VaultError::InvalidMint);
    for (kind, value) in mint_extensions(&data) {
        let supported = match kind {
            EXT_TRANSFER_FEE | EXT_NON_TRANSFERABLE | EXT_INTEREST_BEARING => false,
            // Initialized (1), not frozen (2).
            EXT_DEFAULT_ACCOUNT_STATE => value.first() == Some(&1),
            // The hook's program id follows its authority: unset is all zeros.
            EXT_TRANSFER_HOOK => value.get(32..64).is_some_and(|p| p == [0; 32]),
            _ => true,
        };
        require!(supported, VaultError::UnsupportedMint);
    }
    Ok(data[44])
}

/// A token's scaled UI amount: (multiplier, new multiplier, when the new one starts).
fn scaled_ui_amount(mint: &AccountInfo) -> Result<Option<(f64, f64, i64)>> {
    let data = mint.try_borrow_data()?;
    let Some((_, value)) = mint_extensions(&data).find(|(k, _)| *k == EXT_SCALED_UI_AMOUNT) else {
        return Ok(None);
    };
    require!(value.len() >= 56, VaultError::InvalidMint);
    // Authority (32), multiplier (f64), the new multiplier's start (i64), new multiplier (f64).
    let read_f64 = |at: usize| f64::from_le_bytes(value[at..at + 8].try_into().unwrap());
    let starts = i64::from_le_bytes(value[40..48].try_into().unwrap());
    for multiplier in [read_f64(32), read_f64(48)] {
        require!(
            multiplier.is_finite() && multiplier > 0.0 && multiplier < 1_000.0,
            VaultError::InvalidMint
        );
    }
    Ok(Some((read_f64(32), read_f64(48), starts)))
}

/// How many of the market's units one token is worth, over `MULTIPLIER_SCALE`: Token-2022's
/// scaled UI amount (tokenized stocks reinvest dividends through it), or 1 for other tokens.
pub fn ui_multiplier(mint: &AccountInfo, now: i64) -> Result<u128> {
    Ok(match scaled_ui_amount(mint)? {
        None => MULTIPLIER_SCALE,
        Some((old, new, starts)) => {
            let multiplier = if now >= starts { new } else { old };
            (multiplier * MULTIPLIER_SCALE as f64) as u128
        }
    })
}

/// Whether the token's multiplier is in the middle of a large change (a special dividend, a
/// split): more than `MULTIPLIER_JUMP_BPS`, within a day before to three days after it takes
/// effect. A regular dividend's step (a few bps) is smaller than a deposit's own costs.
pub fn multiplier_changing(mint: &AccountInfo, now: i64) -> Result<bool> {
    let Some((old, new, starts)) = scaled_ui_amount(mint)? else {
        return Ok(false);
    };
    let jump_bps = ((new / old - 1.0).abs() * BPS_SCALE as f64) as u128;
    Ok(jump_bps > MULTIPLIER_JUMP_BPS
        && now >= starts.saturating_sub(MULTIPLIER_WINDOW_BEFORE)
        && now <= starts.saturating_add(MULTIPLIER_WINDOW_AFTER))
}

/// Closes a token account (unwrapping wrapped SOL), paying its lamports to `destination`.
pub fn close_account<'info>(
    account: &AccountInfo<'info>,
    destination: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    let accounts = token::CloseAccount {
        account: account.clone(),
        destination: destination.clone(),
        authority: authority.clone(),
    };
    token::close_account(CpiContext::new_with_signer(token::ID, accounts, seeds))
}

/// Counts the lamports sent to a wrapped-SOL account as its token balance.
pub fn sync_native<'info>(account: &AccountInfo<'info>) -> Result<()> {
    let accounts = token::SyncNative {
        account: account.clone(),
    };
    token::sync_native(CpiContext::new(token::ID, accounts))
}

/// Moves lamports out of a system account, signed by `seeds` when it is a PDA.
pub fn system_transfer<'info>(
    from: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    lamports: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    if lamports == 0 {
        return Ok(());
    }
    let accounts = anchor_lang::system_program::Transfer {
        from: from.clone(),
        to: to.clone(),
    };
    anchor_lang::system_program::transfer(
        CpiContext::new_with_signer(anchor_lang::system_program::ID, accounts, seeds),
        lamports,
    )
}
