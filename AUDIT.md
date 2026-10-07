# Security audit: `vanna_phoenix_vault`

Date: 2026-10-06. Scope: `programs/vanna_phoenix_vault/src` (Anchor 1.2, about 3,600 lines):
`lib.rs`, `state.rs`, `constants.rs`, `errors.rs`, `events.rs`, `math.rs`, `nav.rs`, `phoenix.rs`,
`swap.rs`, `instructions/{admin,common,vault_core,deposit,withdraw,rebalance}.rs`. The repository
has no git history, so findings refer to the code as it stood before this audit; every fix below
is in the current code.

## Summary

| ID | Finding | Severity | Status |
|---|---|---|---|
| H-01 | The swap venue is set by the admin and runs with the vault authority's signature | High | Fixed |
| H-02 | A keeper key can drain a vault through `rebalance` round trips | High | Fixed (bounded) |
| M-01 | Anyone can `initialize` a fresh deployment and become its admin | Medium | Fixed |
| M-02 | Anyone can pre-register a vault's Phoenix trader and block that vault forever | Medium | Fixed |
| M-03 | `create_vault` takes the market's order book, tick size and lot size on trust | Medium | Fixed |
| M-04 | Hawkeye values the vault from index accounts the caller passes, unchecked on some paths | Medium | Fixed |
| M-05 | Withdrawals re-hedge the whole vault: donations or a liquidated short block or burden leavers | Medium | Fixed |
| M-06 | An xStock multiplier jump at a known time can be deposited around | Medium | Fixed |
| M-07 | A zero price band freezes every exit | Medium | Fixed |
| M-08 | Every withdrawal parses Phoenix's perp asset map at fixed offsets | Medium | Fixed |
| M-09 | Deposits off the vault's mix trade its hedged asset at the perp mark | Medium | Fixed |
| M-10 | Phoenix's withdraw throttle, queue or deposit cooldown make exits revert | Medium | Open |
| L-01 | Deposits into a vault that owes more than it holds pay its debt | Low | Fixed |
| L-02 | Deposits and withdrawals price shares differently (virtual offset on one side only) | Low | Fixed |
| L-03 | A partial payout spreads the leaver's trading cost over the other holders | Low | Fixed |
| L-04 | Donations can push the vault over `max_asset_units` and block deposits | Low | Acknowledged |
| L-05 | SOL above Phoenix's 500-SOL counted limit is shorted without counted collateral | Low | Acknowledged |
| L-06 | The SOL vault's USDC debt stays with the remaining holders | Low | Acknowledged |
| L-07 | Admin transfer is one step; `set_config` rewrites every field | Low | Open |
| L-08 | The xStock issuer can pause, freeze or take the vault's tokens | Low | Acknowledged |
| L-09 | Stray USDC or PhUSD in the vault's token accounts is uncounted and unrecoverable | Info | Open |
| I-01..I-05 | See "Informational" | Info | — |

No finding lets an ordinary user steal funds from an honestly run vault. The two Highs need a
privileged key (admin or keeper); the Mediums are griefing, liveness, admin-error and
mispricing issues. The first-deposit (share inflation) attack is not exploitable; see its own
section.

## Method

- Line-by-line review of every instruction, account constraint, CPI and conversion.
- Three independent focused reviews run in parallel: accounts and CPI safety; share math and
  economics; liveness, the SOL vault and the keeper. Findings were merged and each one checked
  against the code.
- Tools: `cargo audit` (605 crates); `cargo clippy` with `arithmetic_side_effects`,
  `cast_possible_truncation`, `cast_sign_loss`, `cast_possible_wrap`, `indexing_slicing`,
  `unwrap_used`; `cargo-build-sbf` stack-frame checks; LiteSVM tests on the real mainnet Phoenix,
  Ember, Hawkeye, Jupiter v6, Orca Whirlpool and Raydium CLMM programs.
- Reference: Ethena's public audits (StakedUSDe MIN_SHARES, reward vesting, per-block mint and
  redeem limits, role abuse, custodian trust). The specific report mentioned did not come
  through; the mapping below uses the public findings.

Each fix has a regression test in `tests/audit.rs` that attacks the program the way the
finding describes (15 tests). The full suite is 65 tests (49 integration, 16 unit), all passing.

## High

### H-01 The swap venue runs with the vault authority's signature
`swap.rs::invoke_route`, `vault_core.rs::swap`, `admin.rs::initialize`, `set_config`.

The vault authority PDA is the share mint's mint authority, the owner of every vault token
account and the Phoenix trader. Single-asset deposits and withdrawals invoke
`config.swap_program` with that signature, and `set_config` accepted any program at once. The
checks after the route covered three token balances, delegate and close-authority tags and the
authority's lamports, but not the share supply, the mint authority, the token accounts' owner
field or the trader account. A venue the admin key pointed anywhere could do a fair swap (to
pass `check_fill`) and in the same call mint shares to itself, reassign the owner of
`vault_usdc`, or withdraw the vault's Phoenix collateral through Ember and send it out. One
admin key could drain every vault.

Fix: `swap_program` may only be Jupiter v6 or the default key (swaps off), in `initialize` and
`set_config`. After every route the vault also requires the share supply and mint authority,
the token accounts' owner, and the trader account's lamports to be unchanged.
Test: `h01_the_swap_venue_can_only_be_jupiter`.

Residual: the vault trusts Jupiter v6's upgrade authority. A stronger design routes swaps
through a separate transit PDA that holds only the amount to swap.

### H-02 A keeper key can drain a vault through `rebalance` round trips
`rebalance.rs`.

The keeper is the vault's counterparty in `rebalance`, priced within users' `slippage_bps` (1%,
up to 10%). Buying the asset from the vault at 1% under mark and selling it back at 1% over took
about 2% of the traded amount per round trip, plus two hedge trades the vault paid for. The sell
side has no margin or cap check, and nothing limited how often the keeper called. A stolen bot
key could take most of a vault in one transaction.

Fix: the keeper trades within its own `keeper_band_bps` (at most `slippage_bps`; 20 bps at
launch) and at most `max_rebalance_usd` per 24 hours ($50,000 at launch). What a stolen keeper
key can cost a vault is now at most the band on the daily limit (about $100 a day at
launch) plus the hedge trades' fees.
Test: `h02_a_keeper_cannot_drain_the_vault_through_round_trips` (2 round trips, then refused; the
vault lost $49.87).

Recommendation: execute rebalances through a Jupiter route checked by `check_fill`, so the
keeper is never the counterparty.

## Medium

### M-01 `initialize` could be front-run
Anyone could call `initialize` on a fresh deployment and become admin, with their own keeper,
swap venue and Phoenix accounts. Fix: the signer must be the program's upgrade authority
(`ProgramData`). Test: `m01_only_the_upgrade_authority_initializes`.

### M-02 Pre-registering a vault's Phoenix trader blocks the vault
Phoenix registers a trader account for any authority without its signature. The vault
authority was derived from the asset mint alone, so anyone could register the trader of a
future vault (say NVDAx) and `create_vault` would fail forever for that asset. Fix: the
authority's seeds take `authority_nonce`, picked by the admin at random; a blocked address is
skipped by picking another. Test: `m02_a_preregistered_trader_cannot_block_a_vault`.

### M-03 `create_vault` did not check the market
The order book, spline collection, tick size and base-lot decimals were only owner-checked or
taken as given. A wrong tick size misprices the vault (10x for a factor-10 error); a wrong order
book hedges on another market. Neither can be fixed afterwards. Fix: all four must equal
Phoenix's listing of the asset id in its perp asset map (spline = PDA `["spline", orderbook]`).
Test: `m03_create_vault_checks_the_market_against_phoenix`.

### M-04 Hawkeye read the caller's index accounts unchecked on some paths
Phoenix's global trader index and active trader buffer come from `remaining_accounts`, checked
only for Phoenix ownership. Phoenix validates them in its own instructions, but Hawkeye's views
only read them. A spot-vault deposit of under half a lot with no USDC, or a withdrawal small
enough to trade no lots and pay no USDC, ran no Phoenix instruction at all. If Hawkeye could be
made to misread the vault through other Phoenix accounts, the NAV would be wrong: a near-zero
NAV mints almost the whole supply, and an inflated one burns too few shares. Fix: every snapshot
starts with Phoenix's `sync_native`, which validates the index accounts. The deposit-mix rule
(M-09) also closes the deposit path. Test: `m04_fake_trader_index_accounts_are_refused_before_valuing`.

### M-05 Withdrawals re-hedged the whole vault
`withdraw` traded the short to `hedge_lots(units − leaving)`, where `units` is the raw balance.
Asset donated to the vault, or a short Phoenix liquidated or deleveraged, made the next leaver
open new short for all of it. That could revert (thin book, open-interest cap, margin) and block
every exit, or make the leaver pay for everyone. Fix: a withdrawal closes the same share of the
short as the asset that leaves (`VaultCore::close_part`) and never opens short. The keeper
re-hedges with a hedge-only `rebalance` (both amounts zero).
Tests: `m05_a_donation_cannot_block_withdrawals` (3,000 TSLAx donated, more than the book;
the withdrawal goes through), `m05_the_keeper_rehedges_a_donation`.

### M-06 Depositing around an xStock multiplier change
xStocks pay dividends, or apply splits, by raising the token's multiplier at a timestamp stored
in the mint in advance. A deposit just before a large step and a withdrawal after it takes part
of the step from the holders. Around a split, Phoenix's perp and the multiplier change at
different moments, and a deposit could be valued at the wrong number of shares (up to 2x). Fix:
when the multiplier changes by more than 0.25%, deposits are refused from a day before it takes
effect until three days after (`MultiplierChanging`). Withdrawals stay open: they pay in kind,
which is fair whatever the price. A regular dividend (about 0.07% for META) is smaller than a
deposit's own costs and does not close deposits.
Test: `m06_deposits_wait_out_a_large_multiplier_change`.

### M-07 A zero price band froze every exit
`slippage_bps` had no floor. At 0, every withdrawal that trades a lot and every routed swap
reverts. Fix: `slippage_bps` is at least 10. Test: `m07_the_price_band_has_a_floor`.

### M-08 Withdrawals depended on raw parsing of Phoenix's perp asset map
Every snapshot read open interest and session flags at fixed offsets. A Phoenix layout change,
a delisting or a relisting would have reverted every withdrawal until a program upgrade. Fix:
withdrawals no longer read the map at all. Deposits and the keeper check its discriminant and
exact size first, so a layout change stops deposits (fail closed) but never exits.

### M-09 Deposits off the vault's mix traded its hedged asset at the perp mark
The vault values its tokens at Phoenix's perp mark. A deposit of USDC alone followed by a
pro-rata withdrawal bought the vault's tokens at mark; when the perp trades away from spot (a
sell-off, or a stock's weekend), the gap went to that user. Fix: a deposit without a swap must be
within 5% of the vault's mix; a single-asset deposit goes through Jupiter at spot.
Test: `m08_a_deposit_without_a_swap_must_match_the_mix`.

### M-10 Phoenix's withdraw throttle, queue or deposit cooldown make exits revert (open)
If Phoenix's global withdraw budget is spent, its queue is not empty, or it turns on a deposit
cooldown (0 on mainnet today), Phoenix queues or refuses the USDC leg. `ember_withdraw` then
finds no PhUSD and the whole withdrawal reverts. Direct Phoenix users are paid later; vault
holders cannot leave until it clears. Recommendation: let a withdrawal take only what Phoenix
releases now and burn shares for that part (as it already does for SOL), or let users take the
asset leg alone.

## Low

- **L-01 Deposits into an insolvent vault (fixed).** NAV is floored at zero, so a deposit into a
  vault that owes Phoenix more than it holds first pays the debt. Deposits now require positive
  equity when shares exist (`VaultInsolvent`). Test: `l01_no_deposits_into_a_wiped_out_vault`.
- **L-02 Asymmetric share pricing (fixed).** Deposits priced shares with the virtual offset
  (1e6 shares and $1) but withdrawals used the real supply. In a nearly empty vault whose share
  price is far from $1, a large deposit and withdrawal moved value from the holder there.
  Withdrawals now price the part that stays exactly like a deposit. Test:
  `l02_a_round_trip_in_a_tiny_vault_takes_nothing`, and the unit test
  `a_deposit_and_withdrawal_at_once_move_no_value_even_in_a_tiny_vault`.
- **L-03 Partial payouts spread the leaver's cost (fixed).** When Phoenix lets only part of a
  payout leave, the leaver burned shares in proportion to what was paid, but their whole trade
  cost had already come out of the vault. The burn now counts the cost as paid. Unit test:
  `a_partial_payout_burns_only_what_it_paid_for`.
- **L-04 Donations and `max_asset_units` (acknowledged).** Donating SOL to a SOL vault near 450
  SOL blocks deposits that add SOL until withdrawals bring it down. The donor pays, and the
  keeper can take the excess out with `rebalance`.
- **L-05 SOL above Phoenix's counted limit (acknowledged).** Phoenix counts at most 500 SOL per
  trader. The 450-SOL cap stops deposits, but donations (or Phoenix's global cap filling) can
  leave more SOL in the account than counts as margin, which raises the liquidation risk. The
  keeper should watch counted against held SOL.
- **L-06 The SOL vault's USDC debt (acknowledged).** After a rally the SOL vault owes USDC.
  Leavers take SOL and leave the debt behind, so it grows as a share of what remains, and the
  last holder waits for the keeper to repay it. This is the SOL vault's design; the keeper settles
  the debt.
- **L-07 One-step admin transfer (open).** A typo in `set_config`'s admin key loses control for
  good, and each call rewrites every field. Recommendation: propose and accept, and one setter
  per field.
- **L-08 xStock issuer powers (acknowledged).** Backed can pause TSLAx/METAx (exits revert and
  burn nothing until unpaused; tested), freeze the vault's account, take tokens with its
  permanent delegate, or turn on a transfer hook (transfers then fail). `create_vault` rejects
  mints that already have an active hook, transfer fees, interest or frozen accounts.
- **L-09 Stray balances (open, informational).** USDC or PhUSD sent to the vault's token
  accounts is not in the NAV and no instruction can recover it.

## Informational

- **I-01** `cargo audit`: no vulnerabilities. Five crates are flagged unmaintained, all
  transitive; only `bincode` (through Anchor and Solana) ships on-chain.
- **I-02** The keeper can reopen deposits the admin closed (`set_deposits_open`). By design: the
  admin sets the caps, the keeper runs the funding guard.
- **I-03** The open-interest cap reads Phoenix's open interest once per deposit; offsetting
  positions can inflate it. `max_deposit_usd` limits one deposit, not a slot.
- **I-04** Every snapshot now calls Phoenix's `sync_native` (M-04). If Phoenix ever turned off
  native-SOL collateral and `sync_native` failed, spot vaults would stop too. Phoenix stores only
  the first index account of each group, which would be a weaker check once it adds more.
- **I-05** Safe by design: funding accrues continuously (no step to sandwich, unlike Ethena's
  reward transfers). Every conversion rounds for the vault. All casts are checked, and release
  builds trap on overflow. Raw byte reads are length-checked. Re-entrancy is impossible.

## The first-deposit attack

The classic attack on share vaults: be the first depositor with a dust amount, donate a large
amount so one share becomes very expensive, and let the next depositor's shares round down to
zero (or to far fewer than they paid for), then withdraw both deposits. It does not work here:

1. Shares are priced with a virtual offset of 1e6 shares and $1:
   `shares = value × (supply + 1e6) / (NAV + 1e6)`. To round a victim's deposit to zero, the
   attacker must donate about a million times the victim's deposit.
2. A deposit that would mint zero shares reverts (`shares > 0`), and users pass `min_shares`.
3. A donation is unhedged. The next deposit must hedge it, and margin and caps stop that deposit
   if the donation is large. The victim's transaction reverts and they keep their money.
4. The attacker loses part of the donation to the virtual shares; before this audit the
   victim's own shares took it.

Test: `first_deposit_inflation_attack_fails`. The attacker deposits $7 at the mix, donates 10 ETH
($35,000); the victim's $3,500 deposit reverts with their funds untouched; the attacker gets
$30,630 back of $35,007.

L-02 fixed the remaining asymmetry: deposit and withdrawal now price shares the same way, so
nothing moves between holders even in a nearly empty vault.

## Ethena scenarios, mapped

| Ethena issue | Here |
|---|---|
| First deposit / MIN_SHARES | Virtual offset, `shares > 0`, `min_shares`; pricing made symmetric (L-02) |
| Reward sandwiching (8-hour vesting) | Funding accrues continuously; multiplier jumps close deposits (M-06) |
| Per-block mint and redeem limits | `max_deposit_usd`, open-interest share, position and asset caps; keeper daily limit (H-02) |
| Role abuse (minter, gatekeeper) | Keeper band and daily limit (H-02); swap venue pinned (H-01); `initialize` gated (M-01) |
| Restricted users bypassing through transfers | Not applicable: shares are freely transferable by design |
| Custodian and off-chain trust | Phoenix, Ember, Hawkeye, Jupiter and Backed are trusted; their powers are listed above |
| Cooldown changes stranding users | No vault cooldown; Phoenix's own cooldown and throttle are M-10 |
| Signed-order replay | Not applicable: no off-chain signed orders |

## Changes to the interface

- `initialize` takes `program` and `program_data`; its signer must be the upgrade authority.
- `create_vault` takes `authority_nonce`; the authority is PDA
  `["authority", basis_vault, nonce (u64 LE)]`.
- `VaultParams` adds `keeper_band_bps` and `max_rebalance_usd`; `slippage_bps` is 10 to 1,000.
- `VaultCore`'s Phoenix, Hawkeye and Ember programs moved into a nested `phoenix` group (the
  account order changed), and `basis_vault` is writable.
- `rebalance` with both amounts zero re-hedges only.
- A deposit without a route must be within 5% of the vault's mix; USDC alone or the asset alone
  needs a Jupiter route.
- `swap_program` must be Jupiter v6 or the default key.
