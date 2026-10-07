//! A stand-in for Jupiter v6 in the vault's tests, loaded at Jupiter's address. The
//! snapshotted Jupiter routes swap over real pools at mainnet prices, so they only work while
//! Phoenix's mark sits at those prices; this swaps at whatever price the test quotes, so a
//! deposit or withdrawal can run at any mark.
//!
//! Its data has the layout the vault resizes: Jupiter's `shared_accounts_route` discriminator,
//! then `in_amount: u64, quoted_out_amount: u64, slippage_bps: u16, platform_fee_bps: u8`. It
//! takes `in_amount` from the trader and pays exactly `quoted_out_amount` from its reserve.
//!
//! Accounts: the trader (signer), source, destination, the reserve's account for the sold
//! mint, its account for the bought mint, the reserve authority (PDA `["reserve"]`), the sold
//! mint, the bought mint, and each mint's token program.

use solana_account_info::AccountInfo;
use solana_cpi::{invoke, invoke_signed};
use solana_instruction::{AccountMeta, Instruction};
use solana_program_error::{ProgramError, ProgramResult};
use solana_pubkey::Pubkey;

solana_program_entrypoint::entrypoint!(process);

pub const RESERVE_SEED: &[u8] = b"reserve";
const TAIL: usize = 19;

fn process(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let [trader, source, destination, reserve_in, reserve_out, reserve, sell_mint, buy_mint, sell_program, buy_program, ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if data.len() < 8 + TAIL {
        return Err(ProgramError::InvalidInstructionData);
    }
    let at = data.len() - TAIL;
    let field = |i: usize| u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
    let (in_amount, out_amount) = (field(at), field(at + 8));
    let (expected, bump) = Pubkey::find_program_address(&[RESERVE_SEED], program_id);
    if *reserve.key != expected {
        return Err(ProgramError::InvalidSeeds);
    }
    invoke(
        &transfer_checked(sell_program.key, source, sell_mint, reserve_in, trader, in_amount)?,
        &[
            source.clone(),
            sell_mint.clone(),
            reserve_in.clone(),
            trader.clone(),
            sell_program.clone(),
        ],
    )?;
    invoke_signed(
        &transfer_checked(buy_program.key, reserve_out, buy_mint, destination, reserve, out_amount)?,
        &[
            reserve_out.clone(),
            buy_mint.clone(),
            destination.clone(),
            reserve.clone(),
            buy_program.clone(),
        ],
        &[&[RESERVE_SEED, &[bump]]],
    )
}

/// SPL Token's (and Token-2022's) `transfer_checked`.
fn transfer_checked(
    program: &Pubkey,
    from: &AccountInfo,
    mint: &AccountInfo,
    to: &AccountInfo,
    authority: &AccountInfo,
    amount: u64,
) -> Result<Instruction, ProgramError> {
    let decimals = *mint
        .try_borrow_data()?
        .get(44)
        .ok_or(ProgramError::InvalidAccountData)?;
    let mut data = vec![12];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Ok(Instruction {
        program_id: *program,
        accounts: vec![
            AccountMeta::new(*from.key, false),
            AccountMeta::new_readonly(*mint.key, false),
            AccountMeta::new(*to.key, false),
            AccountMeta::new_readonly(*authority.key, true),
        ],
        data,
    })
}
