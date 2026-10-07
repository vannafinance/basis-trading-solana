//! Reference bytes the TypeScript client (`client/`) is checked against: instructions, PDAs,
//! Hawkeye views, an event and a vault account, built by the program's own Rust types. Writes
//! `client/test/vectors.json`:
//!
//! cargo test -p vanna_phoenix_vault --test vanna_phoenix_vault client_vectors -- --ignored

use crate::env::*;
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use anchor_lang::{Discriminator, Event, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use phoenix_rise_ix::hawkeye::{
    create_hawkeye_view_margin_for_asset_ix, create_hawkeye_view_margin_ix,
    HawkeyeTraderViewAccounts,
};
use serde_json::{json, Value};
use vanna_phoenix_vault::constants::*;
use vanna_phoenix_vault::events::Deposited;
use vanna_phoenix_vault::instructions::PhoenixProgramsAccounts;
use vanna_phoenix_vault::phoenix::*;
use vanna_phoenix_vault::state::{authority_address, BasisVault, VaultKind};
use vanna_phoenix_vault::swap::{Receive, SwapRoute};
use vanna_phoenix_vault::{accounts, instruction};

fn key(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}

fn metas(metas: &[AccountMeta]) -> Value {
    metas
        .iter()
        .map(|m| json!([m.pubkey.to_string(), m.is_signer, m.is_writable]))
        .collect()
}

fn core() -> accounts::VaultCore {
    accounts::VaultCore {
        config: Pubkey::find_program_address(&[CONFIG_SEED], &vanna_phoenix_vault::ID).0,
        basis_vault: key(2),
        authority: key(3),
        share_mint: key(4),
        usdc_mint: key(5),
        phoenix_mint: key(6),
        vault_usdc: key(7),
        vault_phoenix: key(8),
        vault_asset: Some(key(9)),
        asset_mint: key(10),
        asset_token_program: key(11),
        trader_account: key(12),
        phoenix: PhoenixProgramsAccounts {
            phoenix_program: PHOENIX_PROGRAM_ID,
            log_authority: PHOENIX_LOG_AUTHORITY,
            global_config: PHOENIX_GLOBAL_CONFIG,
            hawkeye_program: HAWKEYE_ID,
            ember_program: EMBER_PROGRAM_ID,
            ember_state: "6ur7v6AXNpnHeEb6xuk7PyezvZ1i5GrgYyWZkNCpzbRz"
                .parse()
                .unwrap(),
            ember_vault: "FKcEb4TdPDTRuMnQDpSEPQBcrm15S73xiUD6Qf8ZLUkq"
                .parse()
                .unwrap(),
        },
        perp_asset_map: key(20),
        orderbook: key(21),
        spline_collection: key(22),
        withdraw_queue: key(23),
        global_vault: key(24),
    }
}

#[test]
#[ignore]
fn client_vectors() {
    let token = anchor_spl::token::ID;
    let ata = anchor_spl::associated_token::ID;
    let system = anchor_lang::system_program::ID;
    let index = [
        AccountMeta::new(key(40), false),
        AccountMeta::new(key(41), false),
    ];
    // The user's accounts, as the client derives them: the share and USDC accounts under SPL
    // Token, the asset account under the vault's asset token program (here `key(11)`).
    let user = key(30);
    let user_shares = get_associated_token_address_with_program_id(&user, &key(4), &token);
    let user_asset = get_associated_token_address_with_program_id(&user, &key(10), &key(11));
    let user_usdc = get_associated_token_address_with_program_id(&user, &key(5), &token);

    let mut deposit = accounts::Deposit {
        user,
        core: core(),
        user_shares,
        user_asset: Some(user_asset),
        user_usdc: Some(user_usdc),
        swap_program: Some(JUPITER),
        vault_wsol: None,
        token_program: token,
        associated_token_program: ata,
        system_program: system,
    }
    .to_account_metas(None);
    deposit.extend(index.clone());
    let deposit_data = instruction::Deposit {
        gti: 1,
        atb: 1,
        asset_amount: 123_456,
        usdc_amount: 7_890_000,
        min_shares: 42,
        route: Some(SwapRoute {
            accounts: 3,
            data: vec![9, 8, 7],
        }),
    }
    .data();

    let mut withdraw = accounts::Withdraw {
        user,
        core: core(),
        user_shares,
        user_asset: Some(user_asset),
        user_usdc: Some(user_usdc),
        swap_program: None,
        vault_wsol: None,
        token_program: token,
        associated_token_program: ata,
        system_program: system,
    }
    .to_account_metas(None);
    withdraw.extend(index.clone());
    let withdraw_data = instruction::Withdraw {
        gti: 1,
        atb: 1,
        shares: 1_000_000,
        min_asset_out: 5,
        min_usdc_out: 6,
        receive: Receive::Usdc,
        route: None,
    }
    .data();

    let view = |trader| HawkeyeTraderViewAccounts {
        phoenix_program_id: PHOENIX_PROGRAM_ID,
        global_config: PHOENIX_GLOBAL_CONFIG,
        global_trader_index: vec![key(40)],
        active_trader_buffer: vec![key(41)],
        perp_asset_map: key(20),
        trader,
    };
    let margin: anchor_lang::solana_program::instruction::Instruction =
        create_hawkeye_view_margin_ix(view(key(12))).into();
    let asset: anchor_lang::solana_program::instruction::Instruction =
        create_hawkeye_view_margin_for_asset_ix(view(key(12)), 42).into();

    let basis_vault = Pubkey::find_program_address(
        &[BASIS_VAULT_SEED, ETH_MINT.as_ref()],
        &vanna_phoenix_vault::ID,
    )
    .0;
    let event = Deposited {
        basis_vault,
        user: key(30),
        asset_amount: 1,
        usdc_amount: 2,
        shares: 3,
        value_usd: 4,
        nav_before_usd: 5,
        position_lots: -6,
    };
    let vault = BasisVault {
        kind: VaultKind::Spot,
        asset_mint: ETH_MINT,
        asset_decimals: 8,
        asset_token_program: token,
        share_mint: key(4),
        authority: key(3),
        usdc_account: key(7),
        phoenix_account: key(8),
        asset_account: key(9),
        trader_account: key(12),
        asset_id: 2,
        orderbook: key(21),
        spline_collection: key(22),
        base_lot_decimals: 3,
        tick_size: 10,
        max_position_usd: 300_000_000_000,
        max_asset_units: u64::MAX,
        max_oi_share_bps: 2_500,
        max_deposit_usd: 250_000_000_000,
        slippage_bps: 100,
        keeper_band_bps: 20,
        max_rebalance_usd: 50_000_000_000,
        rebalance_window_start: 7,
        rebalance_window_usd: 8,
        margin_bps: 5_000,
        target_margin_bps: 10_000,
        deposits_open: true,
        after_hours_deposits: false,
        bump: 254,
        authority_nonce: 7u64.to_le_bytes(),
        authority_bump: 253,
    };
    let mut vault_bytes = BasisVault::DISCRIMINATOR.to_vec();
    anchor_lang::AnchorSerialize::serialize(&vault, &mut vault_bytes).unwrap();

    let authority = authority_address(&basis_vault, 7).0;
    let out = json!({
        "deposit": { "metas": metas(&deposit), "data": hex(&deposit_data) },
        "withdraw": { "metas": metas(&withdraw), "data": hex(&withdraw_data) },
        "view_margin": { "metas": metas(&margin.accounts), "data": hex(&margin.data) },
        "view_asset": { "metas": metas(&asset.accounts), "data": hex(&asset.data) },
        "pdas": {
            "config": Pubkey::find_program_address(&[CONFIG_SEED], &vanna_phoenix_vault::ID).0.to_string(),
            "eth_basis_vault": basis_vault.to_string(),
            "eth_authority_nonce_7": authority.to_string(),
            "eth_share_mint": Pubkey::find_program_address(&[SHARE_MINT_SEED, basis_vault.as_ref()], &vanna_phoenix_vault::ID).0.to_string(),
            "eth_trader_nonce_7": trader_account_address(&authority).to_string(),
        },
        "event_deposited": hex(&event.data()),
        "basis_vault_account": hex(&vault_bytes),
    });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../client/test/vectors.json"
    );
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    eprintln!("wrote {path}");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
