# @vanna/phoenix-vault-client

TypeScript client for the Vanna Phoenix basis vaults (`vanna_phoenix_vault`). It reads vaults,
builds deposit and withdrawal transactions (with Jupiter routes for single-asset flows),
previews them by simulation, and decodes events and errors.

The integration guide is [docs/FRONTEND_INTEGRATION.md](../docs/FRONTEND_INTEGRATION.md).

```bash
npm install
npm run build   # dist/
npm test        # checks the client against bytes the program's Rust types produced
```

| File | What |
|---|---|
| `src/flows.ts` | `prepareDeposit`, `prepareWithdraw`, `depositStatus`: the frontend's actions |
| `src/vault.ts` | `vaultProgram`, `fetchVault`, `depositInstruction`, `withdrawInstruction` |
| `src/state.ts` | `getVaultState`, `getUserPosition`, Hawkeye views, Phoenix API reads |
| `src/jupiter.ts` | `jupiterRoute`: a Jupiter v6 route built for the vault authority |
| `src/transaction.ts` | `buildTransaction` (v0 + lookup tables), `simulate`, `parseEvents`, errors |
| `src/math.ts` | The program's arithmetic in bigint |
| `src/pda.ts`, `src/constants.ts` | Addresses |
| `src/lookup-table.ts` | Creating a vault's lookup table (ops) |
| `src/idl/` | The program's IDL and its TypeScript types |
