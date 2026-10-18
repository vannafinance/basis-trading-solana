# @vanna/phoenix-vault-client

TypeScript client for the Vanna Phoenix basis vaults (`vanna_phoenix_vault`). It reads vaults,
builds USDC deposit and withdrawal transactions (each with the Jupiter route that buys or sells
the vault's asset), previews them by simulation, and decodes events and errors.

The integration guide is [docs/FRONTEND_INTEGRATION.md](../docs/FRONTEND_INTEGRATION.md).

```bash
npm install
npm run build   # dist/
npm test        # checks the client against bytes the program's Rust types produced, and the page getters on known numbers
```

| File | What |
|---|---|
| `src/flows.ts` | `prepareDeposit`, `prepareWithdraw`, `depositStatus`: the frontend's actions |
| `src/vault.ts` | `vaultProgram`, `fetchVault`, `depositInstruction`, `withdrawInstruction` |
| `src/state.ts` | `getVaultState`, `getUserPosition`, Hawkeye views, Phoenix API reads |
| `src/views.ts` | `getVaultPage` and the getters behind each field of the vault page: positions, delta, caps, liquidation distance, funding, APY, yield, events, a user's earnings |
| `src/jupiter.ts` | `jupiterRoute`: a Jupiter v6 route built for the vault authority |
| `src/transaction.ts` | `buildTransaction` (v0 + lookup tables), `simulate`, `parseEvents`, errors |
| `src/math.ts` | The program's arithmetic in bigint |
| `src/pda.ts`, `src/constants.ts` | Addresses |
| `src/lookup-table.ts` | Creating a vault's lookup table (ops) |
| `src/idl/` | The program's IDL and its TypeScript types |
