//! The keeper against the real Phoenix, Ember and Hawkeye programs and the vault program in
//! LiteSVM, through the vault's own test harness (`programs/vanna_phoenix-multi/tests/env.rs`). Build the vault and the
//! mock Jupiter with `cargo build-sbf` first, as for the vault's tests.

#![allow(clippy::result_large_err)]
#![allow(dead_code)]

mod audit;
#[path = "../../programs/vanna_phoenix-multi/tests/env.rs"]
mod env;
mod flows;
mod scenarios;
mod sim;
