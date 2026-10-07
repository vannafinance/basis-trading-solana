//! Test harness: the real Phoenix, Ember and Hawkeye programs (mainnet BPF) in Phoenix's own
//! LiteSVM fixture, with the SOL, ETH, BTC, TSLA and META vaults created, onboarded and quoted
//! around.
//!
//! The vaults use the mainnet USDC, Wormhole ETH and cbBTC addresses (minted freely here) and
//! the mainnet TSLAx and METAx mints (Token-2022, with their real extensions and dividend
//! multiplier), so `use_jupiter` can load real Jupiter routes over real Orca and Raydium pools
//! next to them. Phoenix's fixture lists only BTC, ETH and SOL; the stock markets are added
//! with mainnet's lot and tick sizes.
//!
//! Every deposit and withdrawal swaps through Jupiter's address. Until `use_jupiter` loads the
//! real program there, a mock (`tests/mock_jupiter`) sits at it and swaps at Phoenix's mark,
//! so tests can move prices freely; the real routes only work at the snapshot's prices.

use anchor_lang::prelude::{pubkey, Clock, Pubkey};
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::{
    get_associated_token_address, get_associated_token_address_with_program_id,
};
use litesvm::types::TransactionMetadata;
use phoenix_rise_ix::cancel_orders::{create_cancel_all_ix, CancelAllParams};
use phoenix_rise_ix::hawkeye::{
    create_hawkeye_view_margin_for_asset_ix, create_hawkeye_view_margin_ix, decode_hawkeye_return,
    HawkeyeTraderViewAccounts, ViewAssetReturn, ViewMarginReturn,
};
use phoenix_rise_ix::limit_order::{create_place_limit_order_ix, LimitOrderParams};
use phoenix_rise_ix::types::Side;
use phoenix_rise_litesvm_test::{
    decode_fixture_instruction, default_sdk_localnet_fixture, find_sdk_localnet_program_paths,
    parse_pubkey, parse_pubkeys, sdk_localnet_seed_bytes, FixtureLiquidity, FixtureMarket,
    FixtureOraclePriceUpdate, FixturePrice, FixtureSigner, SdkLocalnetContext, SdkLocalnetFixture,
    SdkLocalnetProgram, PHOENIX_MAINNET_BPF_PROGRAMS_ENV,
};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use std::collections::HashMap;
use vanna_phoenix_vault::constants::*;
use vanna_phoenix_vault::instructions::{MarketParams, VaultParams};
use vanna_phoenix_vault::math::Price;
use vanna_phoenix_vault::phoenix::{
    trader_account_address, EMBER_PROGRAM_ID, HAWKEYE_ID, PHOENIX_GLOBAL_CONFIG,
    PHOENIX_LOG_AUTHORITY, PHOENIX_PROGRAM_ID,
};
use vanna_phoenix_vault::state::{authority_address, BasisVault, VaultKind};
use vanna_phoenix_vault::swap::{deposit_swap_amount, Sell, SwapRoute};
use vanna_phoenix_vault::{accounts, instruction};

pub const SOL: u64 = 1_000_000_000;
pub const USDC: u64 = 1_000_000;
/// Wormhole ETH and BTC on Solana have 8 decimals.
pub const SPOT_DECIMALS: u8 = 8;
pub const ETH: u64 = 100_000_000;
pub const BTC: u64 = 100_000_000;

pub const USDC_MINT: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
/// Wormhole ETH.
pub const ETH_MINT: Pubkey = pubkey!("7vfCXTUXx5WJV5JADk17DUJ4ksgau7utNKj4b963voxs");
pub const BTC_MINT: Pubkey = pubkey!("cbbtcf3aa214zXHbiAZQwf4122FBYbraNdFqgw4iMij");
/// Backed's tokenized stocks (xStocks): Token-2022, 8 decimals.
pub const TSLA_MINT: Pubkey = pubkey!("XsDoVfqeBukxuZHWhdvWHBhgEHjGNst4MLodqsJHzoB");
pub const META_MINT: Pubkey = pubkey!("Xsa62P5mvPszXL1krVUnU5ar38bBSVcWAB6fmPCo5Zu");
/// One xStock token.
pub const STOCK: u64 = 100_000_000;
pub const JUPITER: Pubkey = pubkey!("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4");

const ADMIN: &str = "vanna-admin";
const KEEPER: &str = "vanna-keeper";
const MAKER: &str = "orderbookTrader";
const TOKEN_PROGRAM: Pubkey = anchor_spl::token::ID;
pub const TOKEN_2022: Pubkey = anchor_spl::token_2022::ID;
const ATA_PROGRAM: Pubkey = anchor_spl::associated_token::ID;
const SYSTEM_PROGRAM: Pubkey = anchor_lang::system_program::ID;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asset {
    Sol,
    Eth,
    Btc,
    Tsla,
    Meta,
}

impl Asset {
    pub const ALL: [Asset; 5] = [Asset::Sol, Asset::Eth, Asset::Btc, Asset::Tsla, Asset::Meta];

    pub fn symbol(self) -> &'static str {
        match self {
            Asset::Sol => "SOL",
            Asset::Eth => "ETH",
            Asset::Btc => "BTC",
            Asset::Tsla => "TSLA",
            Asset::Meta => "META",
        }
    }

    pub fn start_price(self) -> f64 {
        match self {
            Asset::Sol => 150.0,
            Asset::Eth => 3_500.0,
            Asset::Btc => 100_000.0,
            Asset::Tsla => 382.0,
            Asset::Meta => 746.0,
        }
    }

    pub fn is_stock(self) -> bool {
        matches!(self, Asset::Tsla | Asset::Meta)
    }

    pub fn token_program(self) -> Pubkey {
        if self.is_stock() {
            TOKEN_2022
        } else {
            TOKEN_PROGRAM
        }
    }

    /// Base lots the maker quotes on each side.
    fn quote_lots(self) -> u64 {
        match self {
            Asset::Sol => 200_000,
            Asset::Eth => 200_000,
            Asset::Btc => 100_000,
            // 2,000 shares: about what Phoenix's TSLA and META books hold within 0.5%.
            Asset::Tsla | Asset::Meta => 2_000_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Vault {
    pub kind: VaultKind,
    /// The authority's nonce.
    pub nonce: u64,
    pub asset_mint: Pubkey,
    pub decimals: u8,
    pub token_program: Pubkey,
    pub basis_vault: Pubkey,
    pub authority: Pubkey,
    pub share_mint: Pubkey,
    pub trader: Pubkey,
    pub vault_usdc: Pubkey,
    pub vault_phoenix: Pubkey,
    pub vault_asset: Option<Pubkey>,
    pub orderbook: Pubkey,
    pub spline: Pubkey,
    pub asset_id: u32,
    pub base_lot_decimals: u8,
    pub tick_size: u64,
}

/// What the vault owns, read the way the program reads it.
#[derive(Clone, Copy, Debug)]
pub struct VaultView {
    /// Phoenix's mark: per share for a stock.
    pub mark_usd: f64,
    pub position_lots: i64,
    pub asset_units: u64,
    pub collateral_usd: f64,
    pub upnl_usd: f64,
    pub funding_usd: f64,
    pub effective_usd: f64,
    pub nav_usd: f64,
    pub supply: u64,
}

impl VaultView {
    pub fn share_price(&self) -> f64 {
        if self.supply == 0 {
            1.0
        } else {
            self.nav_usd / (self.supply as f64 / USDC as f64)
        }
    }
}

pub struct Env {
    pub ctx: SdkLocalnetContext,
    pub gti: Vec<Pubkey>,
    pub atb: Vec<Pubkey>,
    pub admin: Pubkey,
    pub keeper: Pubkey,
    pub config: Pubkey,
    pub usdc_mint: Pubkey,
    pub phoenix_mint: Pubkey,
    pub perp_asset_map: Pubkey,
    pub withdraw_queue: Pubkey,
    pub global_vault: Pubkey,
    pub ember_state: Pubkey,
    pub ember_vault: Pubkey,
    pub sol: Vault,
    pub eth: Vault,
    pub btc: Vault,
    pub tsla: Vault,
    pub meta: Vault,
    /// Last perp and spot price sent to Phoenix's oracle, per market.
    oracle: HashMap<&'static str, (f64, f64)>,
    /// The Jupiter routes and mainnet accounts, once `use_jupiter` has loaded them.
    jupiter: Option<serde_json::Value>,
}

/// Test vaults start without caps, so tests can size deposits freely; `mainnet_params` has the
/// caps the vaults launch with.
pub fn default_params() -> VaultParams {
    VaultParams {
        max_position_usd: 50_000_000 * USDC,
        max_asset_units: u64::MAX,
        max_oi_share_bps: 10_000,
        max_deposit_usd: 50_000_000 * USDC,
        slippage_bps: 100,
        keeper_band_bps: 100,
        max_rebalance_usd: 50_000_000 * USDC,
        margin_bps: 5_000,
        target_margin_bps: 10_000,
        deposits_open: true,
        after_hours_deposits: true,
    }
}

/// The SOL vault's SOL is its own margin: all of a deposit buys SOL.
pub fn sol_params() -> VaultParams {
    VaultParams {
        target_margin_bps: 0,
        ..default_params()
    }
}

/// The caps each vault launches with, from Phoenix's mainnet markets on 2026-10-06 (see the
/// README): the short stays at most a quarter of the market's open interest, one deposit at
/// most what the book takes within about 0.5%, and the SOL vault under the 500 SOL of native
/// collateral Phoenix counts per trader. Stock vaults close deposits after hours. The keeper
/// trades within 0.2% of the mark and at most $50,000 a day (reinvested funding is a few
/// hundred dollars a day at these sizes).
pub fn mainnet_params(asset: Asset) -> VaultParams {
    let (max_position, max_deposit) = match asset {
        Asset::Sol => (100_000, 100_000),
        Asset::Eth => (300_000, 250_000),
        Asset::Btc => (750_000, 250_000),
        Asset::Tsla => (300_000, 100_000),
        Asset::Meta => (200_000, 50_000),
    };
    VaultParams {
        max_position_usd: max_position * USDC,
        max_asset_units: if asset == Asset::Sol {
            450 * SOL
        } else {
            u64::MAX
        },
        max_oi_share_bps: 2_500,
        max_deposit_usd: max_deposit * USDC,
        keeper_band_bps: 20,
        max_rebalance_usd: 50_000 * USDC,
        after_hours_deposits: !asset.is_stock(),
        ..if asset == Asset::Sol {
            sol_params()
        } else {
            default_params()
        }
    }
}

impl Env {
    /// The vaults created, onboarded and quoted around.
    pub fn new() -> Self {
        let mut env = Self::bare();
        env.initialize(ADMIN)
            .unwrap_or_else(|l| panic!("{}", l.join("\n")));
        for asset in Asset::ALL {
            let params = if asset == Asset::Sol {
                sol_params()
            } else {
                default_params()
            };
            if asset.is_stock() {
                // The stock markets get their first oracle and spline prices here.
                env.move_price(asset, asset.start_price());
            }
            env.create_vault(asset, params);
            env.onboard(asset);
            env.quote(asset, asset.start_price());
        }
        env
    }

    /// Phoenix, the mints and the program deployed with the admin as its upgrade authority;
    /// nothing initialized yet.
    pub fn bare() -> Self {
        // Edition 2021: tells the fixture to fetch Phoenix, Ember and Hawkeye from mainnet.
        std::env::set_var(PHOENIX_MAINNET_BPF_PROGRAMS_ENV, "1");
        let paths = find_sdk_localnet_program_paths().expect("Phoenix protocol programs");
        let so = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/deploy/vanna_phoenix_vault.so"
        );
        let mut fixture = default_sdk_localnet_fixture().expect("fixture");
        use_mainnet_usdc(&mut fixture);
        add_stock_market(&mut fixture, "TSLA");
        add_stock_market(&mut fixture, "META");
        let mock_jupiter = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/deploy/mock_jupiter.so"
        );
        let mut ctx = SdkLocalnetContext::new_with_programs(
            fixture,
            paths,
            [
                SdkLocalnetProgram::new(vanna_phoenix_vault::ID, so),
                SdkLocalnetProgram::new(JUPITER, mock_jupiter),
            ],
        );
        // Real time, from when the Jupiter routes were snapshotted: Orca's pools reject a clock
        // older than their last update.
        let mut clock: Clock = ctx.svm.get_sysvar();
        clock.unix_timestamp = jupiter_snapshot()["fetched_at"].as_i64().unwrap() + 60;
        ctx.svm.set_sysvar(&clock);
        let payer = ctx.signer_pubkey("payer");
        create_mint(&mut ctx, USDC_MINT, payer, 6);
        for transaction in ctx.fixture.setup_transactions.clone() {
            if transaction.name != "createFakeUsdcMint" {
                ctx.send_fixture_transaction(&transaction.name);
            }
        }
        ctx.svm.warp_to_slot(200);
        for name in [
            "oracleSetPrices",
            "splineUpdatePrices",
            "orderbookPlaceLevels",
        ] {
            ctx.send_fixture_transaction(name);
        }
        enable_native_sol(&mut ctx);
        // Mainnet has no deposit cooldown (read on 2026-10-05); the fixture sets 150 slots.
        set_deposit_cooldown(&mut ctx, 0);

        let addresses = ctx.fixture.addresses.clone();
        let admin = ctx.add_signer(ADMIN, 1_000 * SOL);
        let keeper = ctx.add_signer(KEEPER, 10_000 * SOL);
        let config = Pubkey::find_program_address(&[CONFIG_SEED], &vanna_phoenix_vault::ID).0;
        create_native_mint(&mut ctx);
        create_mint(&mut ctx, ETH_MINT, admin, SPOT_DECIMALS);
        create_mint(&mut ctx, BTC_MINT, admin, SPOT_DECIMALS);
        for mint in [TSLA_MINT, META_MINT] {
            create_mainnet_mint(&mut ctx, mint, admin);
        }
        let mut env = Env {
            gti: parse_pubkeys(&addresses.global_trader_index),
            atb: parse_pubkeys(&addresses.active_trader_buffer),
            usdc_mint: ctx.fake_usdc_mint(),
            phoenix_mint: ctx.phoenix_collateral_mint(),
            perp_asset_map: parse_pubkey(&addresses.perp_asset_map).unwrap(),
            withdraw_queue: parse_pubkey(&addresses.withdraw_queue).unwrap(),
            global_vault: parse_pubkey(&addresses.global_vault).unwrap(),
            ember_state: parse_pubkey(&addresses.ember_state).unwrap(),
            ember_vault: parse_pubkey(&addresses.ember_vault).unwrap(),
            sol: vault_keys(&ctx, Asset::Sol, VaultKind::NativeSol, NATIVE_MINT, 9, 0),
            eth: vault_keys(
                &ctx,
                Asset::Eth,
                VaultKind::Spot,
                ETH_MINT,
                SPOT_DECIMALS,
                0,
            ),
            btc: vault_keys(
                &ctx,
                Asset::Btc,
                VaultKind::Spot,
                BTC_MINT,
                SPOT_DECIMALS,
                0,
            ),
            tsla: vault_keys(&ctx, Asset::Tsla, VaultKind::Spot, TSLA_MINT, 8, 0),
            meta: vault_keys(&ctx, Asset::Meta, VaultKind::Spot, META_MINT, 8, 0),
            jupiter: None,
            ctx,
            admin,
            keeper,
            config,
            oracle: Asset::ALL
                .into_iter()
                .map(|a| (a.symbol(), (a.start_price(), a.start_price())))
                .collect(),
        };

        set_upgrade_authority(&mut env.ctx, admin);
        env.fund_mock_reserve();
        env
    }

    /// The mock Jupiter's reserve: plenty of every token it pays out.
    fn fund_mock_reserve(&mut self) {
        let reserve = mock_reserve();
        for mint in [USDC_MINT, NATIVE_MINT, ETH_MINT, BTC_MINT, TSLA_MINT, META_MINT] {
            self.create_ata(ADMIN, reserve, mint);
        }
        self.mint_usdc(reserve, 1_000_000_000 * USDC);
        for asset in [Asset::Eth, Asset::Btc, Asset::Tsla, Asset::Meta] {
            let units = 10u64.pow(self.vault(asset).decimals as u32);
            self.mint_asset(asset, reserve, 1_000_000 * units);
        }
        let wsol = Self::ata(&reserve, &NATIVE_MINT);
        self.ctx.svm.airdrop(&wsol, 100_000 * SOL).unwrap();
        let sync = Instruction {
            program_id: TOKEN_PROGRAM,
            accounts: vec![AccountMeta::new(wsol, false)],
            data: vec![17],
        };
        self.ctx.send_instructions(vec![sync], ADMIN, "sync reserve wSOL");
    }

    /// `initialize`, signed by `signer`: the admin, the keeper and Jupiter as the swap venue.
    pub fn initialize(&mut self, signer: &str) -> Result<TransactionMetadata, Vec<String>> {
        let ix = self.ix(
            accounts::Initialize {
                admin: self.ctx.signer_pubkey(signer),
                program: vanna_phoenix_vault::ID,
                program_data: program_data_address(),
                config: self.config,
                usdc_mint: self.usdc_mint,
                phoenix_mint: self.phoenix_mint,
                perp_asset_map: self.perp_asset_map,
                withdraw_queue: self.withdraw_queue,
                global_vault: self.global_vault,
                system_program: SYSTEM_PROGRAM,
            },
            instruction::Initialize {
                keeper: self.keeper,
                swap_program: JUPITER,
            },
        );
        self.send(signer, vec![ix], "initialize")
    }

    pub fn admin_seed(&self) -> &'static str {
        ADMIN
    }

    /// `set_config` keeping everything but the swap venue.
    pub fn set_config_swap_program(
        &mut self,
        swap_program: Pubkey,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let ix = self.ix(
            accounts::AdminOnly {
                admin: self.admin,
                config: self.config,
            },
            instruction::SetConfig {
                admin: self.admin,
                keeper: self.keeper,
                swap_program,
                paused: false,
            },
        );
        self.send(ADMIN, vec![ix], "set config")
    }

    /// Loads the real Jupiter v6 and Orca Whirlpool programs and every mainnet account the
    /// snapshotted routes touch (`scripts/jupiter_fork.py`), and moves Phoenix's marks between
    /// the routes' buy and sell prices, as on mainnet where Phoenix and the pools price the
    /// same assets. The mints stay the test's own, so tests can still mint them.
    pub fn use_jupiter(&mut self) {
        use base64::Engine;
        let snapshot = jupiter_snapshot();
        let cache = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../target/deploy/.cache/mainnet"
        );
        for program in snapshot["programs"].as_array().unwrap() {
            let id = program.as_str().unwrap();
            let path = std::path::PathBuf::from(format!("{cache}/{id}.so"));
            assert!(
                path.exists(),
                "{} is missing: run scripts/jupiter_fork.py --programs-only",
                path.display()
            );
            self.ctx.load_program_from_path(&id.parse().unwrap(), &path);
        }
        let own = [
            USDC_MINT,
            ETH_MINT,
            BTC_MINT,
            NATIVE_MINT,
            TSLA_MINT,
            META_MINT,
        ];
        for (key, account) in snapshot["accounts"].as_object().unwrap() {
            let key: Pubkey = key.parse().unwrap();
            if own.contains(&key) {
                continue;
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(account["data"].as_str().unwrap())
                .unwrap();
            let account = solana_account::Account {
                lamports: account["lamports"].as_u64().unwrap(),
                data,
                owner: account["owner"].as_str().unwrap().parse().unwrap(),
                executable: false,
                rent_epoch: 0,
            };
            self.ctx.svm.set_account(key, account).unwrap();
        }
        self.jupiter = Some(snapshot);
        for asset in Asset::ALL {
            // The pools price a token; Phoenix prices the market unit (a share), which a
            // dividend-reinvesting stock token is worth more than.
            let mid = (self.jupiter_price(asset, Sell::Usdc)
                + self.jupiter_price(asset, Sell::Asset))
                / 2.0
                / self.multiplier(asset);
            self.move_price(asset, (mid * 1_000.0).round() / 1_000.0);
        }
    }

    /// Offset of a Token-2022 mint extension's value in the asset mint's data.
    fn mint_extension(&self, asset: Asset, kind: u16) -> Option<usize> {
        let data = self.ctx.account_data(&self.vault(asset).asset_mint);
        let mut at = 166;
        while data.len() >= at + 4 {
            let len = u16::from_le_bytes([data[at + 2], data[at + 3]]) as usize;
            if u16::from_le_bytes([data[at], data[at + 1]]) == kind {
                return Some(at + 4);
            }
            at += 4 + len;
        }
        None
    }

    /// Market units (shares) per token: Token-2022's scaled UI multiplier, as the program reads
    /// it; 1 for other tokens.
    pub fn multiplier(&self, asset: Asset) -> f64 {
        let Some(at) = self.mint_extension(asset, 25) else {
            return 1.0;
        };
        let data = self.ctx.account_data(&self.vault(asset).asset_mint);
        let now = self.ctx.svm.get_sysvar::<Clock>().unix_timestamp;
        let f = |i: usize| f64::from_le_bytes(data[at + i..at + i + 8].try_into().unwrap());
        let starts = i64::from_le_bytes(data[at + 40..at + 48].try_into().unwrap());
        if now >= starts {
            f(48)
        } else {
            f(32)
        }
    }

    /// Sets a stock token's multiplier and the next one, starting at `starts`, the way its
    /// issuer can.
    pub fn set_multiplier(&mut self, asset: Asset, multiplier: f64, next: f64, starts: i64) {
        let mint = self.vault(asset).asset_mint;
        let at = self
            .mint_extension(asset, 25)
            .expect("scaled UI amount mint");
        let mut account = self.ctx.svm.get_account(&mint).unwrap();
        account.data[at + 32..at + 40].copy_from_slice(&multiplier.to_le_bytes());
        account.data[at + 40..at + 48].copy_from_slice(&starts.to_le_bytes());
        account.data[at + 48..at + 56].copy_from_slice(&next.to_le_bytes());
        self.ctx.svm.set_account(mint, account).unwrap();
    }

    /// Pauses or unpauses a stock token the way its issuer can (Token-2022's pausable
    /// extension: authority, then the paused flag).
    pub fn set_token_paused(&mut self, asset: Asset, paused: bool) {
        let mint = self.vault(asset).asset_mint;
        let at = self.mint_extension(asset, 26).expect("pausable mint");
        let mut account = self.ctx.svm.get_account(&mint).unwrap();
        account.data[at + 32] = paused as u8;
        self.ctx.svm.set_account(mint, account).unwrap();
    }

    /// A snapshotted Jupiter route: its instruction, built by Jupiter's API for the vault
    /// authority, and Jupiter's quote.
    pub fn jupiter_route(&self, name: &str) -> JupiterRoute {
        use base64::Engine;
        let route = &self.jupiter.as_ref().expect("use_jupiter first")["routes"][name];
        let authority: Pubkey = route["authority"].as_str().unwrap().parse().unwrap();
        let metas: Vec<AccountMeta> = route["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                let pubkey: Pubkey = m["pubkey"].as_str().unwrap().parse().unwrap();
                AccountMeta {
                    pubkey,
                    // The vault signs for its authority inside the program.
                    is_signer: m["isSigner"].as_bool().unwrap() && pubkey != authority,
                    is_writable: m["isWritable"].as_bool().unwrap(),
                }
            })
            .collect();
        let data = base64::engine::general_purpose::STANDARD
            .decode(route["data"].as_str().unwrap())
            .unwrap();
        JupiterRoute {
            route: Route {
                args: SwapRoute {
                    accounts: metas.len() as u8,
                    data,
                },
                metas,
            },
            authority,
            in_amount: route["in_amount"].as_u64().unwrap(),
            quoted_out: route["quoted_out_amount"].as_u64().unwrap(),
        }
    }

    /// The Jupiter route of `asset`'s vault that sells `sell`, resized to `amount_in`. A
    /// Jupiter route's data ends with its input amount, quoted output, slippage and fee; this
    /// is what Jupiter's API returns when asked again for a new amount over the same pool.
    pub fn jupiter_route_for(&self, asset: Asset, sell: Sell, amount_in: u64) -> JupiterRoute {
        let mut route = self.jupiter_route(route_name(asset, sell));
        assert_eq!(
            route.authority,
            self.vault(asset).authority,
            "a route for this vault"
        );
        let quoted =
            (route.quoted_out as u128 * amount_in as u128 / route.in_amount as u128) as u64;
        let data = &mut route.route.args.data;
        let n = data.len();
        data[n - 19..n - 11].copy_from_slice(&amount_in.to_le_bytes());
        data[n - 11..n - 3].copy_from_slice(&quoted.to_le_bytes());
        route.in_amount = amount_in;
        route.quoted_out = quoted;
        route
    }

    /// USD per whole asset the snapshotted route pays (buying) or gets (selling), at its quote.
    pub fn jupiter_price(&self, asset: Asset, sell: Sell) -> f64 {
        let route = self.jupiter_route(route_name(asset, sell));
        let units = 10f64.powi(self.vault(asset).decimals as i32);
        let (usdc, amount) = match sell {
            Sell::Usdc => (route.in_amount, route.quoted_out),
            Sell::Asset => (route.quoted_out, route.in_amount),
        };
        usdc as f64 / USDC as f64 / (amount as f64 / units)
    }

    /// What the vault prices one token at: Phoenix's mark times the token's multiplier.
    pub fn mark_price(&mut self, asset: Asset) -> Price {
        let v = self.vault(asset).clone();
        let ticks = self.view_asset(asset).mark_price_ticks;
        let per_share = ticks * v.tick_size * 10u64.pow(v.base_lot_decimals as u32);
        Price {
            value: (per_share as f64 * self.multiplier(asset)) as u64,
            expo: -6,
        }
    }

    /// A route selling `amount` of `sell` at Phoenix's mark, as the app asks Jupiter for one:
    /// the snapshotted Jupiter route once `use_jupiter` has loaded it, the mock's otherwise.
    pub fn route_for(&mut self, asset: Asset, sell: Sell, amount: u64) -> Route {
        if self.jupiter.is_some() {
            return self.jupiter_route_for(asset, sell, amount).route;
        }
        let price = self.mark_price(asset).value as u128;
        let units = 10u128.pow(self.vault(asset).decimals as u32);
        let out = match sell {
            Sell::Usdc => amount as u128 * units / price,
            Sell::Asset => amount as u128 * price / units,
        };
        self.mock_route(asset, sell, amount, out as u64)
    }

    /// A mock Jupiter route for `asset`'s vault: it takes `in_amount` of `sell` from the vault
    /// and pays `quoted_out` of the other side, the quote scaled with the amount the vault
    /// resizes it to.
    pub fn mock_route(&self, asset: Asset, sell: Sell, in_amount: u64, quoted_out: u64) -> Route {
        let v = self.vault(asset);
        let reserve = mock_reserve();
        let (asset_account, asset_mint, asset_program) = match v.vault_asset {
            Some(account) => (account, v.asset_mint, v.token_program),
            None => (
                get_associated_token_address(&v.authority, &NATIVE_MINT),
                NATIVE_MINT,
                TOKEN_PROGRAM,
            ),
        };
        let usdc = (v.vault_usdc, self.usdc_mint, TOKEN_PROGRAM);
        let asset_side = (asset_account, asset_mint, asset_program);
        let (sold, bought) = match sell {
            Sell::Usdc => (usdc, asset_side),
            Sell::Asset => (asset_side, usdc),
        };
        let metas = vec![
            AccountMeta::new_readonly(v.authority, false),
            AccountMeta::new(sold.0, false),
            AccountMeta::new(bought.0, false),
            AccountMeta::new(Self::ata(&reserve, &sold.1), false),
            AccountMeta::new(Self::ata(&reserve, &bought.1), false),
            AccountMeta::new_readonly(reserve, false),
            AccountMeta::new_readonly(sold.1, false),
            AccountMeta::new_readonly(bought.1, false),
            AccountMeta::new_readonly(sold.2, false),
            AccountMeta::new_readonly(bought.2, false),
        ];
        // Jupiter's `shared_accounts_route`, cut to the fields the vault and the mock read.
        let mut data = vec![193, 32, 155, 51, 65, 214, 156, 129];
        data.extend_from_slice(&in_amount.to_le_bytes());
        data.extend_from_slice(&quoted_out.to_le_bytes());
        data.extend_from_slice(&50u16.to_le_bytes());
        data.push(0);
        Route {
            args: SwapRoute {
                accounts: metas.len() as u8,
                data,
            },
            metas,
        }
    }

    /// The asset a withdrawal of `shares` will sell, about: a spot vault's part of its tokens;
    /// for the SOL vault, the payout less the trade's cost in SOL. The vault resizes the route
    /// to the exact amount.
    pub fn withdraw_estimate(&mut self, asset: Asset, shares: u64) -> u64 {
        let v = self.vault(asset).clone();
        let view = self.view(asset);
        let part = shares as f64 / view.supply as f64;
        match v.kind {
            VaultKind::Spot => (view.asset_units as f64 * part) as u64,
            VaultKind::NativeSol => {
                let units = 10f64.powi(v.decimals as i32);
                (view.nav_usd * part * 0.999 / view.mark_usd * units) as u64
            }
        }
    }

    pub fn vault(&self, asset: Asset) -> &Vault {
        match asset {
            Asset::Sol => &self.sol,
            Asset::Eth => &self.eth,
            Asset::Btc => &self.btc,
            Asset::Tsla => &self.tsla,
            Asset::Meta => &self.meta,
        }
    }

    fn ix(&self, accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
        Instruction {
            program_id: vanna_phoenix_vault::ID,
            accounts: accounts.to_account_metas(None),
            data: data.data(),
        }
    }

    fn with_index(&self, mut ix: Instruction) -> Instruction {
        ix.accounts.extend(
            self.gti
                .iter()
                .chain(&self.atb)
                .map(|k| AccountMeta::new(*k, false)),
        );
        ix
    }

    pub fn send(
        &mut self,
        payer: &str,
        ixs: Vec<Instruction>,
        _label: &str,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        all.extend(ixs);
        self.ctx
            .try_send_instructions_with_metadata(all, payer)
            .map_err(|failed| failed.meta.logs)
    }

    fn create_vault(&mut self, asset: Asset, params: VaultParams) {
        let v = self.vault(asset).clone();
        self.try_create_vault(&v, params).unwrap_or_else(|logs| {
            panic!(
                "create {} vault failed:\n{}",
                asset.symbol(),
                logs.join("\n")
            )
        });
    }

    /// `create_vault` for the vault `v` describes, with its market as `v` gives it.
    pub fn try_create_vault(
        &mut self,
        v: &Vault,
        params: VaultParams,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let ix = self.ix(
            accounts::CreateVault {
                admin: self.admin,
                config: self.config,
                asset_mint: v.asset_mint,
                basis_vault: v.basis_vault,
                authority: v.authority,
                share_mint: v.share_mint,
                usdc_mint: self.usdc_mint,
                phoenix_mint: self.phoenix_mint,
                vault_usdc: v.vault_usdc,
                vault_phoenix: v.vault_phoenix,
                vault_asset: v.vault_asset,
                trader_account: v.trader,
                phoenix_program: PHOENIX_PROGRAM_ID,
                log_authority: PHOENIX_LOG_AUTHORITY,
                global_config: PHOENIX_GLOBAL_CONFIG,
                orderbook: v.orderbook,
                spline_collection: v.spline,
                perp_asset_map: self.perp_asset_map,
                token_program: TOKEN_PROGRAM,
                asset_token_program: v.token_program,
                associated_token_program: ATA_PROGRAM,
                system_program: SYSTEM_PROGRAM,
            },
            instruction::CreateVault {
                kind: v.kind,
                market: MarketParams {
                    asset_id: v.asset_id,
                    base_lot_decimals: v.base_lot_decimals,
                    tick_size: v.tick_size,
                },
                params,
                authority_nonce: v.nonce,
            },
        );
        self.send(ADMIN, vec![ix], "create vault")
    }

    /// A new classic SPL mint the admin can mint, for vaults beyond the five.
    pub fn new_spot_mint(&mut self, seed: &str, decimals: u8) -> Pubkey {
        let mint = self.ctx.add_signer(seed, 0);
        create_mint(&mut self.ctx, mint, self.admin, decimals);
        mint
    }

    /// The vault of `mint` hedged on `market`'s Phoenix market (`edit` changes the market's
    /// parameters before they are sent), under authority nonce `nonce`.
    pub fn spot_vault_on(&self, mint: Pubkey, market: Asset, nonce: u64) -> Vault {
        vault_keys(&self.ctx, market, VaultKind::Spot, mint, 8, nonce)
    }

    pub fn try_create_vault_on(
        &mut self,
        mint: Pubkey,
        market: Asset,
        edit: impl FnOnce(&mut MarketParams),
    ) -> Result<TransactionMetadata, Vec<String>> {
        let mut v = self.spot_vault_on(mint, market, 0);
        v.token_program = TOKEN_PROGRAM;
        v.vault_asset = Some(get_associated_token_address(&v.authority, &mint));
        let mut m = MarketParams {
            asset_id: v.asset_id,
            base_lot_decimals: v.base_lot_decimals,
            tick_size: v.tick_size,
        };
        edit(&mut m);
        v.asset_id = m.asset_id;
        v.base_lot_decimals = m.base_lot_decimals;
        v.tick_size = m.tick_size;
        self.try_create_vault(&v, default_params())
    }

    /// As `try_create_vault_on`, but naming another order book (and its spline collection).
    pub fn try_create_vault_on_book(
        &mut self,
        mint: Pubkey,
        market: Asset,
        orderbook: Pubkey,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let mut v = self.spot_vault_on(mint, market, 0);
        v.token_program = TOKEN_PROGRAM;
        v.vault_asset = Some(get_associated_token_address(&v.authority, &mint));
        v.orderbook = orderbook;
        v.spline =
            Pubkey::find_program_address(&[b"spline", orderbook.as_ref()], &PHOENIX_PROGRAM_ID).0;
        self.try_create_vault(&v, default_params())
    }

    /// Registers the Phoenix trader account of `v`'s authority from someone else's wallet, as
    /// anyone can.
    pub fn preregister_trader(&mut self, payer_seed: &str, v: &Vault) {
        use phoenix_rise_ix::register_trader::{create_register_trader_ix, RegisterTraderParams};
        let params = RegisterTraderParams::builder()
            .payer(self.ctx.signer_pubkey(payer_seed))
            .trader(v.authority)
            .trader_account(v.trader)
            .max_positions(1)
            .trader_pda_index(0)
            .subaccount_index(0)
            .build()
            .unwrap();
        let ix = create_register_trader_ix(params).unwrap().into();
        self.ctx
            .send_instructions(vec![ix], payer_seed, "register someone else's trader");
    }

    /// Sends `amount` of the asset straight to the vault's token account.
    pub fn donate_asset(&mut self, asset: Asset, from_seed: &str, amount: u64) {
        let from = self.ctx.signer_pubkey(from_seed);
        let v = self.vault(asset).clone();
        let mut data = vec![12];
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(v.decimals);
        let ix = Instruction {
            program_id: v.token_program,
            accounts: vec![
                AccountMeta::new(Self::ata(&from, &v.asset_mint), false),
                AccountMeta::new_readonly(v.asset_mint, false),
                AccountMeta::new(v.vault_asset.unwrap(), false),
                AccountMeta::new_readonly(from, true),
            ],
            data,
        };
        self.send(from_seed, vec![ix], "donate")
            .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    }

    /// Takes every SOL out of the SOL vault's trader account but its rent: what a liquidation
    /// of its collateral would leave.
    pub fn drain_sol_vault(&mut self) {
        let trader = self.sol.trader;
        let mut account = self.ctx.svm.get_account(&trader).unwrap();
        account.lamports = self
            .ctx
            .svm
            .minimum_balance_for_rent_exemption(account.data.len());
        self.ctx.svm.set_account(trader, account).unwrap();
    }

    pub fn update_vault(&mut self, asset: Asset, params: VaultParams) {
        self.try_update_vault(asset, params)
            .unwrap_or_else(|l| panic!("{}", l.join("\n")));
    }

    pub fn try_update_vault(
        &mut self,
        asset: Asset,
        params: VaultParams,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let v = self.vault(asset).clone();
        let ix = self.ix(
            accounts::UpdateVault {
                admin: self.admin,
                config: self.config,
                basis_vault: v.basis_vault,
            },
            instruction::UpdateVault { params },
        );
        self.send(ADMIN, vec![ix], "update vault")
    }

    /// The keeper's funding guard: opens or closes a vault's deposits, signed by `signer`.
    pub fn set_deposits_open(
        &mut self,
        signer: &str,
        asset: Asset,
        open: bool,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let ix = self.ix(
            accounts::SetDeposits {
                signer: self.ctx.signer_pubkey(signer),
                config: self.config,
                basis_vault: self.vault(asset).basis_vault,
            },
            instruction::SetDepositsOpen { open },
        );
        self.send(signer, vec![ix], "set deposits")
    }

    pub fn keeper_seed(&self) -> &'static str {
        KEEPER
    }

    pub fn basis_vault(&self, asset: Asset) -> BasisVault {
        let data = self.ctx.account_data(&self.vault(asset).basis_vault);
        BasisVault::try_deserialize(&mut &data[..]).unwrap()
    }

    /// Phoenix's authority grants the vault's trader its capabilities, the way the fixture
    /// onboards its own takers.
    fn onboard(&mut self, asset: Asset) {
        let taker = self.ctx.actor("taker0");
        let template = self
            .ctx
            .fixture
            .setup_transactions
            .iter()
            .flat_map(|t| t.instructions.iter())
            .find(|ix| ix.name == "enableTaker0TraderCapabilities")
            .expect("capability template")
            .clone();
        let mut ix = decode_fixture_instruction(&template).unwrap();
        let taker_trader = parse_pubkey(&taker.trader_account).unwrap();
        let trader = self.vault(asset).trader;
        for meta in ix.accounts.iter_mut() {
            if meta.pubkey == taker_trader {
                meta.pubkey = trader;
            }
        }
        self.ctx
            .send_instructions(vec![ix], "payer", "enable vault capabilities");
    }

    /// The fixture maker cancels its orders on the market and quotes both sides 0.05% around
    /// `price_usd`.
    pub fn quote(&mut self, asset: Asset, price_usd: f64) {
        let maker = self.ctx.actor(MAKER);
        let market = self.ctx.market(asset.symbol());
        let trader = parse_pubkey(&maker.pubkey).unwrap();
        let trader_account = parse_pubkey(&maker.trader_account).unwrap();
        let orderbook = parse_pubkey(&market.orderbook).unwrap();
        let spline = parse_pubkey(&market.spline).unwrap();
        let cancel = CancelAllParams::builder()
            .trader(trader)
            .trader_account(trader_account)
            .perp_asset_map(self.perp_asset_map)
            .orderbook(orderbook)
            .spline_collection(spline)
            .global_trader_index(self.gti.clone())
            .active_trader_buffer(self.atb.clone())
            .build()
            .unwrap();
        let mut ixs: Vec<Instruction> = vec![create_cancel_all_ix(cancel).unwrap().into()];
        for (side, price) in [
            (Side::Bid, price_usd * 0.9995),
            (Side::Ask, price_usd * 1.0005),
        ] {
            let params = LimitOrderParams::builder()
                .trader(trader)
                .trader_account(trader_account)
                .perp_asset_map(self.perp_asset_map)
                .orderbook(orderbook)
                .spline_collection(spline)
                .global_trader_index(self.gti.clone())
                .active_trader_buffer(self.atb.clone())
                .side(side)
                .price_in_ticks(market.price_usd_to_ticks(price, 6))
                .num_base_lots(asset.quote_lots())
                .build()
                .unwrap();
            ixs.push(create_place_limit_order_ix(params).unwrap().into());
        }
        self.ctx.send_instructions(ixs, &maker.seed, "maker quotes");
    }

    /// Another trader (the fixture's `taker0`) trades `lots` on the asset's market at any price
    /// within 1%: open interest the vault does not own.
    pub fn outside_trade(&mut self, asset: Asset, sell: bool, lots: u64) {
        use phoenix_rise_ix::market_order::{create_place_market_order_ix, MarketOrderParams};
        let taker = self.ctx.actor("taker0");
        let v = self.vault(asset).clone();
        let mark = self.view_asset(asset).mark_price_ticks;
        let limit = if sell {
            mark * 99 / 100
        } else {
            mark * 101 / 100
        };
        let params = MarketOrderParams::builder()
            .trader(parse_pubkey(&taker.pubkey).unwrap())
            .trader_account(parse_pubkey(&taker.trader_account).unwrap())
            .perp_asset_map(self.perp_asset_map)
            .orderbook(v.orderbook)
            .spline_collection(v.spline)
            .global_trader_index(self.gti.clone())
            .active_trader_buffer(self.atb.clone())
            .side(if sell { Side::Ask } else { Side::Bid })
            .price_in_ticks(limit)
            .num_base_lots(lots)
            .min_base_lots_to_fill(lots)
            .build()
            .unwrap();
        let ix = create_place_market_order_ix(params).unwrap().into();
        self.ctx
            .send_instructions(vec![ix], &taker.seed, "outside trade");
        let price = self.oracle[asset.symbol()].0;
        self.quote(asset, price);
    }

    /// The asset's slot in Phoenix's perp asset map: (account data, offset of the slot).
    fn asset_slot(&self, asset: Asset) -> (Vec<u8>, usize) {
        let data = self.ctx.account_data(&self.perp_asset_map);
        let slots = u32::from_le_bytes(data[32..36].try_into().unwrap()) as usize;
        let id = self.vault(asset).asset_id;
        let at = (0..slots)
            .map(|slot| 48 + slot * 1_584)
            .find(|at| {
                let low = u16::from_le_bytes(data[at + 960..at + 962].try_into().unwrap()) as u32;
                let high = u16::from_le_bytes(data[at + 964..at + 966].try_into().unwrap()) as u32;
                low | high << 16 == id
            })
            .expect("market in the perp asset map");
        (data, at)
    }

    /// The market's open interest in base lots, as the program reads it.
    pub fn open_interest_lots(&self, asset: Asset) -> u64 {
        let (data, at) = self.asset_slot(asset);
        u64::from_le_bytes(data[at + 1_440..at + 1_448].try_into().unwrap())
    }

    /// Sets or clears Phoenix's after-hours flag on the market, as Phoenix does when a stock
    /// market's session ends on Friday and starts again on Sunday.
    pub fn set_after_hours(&mut self, asset: Asset, on: bool) {
        let (mut data, at) = self.asset_slot(asset);
        if on {
            data[at + 1_472] |= 1 << 2;
        } else {
            data[at + 1_472] &= !(1 << 2);
        }
        let mut account = self.ctx.svm.get_account(&self.perp_asset_map).unwrap();
        account.data = data;
        self.ctx
            .svm
            .set_account(self.perp_asset_map, account)
            .unwrap();
    }

    /// Moves Phoenix's mark and spot price and the maker's quotes to `price_usd`.
    pub fn move_price(&mut self, asset: Asset, price_usd: f64) {
        self.ctx.move_market_price_usd(asset.symbol(), price_usd);
        self.oracle.insert(asset.symbol(), (price_usd, price_usd));
        self.quote(asset, price_usd);
    }

    /// Sets the perp price apart from spot. Phoenix's funding follows the premium: with the
    /// perp above spot, longs pay shorts.
    pub fn set_perp_premium(&mut self, asset: Asset, perp_usd: f64, spot_usd: f64) {
        self.oracle.insert(asset.symbol(), (perp_usd, spot_usd));
        self.send_oracle(asset);
        self.quote(asset, perp_usd);
    }

    fn send_oracle(&mut self, asset: Asset) {
        let (perp, spot) = self.oracle[asset.symbol()];
        self.ctx
            .update_oracle_prices(&[FixtureOraclePriceUpdate::new(
                asset.symbol(),
                Some(FixturePrice::from_usd(perp)),
                FixturePrice::from_usd(spot),
            )]);
    }

    /// Moves the clock forward and refreshes every oracle price at the new time.
    pub fn warp_hours(&mut self, hours: i64) {
        let mut clock: Clock = self.ctx.svm.get_sysvar();
        clock.unix_timestamp += hours * 3_600;
        clock.slot += hours as u64 * 9_000;
        self.ctx.svm.set_sysvar(&clock);
        for asset in Asset::ALL {
            self.send_oracle(asset);
        }
    }

    pub fn warp_slots(&mut self, slots: u64) {
        let mut clock: Clock = self.ctx.svm.get_sysvar();
        clock.slot += slots;
        clock.unix_timestamp += (slots * 2 / 5) as i64;
        self.ctx.svm.set_sysvar(&clock);
        for asset in Asset::ALL {
            self.send_oracle(asset);
        }
    }

    // ------------------------------------------------------------ users and tokens

    pub fn add_user(&mut self, name: &str, lamports: u64) -> Pubkey {
        let user = self.ctx.add_signer(name, lamports);
        let mints = [self.usdc_mint, ETH_MINT, BTC_MINT, TSLA_MINT, META_MINT];
        for mint in mints {
            self.create_ata(name, user, mint);
        }
        user
    }

    /// The token program of a mint: Token-2022 for the stock tokens.
    fn token_program_of(mint: &Pubkey) -> Pubkey {
        if *mint == TSLA_MINT || *mint == META_MINT {
            TOKEN_2022
        } else {
            TOKEN_PROGRAM
        }
    }

    /// `owner`'s associated token account of `mint`, under the mint's token program.
    pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(owner, mint, &Self::token_program_of(mint))
    }

    pub fn create_ata(&mut self, payer: &str, owner: Pubkey, mint: Pubkey) -> Pubkey {
        let payer_key = self.ctx.signer_pubkey(payer);
        let ata = Self::ata(&owner, &mint);
        let ix = Instruction {
            program_id: ATA_PROGRAM,
            accounts: vec![
                AccountMeta::new(payer_key, true),
                AccountMeta::new(ata, false),
                AccountMeta::new_readonly(owner, false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
                AccountMeta::new_readonly(Self::token_program_of(&mint), false),
            ],
            data: vec![1],
        };
        self.ctx.send_instructions(vec![ix], payer, "create ata");
        ata
    }

    pub fn mint_usdc(&mut self, owner: Pubkey, amount: u64) {
        let ata = get_associated_token_address(&owner, &self.usdc_mint);
        self.ctx.mint_fake_usdc_to_token_account(ata, amount);
    }

    pub fn mint_asset(&mut self, asset: Asset, owner: Pubkey, amount: u64) {
        let mint = self.vault(asset).asset_mint;
        let mut data = vec![7];
        data.extend_from_slice(&amount.to_le_bytes());
        let ix = Instruction {
            program_id: asset.token_program(),
            accounts: vec![
                AccountMeta::new(mint, false),
                AccountMeta::new(Self::ata(&owner, &mint), false),
                AccountMeta::new_readonly(self.admin, true),
            ],
            data,
        };
        self.ctx.send_instructions(vec![ix], ADMIN, "mint asset");
    }

    /// Balance of a token account; 0 if it does not exist.
    pub fn token_at(&self, account: &Pubkey) -> u64 {
        match self.ctx.svm.get_account(account) {
            Some(account) if account.data.len() >= 72 => {
                u64::from_le_bytes(account.data[64..72].try_into().unwrap())
            }
            _ => 0,
        }
    }

    pub fn token(&self, owner: Pubkey, mint: Pubkey) -> u64 {
        self.token_at(&Self::ata(&owner, &mint))
    }

    pub fn usdc(&self, owner: Pubkey) -> u64 {
        self.token(owner, self.usdc_mint)
    }

    pub fn asset_balance(&self, asset: Asset, owner: Pubkey) -> u64 {
        match asset {
            Asset::Sol => self.ctx.svm.get_balance(&owner).unwrap_or(0),
            _ => self.token(owner, self.vault(asset).asset_mint),
        }
    }

    pub fn shares(&self, asset: Asset, owner: Pubkey) -> u64 {
        self.token(owner, self.vault(asset).share_mint)
    }

    /// Whether a holder's share account is frozen (SPL token account state 2).
    pub fn shares_frozen(&self, asset: Asset, owner: Pubkey) -> bool {
        let ata = get_associated_token_address(&owner, &self.vault(asset).share_mint);
        self.ctx
            .svm
            .get_account(&ata)
            .map(|a| a.data[108] == 2)
            .unwrap_or(false)
    }

    /// The share mint's freeze authority, if it has one.
    pub fn share_freeze_authority(&self, asset: Asset) -> Option<Pubkey> {
        let data = self.ctx.account_data(&self.vault(asset).share_mint);
        (data[46..50] != [0; 4]).then(|| Pubkey::try_from(&data[50..82]).unwrap())
    }

    /// A plain SPL token transfer of shares between two holders, signed by the sender.
    pub fn transfer_shares(
        &mut self,
        asset: Asset,
        from_seed: &str,
        to: Pubkey,
        amount: u64,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let from = self.ctx.signer_pubkey(from_seed);
        let mint = self.vault(asset).share_mint;
        let mut data = vec![12];
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(SHARE_DECIMALS);
        let ix = Instruction {
            program_id: TOKEN_PROGRAM,
            accounts: vec![
                AccountMeta::new(get_associated_token_address(&from, &mint), false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new(get_associated_token_address(&to, &mint), false),
                AccountMeta::new_readonly(from, true),
            ],
            data,
        };
        self.send(from_seed, vec![ix], "transfer shares")
    }

    // ------------------------------------------------------------ vault calls

    fn core(&self, asset: Asset) -> accounts::VaultCore {
        let v = self.vault(asset);
        accounts::VaultCore {
            config: self.config,
            basis_vault: v.basis_vault,
            authority: v.authority,
            share_mint: v.share_mint,
            usdc_mint: self.usdc_mint,
            phoenix_mint: self.phoenix_mint,
            vault_usdc: v.vault_usdc,
            vault_phoenix: v.vault_phoenix,
            vault_asset: v.vault_asset,
            asset_mint: v.asset_mint,
            asset_token_program: v.token_program,
            trader_account: v.trader,
            phoenix: vanna_phoenix_vault::instructions::PhoenixProgramsAccounts {
                phoenix_program: PHOENIX_PROGRAM_ID,
                log_authority: PHOENIX_LOG_AUTHORITY,
                global_config: PHOENIX_GLOBAL_CONFIG,
                hawkeye_program: HAWKEYE_ID,
                ember_program: EMBER_PROGRAM_ID,
                ember_state: self.ember_state,
                ember_vault: self.ember_vault,
            },
            perp_asset_map: self.perp_asset_map,
            orderbook: v.orderbook,
            spline_collection: v.spline,
            withdraw_queue: self.withdraw_queue,
            global_vault: self.global_vault,
        }
    }

    /// A deposit of `usdc_amount`, with the route the app builds for it: the part the vault's
    /// mix buys the asset with (`deposit_swap_amount`), at Phoenix's mark.
    pub fn deposit_ix(
        &mut self,
        asset: Asset,
        user: Pubkey,
        usdc_amount: u64,
        min_shares: u64,
    ) -> Instruction {
        let target = self.basis_vault(asset).target_margin_bps;
        let planned = deposit_swap_amount(usdc_amount, target).unwrap().max(1);
        let route = self.route_for(asset, Sell::Usdc, planned);
        self.deposit_ix_with(asset, user, usdc_amount, min_shares, route)
    }

    pub fn deposit_ix_with(
        &self,
        asset: Asset,
        user: Pubkey,
        usdc_amount: u64,
        min_shares: u64,
        route: Route,
    ) -> Instruction {
        let v = self.vault(asset);
        let spot = v.kind == VaultKind::Spot;
        let ix = self.ix(
            accounts::Deposit {
                user,
                core: self.core(asset),
                user_shares: get_associated_token_address(&user, &v.share_mint),
                user_usdc: get_associated_token_address(&user, &self.usdc_mint),
                swap_program: JUPITER,
                vault_wsol: (!spot)
                    .then(|| get_associated_token_address(&v.authority, &NATIVE_MINT)),
                token_program: TOKEN_PROGRAM,
                associated_token_program: ATA_PROGRAM,
                system_program: SYSTEM_PROGRAM,
            },
            instruction::Deposit {
                gti: self.gti.len() as u8,
                atb: self.atb.len() as u8,
                usdc_amount,
                min_shares,
                route: route.args.clone(),
            },
        );
        let mut ix = self.with_index(ix);
        ix.accounts.extend(route.metas);
        ix
    }

    /// Deposits `usdc_amount` of the user's USDC (minted to them first).
    pub fn deposit(
        &mut self,
        asset: Asset,
        user_seed: &str,
        usdc_amount: u64,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let user = self.ctx.signer_pubkey(user_seed);
        self.mint_usdc(user, usdc_amount);
        let ix = self.deposit_ix(asset, user, usdc_amount, 0);
        self.send(user_seed, vec![ix], "deposit")
    }

    /// A withdrawal of `shares`, with a route for about the asset it will sell
    /// (`withdraw_estimate`).
    pub fn withdraw_ix(
        &mut self,
        asset: Asset,
        user: Pubkey,
        shares: u64,
        min_usdc_out: u64,
    ) -> Instruction {
        let estimate = self.withdraw_estimate(asset, shares);
        let route = (estimate > 0).then(|| self.route_for(asset, Sell::Asset, estimate));
        self.withdraw_ix_with(asset, user, shares, min_usdc_out, route)
    }

    pub fn withdraw_ix_with(
        &self,
        asset: Asset,
        user: Pubkey,
        shares: u64,
        min_usdc_out: u64,
        route: Option<Route>,
    ) -> Instruction {
        let v = self.vault(asset);
        let spot = v.kind == VaultKind::Spot;
        let swaps_sol = !spot && route.is_some();
        let ix = self.ix(
            accounts::Withdraw {
                user,
                core: self.core(asset),
                user_shares: get_associated_token_address(&user, &v.share_mint),
                user_usdc: get_associated_token_address(&user, &self.usdc_mint),
                swap_program: route.is_some().then_some(JUPITER),
                vault_wsol: swaps_sol
                    .then(|| get_associated_token_address(&v.authority, &NATIVE_MINT)),
                token_program: TOKEN_PROGRAM,
                associated_token_program: ATA_PROGRAM,
                system_program: SYSTEM_PROGRAM,
            },
            instruction::Withdraw {
                gti: self.gti.len() as u8,
                atb: self.atb.len() as u8,
                shares,
                min_usdc_out,
                route: route.as_ref().map(|r| r.args.clone()),
            },
        );
        let mut ix = self.with_index(ix);
        if let Some(route) = route {
            ix.accounts.extend(route.metas);
        }
        ix
    }

    /// Withdraws `shares` into USDC. When Phoenix lets no asset leave (the SOL vault's SOL
    /// held for its USDC debt), the app sends the withdrawal again without a route.
    pub fn withdraw(
        &mut self,
        asset: Asset,
        user_seed: &str,
        shares: u64,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let user = self.ctx.signer_pubkey(user_seed);
        let ix = self.withdraw_ix(asset, user, shares, 0);
        match self.send(user_seed, vec![ix], "withdraw") {
            Err(logs) if logs.iter().any(|l| l.contains("SwapNotNeeded")) => {
                let ix = self.withdraw_ix_with(asset, user, shares, 0, None);
                self.send(user_seed, vec![ix], "withdraw")
            }
            result => result,
        }
    }

    pub fn rebalance(
        &mut self,
        asset: Asset,
        buy_asset: bool,
        asset_amount: u64,
        usdc_amount: u64,
    ) -> Result<TransactionMetadata, Vec<String>> {
        self.rebalance_as(KEEPER, asset, buy_asset, asset_amount, usdc_amount)
    }

    pub fn rebalance_as(
        &mut self,
        signer: &str,
        asset: Asset,
        buy_asset: bool,
        asset_amount: u64,
        usdc_amount: u64,
    ) -> Result<TransactionMetadata, Vec<String>> {
        let keeper = self.ctx.signer_pubkey(signer);
        let v = self.vault(asset);
        let spot = v.kind == VaultKind::Spot;
        let ix = self.ix(
            accounts::Rebalance {
                keeper,
                core: self.core(asset),
                keeper_asset: spot.then(|| Self::ata(&keeper, &v.asset_mint)),
                keeper_usdc: get_associated_token_address(&keeper, &self.usdc_mint),
                token_program: TOKEN_PROGRAM,
                system_program: SYSTEM_PROGRAM,
            },
            instruction::Rebalance {
                gti: self.gti.len() as u8,
                atb: self.atb.len() as u8,
                buy_asset,
                asset_amount,
                usdc_amount,
            },
        );
        let ix = self.with_index(ix);
        self.send(signer, vec![ix], "rebalance")
    }

    pub fn setup_keeper_accounts(&mut self) {
        let keeper = self.keeper;
        for mint in [self.usdc_mint, ETH_MINT, BTC_MINT, TSLA_MINT, META_MINT] {
            self.create_ata(KEEPER, keeper, mint);
        }
    }

    // ------------------------------------------------------------ Phoenix views

    fn hawkeye(&mut self, ix: phoenix_rise_ix::types::Instruction) -> Vec<u8> {
        let meta = self
            .ctx
            .try_send_instructions_with_metadata(vec![ix.into()], ADMIN)
            .unwrap_or_else(|f| panic!("hawkeye view failed:\n{}", f.meta.logs.join("\n")));
        meta.return_data.data
    }

    fn view_accounts(&self, trader: Pubkey) -> HawkeyeTraderViewAccounts {
        HawkeyeTraderViewAccounts {
            phoenix_program_id: PHOENIX_PROGRAM_ID,
            global_config: PHOENIX_GLOBAL_CONFIG,
            global_trader_index: self.gti.clone(),
            active_trader_buffer: self.atb.clone(),
            perp_asset_map: self.perp_asset_map,
            trader,
        }
    }

    pub fn view_margin(&mut self, asset: Asset) -> ViewMarginReturn {
        let accounts = self.view_accounts(self.vault(asset).trader);
        let data = self.hawkeye(create_hawkeye_view_margin_ix(accounts));
        decode_hawkeye_return(&pad::<ViewMarginReturn>(data), "view_margin").unwrap()
    }

    pub fn view_asset(&mut self, asset: Asset) -> ViewAssetReturn {
        let v = self.vault(asset).clone();
        let data = self.hawkeye(create_hawkeye_view_margin_for_asset_ix(
            self.view_accounts(v.trader),
            v.asset_id,
        ));
        decode_hawkeye_return(&pad::<ViewAssetReturn>(data), "view_margin_for_asset").unwrap()
    }

    /// The vault's NAV and position, computed the way the program does.
    pub fn view(&mut self, asset: Asset) -> VaultView {
        let v = self.vault(asset).clone();
        let a = self.view_asset(asset);
        let m = self.view_margin(asset);
        let asset_units = match v.vault_asset {
            Some(vault_asset) => self.token_at(&vault_asset),
            None => {
                let account = self.ctx.svm.get_account(&v.trader).unwrap();
                let floor = self
                    .ctx
                    .svm
                    .minimum_balance_for_rent_exemption(account.data.len());
                account.lamports - floor
            }
        };
        let usd = |q: i64| q as f64 / USDC as f64;
        let mark_usd =
            a.mark_price_ticks as f64 * v.tick_size as f64 * 10f64.powi(v.base_lot_decimals as i32)
                / USDC as f64;
        let multiplier = self.multiplier(asset);
        let asset_usd = asset_units as f64 / 10f64.powi(v.decimals as i32) * mark_usd * multiplier;
        let equity = usd(m.collateral_quote_lots)
            + usd(m.unrealized_pnl_quote_lots)
            + usd(m.unsettled_funding_quote_lots);
        let supply_data = self.ctx.account_data(&v.share_mint);
        VaultView {
            mark_usd,
            position_lots: a.base_lots,
            asset_units,
            collateral_usd: usd(m.collateral_quote_lots),
            upnl_usd: usd(m.unrealized_pnl_quote_lots),
            funding_usd: usd(m.unsettled_funding_quote_lots),
            effective_usd: usd(m.effective_collateral_quote_lots),
            nav_usd: (asset_usd + equity).max(0.0),
            supply: u64::from_le_bytes(supply_data[36..44].try_into().unwrap()),
        }
    }

    pub fn print(&mut self, asset: Asset, label: &str) -> VaultView {
        let v = self.view(asset);
        let d = self.vault(asset).decimals;
        eprintln!(
            "[{}] {label:<34} mark ${:>10.2} | held {:>10.4} {} | short {:>9.4} | USDC ${:>10.2} | uPnL ${:>9.2} | funding ${:>7.2} | NAV ${:>11.2} | share ${:.6}",
            asset.symbol(),
            v.mark_usd,
            v.asset_units as f64 / 10f64.powi(d as i32),
            asset.symbol(),
            -v.position_lots as f64 / 10f64.powi(self.vault(asset).base_lot_decimals as i32),
            v.collateral_usd,
            v.upnl_usd,
            v.funding_usd,
            v.nav_usd,
            v.share_price(),
        );
        v
    }
}

/// Solana trims trailing zero bytes from return data; the views are fixed-size.
fn pad<T>(mut data: Vec<u8>) -> Vec<u8> {
    data.resize(std::mem::size_of::<T>(), 0);
    data
}

fn vault_keys(
    ctx: &SdkLocalnetContext,
    asset: Asset,
    kind: VaultKind,
    asset_mint: Pubkey,
    decimals: u8,
    nonce: u64,
) -> Vault {
    let id = vanna_phoenix_vault::ID;
    let basis_vault = Pubkey::find_program_address(&[BASIS_VAULT_SEED, asset_mint.as_ref()], &id).0;
    let authority = authority_address(&basis_vault, nonce).0;
    let market = ctx.market(asset.symbol());
    let asset_id = ctx
        .fixture
        .markets
        .iter()
        .position(|m| m.symbol == asset.symbol())
        .unwrap() as u32;
    let usdc_mint = ctx.fake_usdc_mint();
    let phoenix_mint = ctx.phoenix_collateral_mint();
    Vault {
        kind,
        nonce,
        asset_mint,
        decimals,
        token_program: asset.token_program(),
        basis_vault,
        authority,
        share_mint: Pubkey::find_program_address(&[SHARE_MINT_SEED, basis_vault.as_ref()], &id).0,
        trader: trader_account_address(&authority),
        vault_usdc: get_associated_token_address(&authority, &usdc_mint),
        vault_phoenix: get_associated_token_address(&authority, &phoenix_mint),
        vault_asset: (kind == VaultKind::Spot).then(|| {
            get_associated_token_address_with_program_id(
                &authority,
                &asset_mint,
                &asset.token_program(),
            )
        }),
        orderbook: parse_pubkey(&market.orderbook).unwrap(),
        spline: parse_pubkey(&market.spline).unwrap(),
        asset_id,
        base_lot_decimals: market.base_lot_decimals as u8,
        tick_size: market.tick_size_in_quote_lots_per_base_lot,
    }
}

/// A swap route: the instruction data the vault runs and the accounts it needs.
#[derive(Clone, Debug)]
pub struct Route {
    pub args: SwapRoute,
    pub metas: Vec<AccountMeta>,
}

/// The mock Jupiter's reserve authority: it pays the swaps' output.
pub fn mock_reserve() -> Pubkey {
    Pubkey::find_program_address(&[b"reserve"], &JUPITER).0
}

/// The Jupiter routes and mainnet accounts written by `scripts/jupiter_fork.py`.
pub fn jupiter_snapshot() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/jupiter_routes.json")).unwrap()
}

/// A real Jupiter route from the snapshot.
pub struct JupiterRoute {
    pub route: Route,
    /// The vault authority the route was built for.
    pub authority: Pubkey,
    pub in_amount: u64,
    /// Jupiter's quote; the route itself only guarantees this less its slippage (0.5%).
    pub quoted_out: u64,
}

/// The snapshotted route of each vault, each way.
fn route_name(asset: Asset, sell: Sell) -> &'static str {
    match (asset, sell) {
        (Asset::Eth, Sell::Usdc) => "eth_buy",
        (Asset::Eth, Sell::Asset) => "eth_sell",
        (Asset::Btc, Sell::Usdc) => "btc_buy",
        (Asset::Btc, Sell::Asset) => "btc_sell",
        (Asset::Sol, Sell::Usdc) => "sol_buy",
        (Asset::Sol, Sell::Asset) => "sol_sell",
        (Asset::Tsla, Sell::Usdc) => "tsla_buy",
        (Asset::Tsla, Sell::Asset) => "tsla_sell",
        (Asset::Meta, Sell::Usdc) => "meta_buy",
        (Asset::Meta, Sell::Asset) => "meta_sell",
    }
}

/// The native SOL mint, as on mainnet: no mint authority, 9 decimals.
fn create_native_mint(ctx: &mut SdkLocalnetContext) {
    let mut data = vec![0u8; 82];
    data[44] = 9;
    data[45] = 1;
    let lamports = ctx.svm.minimum_balance_for_rent_exemption(82);
    ctx.svm
        .set_account(
            NATIVE_MINT,
            solana_account::Account {
                lamports,
                data,
                owner: TOKEN_PROGRAM,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

/// A classic SPL mint with `authority` as mint authority, written straight into the SVM.
fn create_mint(ctx: &mut SdkLocalnetContext, mint: Pubkey, authority: Pubkey, decimals: u8) {
    let mut data = vec![0u8; 82];
    data[0..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..36].copy_from_slice(authority.as_ref());
    data[44] = decimals;
    data[45] = 1;
    let lamports = ctx.svm.minimum_balance_for_rent_exemption(82);
    ctx.svm
        .set_account(
            mint,
            solana_account::Account {
                lamports,
                data,
                owner: TOKEN_PROGRAM,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

/// A mainnet mint from the Jupiter snapshot (an xStock: Token-2022 with its real extensions
/// and multiplier), with `authority` as its mint authority so tests can mint it.
fn create_mainnet_mint(ctx: &mut SdkLocalnetContext, mint: Pubkey, authority: Pubkey) {
    use base64::Engine;
    let snapshot = jupiter_snapshot();
    let account = &snapshot["accounts"][mint.to_string()];
    let mut data = base64::engine::general_purpose::STANDARD
        .decode(
            account["data"]
                .as_str()
                .expect("mint in the Jupiter snapshot"),
        )
        .unwrap();
    data[0..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..36].copy_from_slice(authority.as_ref());
    let lamports = ctx.svm.minimum_balance_for_rent_exemption(data.len());
    ctx.svm
        .set_account(
            mint,
            solana_account::Account {
                lamports,
                data,
                owner: account["owner"].as_str().unwrap().parse().unwrap(),
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
}

/// Adds a stock perp market to Phoenix's fixture by repeating the SOL market's setup (order
/// book, market, activation, spline) under the stock's symbol, with mainnet's TSLA and META
/// lot and tick sizes (0.001-share lots, 10 quote lots per tick) and leverage of 20x up to
/// 10,000 shares. `Env::move_price` gives it its first price.
fn add_stock_market(fixture: &mut SdkLocalnetFixture, symbol: &str) {
    use base64::Engine;
    const BASE_LOT_DECIMALS: i8 = 3;
    const TICK_SIZE: u64 = 10;
    let seed = format!("market-{symbol}");
    let orderbook =
        Keypair::new_from_array(sdk_localnet_seed_bytes(&fixture.keypair_base_seed, &seed))
            .pubkey();
    let spline =
        Pubkey::find_program_address(&[b"spline", orderbook.as_ref()], &PHOENIX_PROGRAM_ID).0;
    let sol = fixture
        .markets
        .iter()
        .find(|m| m.symbol == "SOL")
        .unwrap()
        .clone();
    let renames: HashMap<String, String> = [
        (sol.orderbook.clone(), orderbook.to_string()),
        (sol.spline.clone(), spline.to_string()),
    ]
    .into();
    fixture.signers.push(FixtureSigner {
        name: format!("{}Market", symbol.to_lowercase()),
        role: format!("{symbol} orderbook"),
        seed: seed.clone(),
        pubkey: orderbook.to_string(),
        initial_lamports: 0,
    });
    fixture.markets.push(FixtureMarket {
        symbol: symbol.to_string(),
        orderbook: orderbook.to_string(),
        spline: spline.to_string(),
        signer_seed: seed,
        base_lot_decimals: BASE_LOT_DECIMALS,
        tick_size_in_quote_lots_per_base_lot: TICK_SIZE,
        liquidity: FixtureLiquidity {
            bids: vec![],
            asks: vec![],
        },
        ..sol
    });
    let copied = [
        "createSOLOrderbook",
        "addSOLMarket",
        "activateSOLMarket",
        "registerSOLSpline",
        "updateSOLSplinePrice",
    ];
    let transactions = fixture
        .setup_transactions
        .iter_mut()
        .chain(fixture.action_templates.iter_mut());
    for transaction in transactions {
        let copies: Vec<_> = transaction
            .instructions
            .iter()
            .filter(|ix| copied.contains(&ix.name.as_str()))
            .map(|ix| {
                let mut ix = ix.clone();
                ix.name = ix.name.replace("SOL", symbol);
                for meta in ix.accounts.iter_mut() {
                    if let Some(new) = renames.get(&meta.pubkey) {
                        meta.pubkey = new.clone();
                    }
                }
                if ix.name.starts_with("add") {
                    let engine = base64::engine::general_purpose::STANDARD;
                    let mut data = engine.decode(&ix.data_base64).unwrap();
                    // Symbol (16 bytes), four leverage tiers (base-lot bound, leverage, limit
                    // order risk factor), tick size, base lot decimals.
                    data[8..24].fill(0);
                    data[8..8 + symbol.len()].copy_from_slice(symbol.as_bytes());
                    for (tier, bound) in [10_000_000u64, 20_000_000, 50_000_000, 100_000_000]
                        .into_iter()
                        .enumerate()
                    {
                        data[24 + 24 * tier..32 + 24 * tier].copy_from_slice(&bound.to_le_bytes());
                    }
                    data[122..130].copy_from_slice(&TICK_SIZE.to_le_bytes());
                    data[130] = BASE_LOT_DECIMALS as u8;
                    ix.data_base64 = engine.encode(data);
                }
                ix
            })
            .collect();
        transaction.instructions.extend(copies);
    }
}

/// Phoenix's fixture mints its own fake USDC, but Jupiter's pools hold mainnet USDC. The
/// fixture is moved onto the mainnet USDC address (its payer becomes the mint authority), so
/// one USDC works in Ember, in the vaults and in the routes. The fake USDC appears only as an
/// account, never in instruction data.
fn use_mainnet_usdc(fixture: &mut SdkLocalnetFixture) {
    let mut renames = HashMap::new();
    for mint in fixture.mints.iter_mut().filter(|m| m.name == "fakeUsdc") {
        renames.insert(mint.pubkey.clone(), USDC_MINT.to_string());
        mint.pubkey = USDC_MINT.to_string();
    }
    for actor in fixture.actors.iter_mut() {
        let owner = parse_pubkey(&actor.pubkey).unwrap();
        let ata = get_associated_token_address(&owner, &USDC_MINT).to_string();
        renames.insert(actor.fake_usdc_token_account.clone(), ata.clone());
        actor.fake_usdc_token_account = ata;
    }
    let transactions = fixture
        .setup_transactions
        .iter_mut()
        .chain(fixture.action_templates.iter_mut());
    for transaction in transactions {
        for ix in transaction.instructions.iter_mut() {
            for meta in ix.accounts.iter_mut() {
                if let Some(new) = renames.get(&meta.pubkey) {
                    meta.pubkey = new.clone();
                }
            }
        }
    }
}

/// Sets how many slots after a USDC deposit Phoenix blocks the trader's USDC withdrawals.
pub fn set_deposit_cooldown(ctx: &mut SdkLocalnetContext, slots: u64) {
    let template = ctx
        .fixture
        .setup_transactions
        .iter()
        .flat_map(|t| t.instructions.iter())
        .find(|ix| ix.name == "setWithdrawCooldown")
        .expect("cooldown template")
        .clone();
    let mut ix = decode_fixture_instruction(&template).unwrap();
    // Option<u64>: tag at 8, value at 9.
    ix.data[9..17].copy_from_slice(&slots.to_le_bytes());
    ctx.send_instructions(vec![ix], "payer", "set deposit cooldown");
}

/// Turns on native SOL collateral with mainnet's limits (read from Phoenix's global config on
/// 2026-10-05): 500 SOL per trader, 50,000 SOL across Phoenix, 20% margin haircut. The
/// fixture's parameter instruction carries one trailing field the mainnet Phoenix build does
/// not have yet, so it is sent without it when needed.
fn enable_native_sol(ctx: &mut SdkLocalnetContext) {
    let template = ctx
        .fixture
        .action_templates
        .iter()
        .find(|t| t.name == "sol-collateral-setup")
        .expect("sol collateral template")
        .clone();
    let ixs: Vec<Instruction> = template
        .instructions
        .iter()
        .map(|ix| decode_fixture_instruction(ix).unwrap())
        .collect();
    ctx.send_instructions(vec![ixs[0].clone()], "payer", "native SOL availability");
    let mut parameters = ixs[1].clone();
    // Option<u64> fields: tag at 8 and 17, value at 9 and 18.
    parameters.data[9..17].copy_from_slice(&(500 * SOL).to_le_bytes());
    parameters.data[18..26].copy_from_slice(&(50_000 * SOL).to_le_bytes());
    if ctx
        .try_send_instructions_with_metadata(vec![parameters.clone()], "payer")
        .is_err()
    {
        parameters.data.pop();
        ctx.send_instructions(vec![parameters], "payer", "native SOL parameters");
    }
    ctx.send_instructions(vec![ixs[2].clone()], "payer", "native SOL activation");
}

/// The program's events of type `T` in a transaction's logs.
pub fn events<T: anchor_lang::Event + anchor_lang::AnchorDeserialize>(
    meta: &TransactionMetadata,
) -> Vec<T> {
    use base64::Engine;
    meta.logs
        .iter()
        .filter_map(|log| log.strip_prefix("Program data: "))
        .filter_map(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
        .filter(|bytes| bytes.starts_with(T::DISCRIMINATOR))
        .map(|bytes| T::deserialize(&mut &bytes[T::DISCRIMINATOR.len()..]).unwrap())
        .collect()
}

/// Whether a failed transaction's logs name `error`.
pub fn failed_with(result: Result<TransactionMetadata, Vec<String>>, error: &str) -> bool {
    match result {
        Ok(_) => false,
        Err(logs) => logs.iter().any(|l| l.contains(error)),
    }
}

/// Serialized size of a one-signer transaction carrying `ixs`: as a legacy transaction, and
/// as a v0 transaction that loads every account except the signer and the invoked programs
/// from one address lookup table.
pub fn transaction_sizes(payer: Pubkey, ixs: &[Instruction]) -> (usize, usize, usize) {
    let mut keys = vec![payer];
    for ix in ixs {
        for k in std::iter::once(&ix.program_id).chain(ix.accounts.iter().map(|m| &m.pubkey)) {
            if !keys.contains(k) {
                keys.push(*k);
            }
        }
    }
    let programs: Vec<Pubkey> = ixs.iter().map(|ix| ix.program_id).collect();
    let static_keys = keys
        .iter()
        .filter(|k| **k == payer || programs.contains(k))
        .count();
    let loaded = keys.len() - static_keys;
    let instructions: usize = ixs
        .iter()
        .map(|ix| {
            1 + compact_len(ix.accounts.len())
                + ix.accounts.len()
                + compact_len(ix.data.len())
                + ix.data.len()
        })
        .sum::<usize>()
        + compact_len(ixs.len());
    let legacy = 1 + 64 + 3 + compact_len(keys.len()) + 32 * keys.len() + 32 + instructions;
    let v0 = 1
        + 1
        + 64
        + 3
        + compact_len(static_keys)
        + 32 * static_keys
        + 32
        + instructions
        + 1
        + 32
        + compact_len(loaded)
        + loaded
        + 1;
    (keys.len(), legacy, v0)
}

fn compact_len(n: usize) -> usize {
    if n < 0x80 {
        1
    } else if n < 0x4000 {
        2
    } else {
        3
    }
}

pub fn usd(amount: u64) -> f64 {
    amount as f64 / USDC as f64
}

/// The program's program data account (upgradeable loader).
pub fn program_data_address() -> Pubkey {
    Pubkey::find_program_address(
        &[vanna_phoenix_vault::ID.as_ref()],
        &anchor_lang::solana_program::bpf_loader_upgradeable::ID,
    )
    .0
}

/// Makes `authority` the program's upgrade authority, as a deploy does (LiteSVM loads programs
/// with none).
fn set_upgrade_authority(ctx: &mut SdkLocalnetContext, authority: Pubkey) {
    let address = program_data_address();
    let mut account = ctx.svm.get_account(&address).expect("program data");
    // UpgradeableLoaderState::ProgramData: tag (4), slot (8), Option<Pubkey> (1 + 32).
    account.data[12] = 1;
    account.data[13..45].copy_from_slice(authority.as_ref());
    ctx.svm.set_account(address, account).unwrap();
}
