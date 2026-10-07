// Checks the client against reference bytes the program's own Rust types produced
// (programs/vanna_phoenix_vault/tests/client_vectors.rs): instructions, account lists, PDAs,
// Hawkeye views, an event and a vault account. No network needed.
import assert from "node:assert/strict";
import { Connection, PublicKey, TransactionInstruction } from "@solana/web3.js";
import vectors from "./vectors.json";
import {
  ASSETS,
  authorityAddress,
  basisVaultAddress,
  configAddress,
  depositInstruction,
  shareMintAddress,
  toVaultInfo,
  traderAccountAddress,
  vaultProgram,
  VaultInfo,
  viewAssetInstruction,
  viewMarginInstruction,
  withdrawInstruction,
  PhoenixKeys,
} from "../src";

const key = (n: number) => new PublicKey(Buffer.alloc(32, n));
const program = vaultProgram(new Connection("http://127.0.0.1:8899"));

const phoenix: PhoenixKeys = {
  perpAssetMap: key(20),
  withdrawQueue: key(23),
  globalVault: key(24),
  globalTraderIndex: [key(40)],
  activeTraderBuffer: [key(41)],
};

const vault: VaultInfo = {
  address: key(2),
  kind: "spot",
  assetMint: key(10),
  assetDecimals: 8,
  assetTokenProgram: key(11),
  shareMint: key(4),
  authority: key(3),
  usdcAccount: key(7),
  phoenixAccount: key(8),
  assetAccount: key(9),
  traderAccount: key(12),
  assetId: 2,
  orderbook: key(21),
  splineCollection: key(22),
  baseLotDecimals: 3,
  tickSize: 10n,
  maxPositionUsd: 0n,
  maxAssetUnits: 0n,
  maxOiShareBps: 0,
  maxDepositUsd: 0n,
  slippageBps: 100,
  keeperBandBps: 20,
  marginBps: 5000,
  targetMarginBps: 10000,
  depositsOpen: true,
  afterHoursDeposits: false,
  usdcMint: key(5),
  phoenixMint: key(6),
  paused: false,
  swapProgram: PublicKey.default,
};

function same(ix: TransactionInstruction, expected: { metas: [string, boolean, boolean][]; data: string }, label: string) {
  const metas = ix.keys.map((m) => [m.pubkey.toBase58(), m.isSigner, m.isWritable]);
  assert.deepEqual(metas, expected.metas, `${label}: accounts`);
  assert.equal(ix.data.toString("hex"), expected.data, `${label}: data`);
  console.log(`ok  ${label}: ${ix.keys.length} accounts, ${ix.data.length} bytes of data`);
}

async function main() {
  const user = key(30);
  same(
    await depositInstruction(program, {
      vault,
      user,
      usdcAmount: 7_890_000n,
      minShares: 42n,
      route: { args: { accounts: 3, data: Buffer.from([9, 8, 7]) }, metas: [], lookupTables: [], inAmount: 0n, outAmount: 0n, quote: null },
      phoenix,
    }),
    vectors.deposit as any,
    "deposit",
  );
  same(
    await withdrawInstruction(program, {
      vault,
      user,
      shares: 1_000_000n,
      minUsdcOut: 6n,
      phoenix,
    }),
    vectors.withdraw as any,
    "withdraw",
  );
  same(viewMarginInstruction(key(12), phoenix), vectors.view_margin as any, "hawkeye view_margin");
  same(viewAssetInstruction(key(12), 42, phoenix), vectors.view_asset as any, "hawkeye view_margin_for_asset");

  const basisVault = basisVaultAddress(ASSETS.ETH.mint);
  const authority = authorityAddress(basisVault, 7n);
  assert.equal(configAddress().toBase58(), vectors.pdas.config);
  assert.equal(basisVault.toBase58(), vectors.pdas.eth_basis_vault);
  assert.equal(authority.toBase58(), vectors.pdas.eth_authority_nonce_7);
  assert.equal(shareMintAddress(basisVault).toBase58(), vectors.pdas.eth_share_mint);
  assert.equal(traderAccountAddress(authority).toBase58(), vectors.pdas.eth_trader_nonce_7);
  console.log("ok  PDAs: config, vault, authority, share mint, Phoenix trader");

  const event = program.coder.events.decode(Buffer.from(vectors.event_deposited, "hex").toString("base64"));
  assert.ok(event, "event decodes");
  assert.equal(event!.name, "deposited");
  assert.equal(event!.data.shares.toString(), "3");
  assert.equal(event!.data.positionLots.toString(), "-6");
  console.log(`ok  event: ${event!.name} { shares: 3, positionLots: -6, ... }`);

  const decoded = program.coder.accounts.decode("basisVault", Buffer.from(vectors.basis_vault_account, "hex"));
  const info = toVaultInfo(basisVault, decoded, { usdcMint: key(5), phoenixMint: key(6), paused: false, swapProgram: key(1) });
  assert.equal(info.kind, "spot");
  assert.equal(info.assetId, 2);
  assert.equal(info.tickSize, 10n);
  assert.equal(info.maxPositionUsd, 300_000_000_000n);
  assert.equal(info.maxDepositUsd, 250_000_000_000n);
  assert.equal(info.keeperBandBps, 20);
  assert.equal(info.afterHoursDeposits, false);
  assert.ok(info.assetAccount!.equals(key(9)));
  console.log("ok  BasisVault account decodes into VaultInfo");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
