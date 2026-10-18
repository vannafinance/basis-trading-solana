//! The keeper bot for Vanna's multi-strategy basis vault.
//!
//! The program holds every move to its limits (buffer, margin floor, leg caps, price band,
//! daily turnover); the keeper decides which moves to make. It reads the vault with the
//! program's own valuation code, weighs Phoenix's funding and open interest, and calls the
//! keeper instructions: `deploy`, `unwind`, `move_margin`, `rehedge`, `set_deposits_open`, and
//! `fulfill_withdraw` (which anyone may call).
//!
//! - [`observe`] reads the vault, its legs and Phoenix's views of them.
//! - [`policy`] decides, one rule at a time, what is worth doing.
//! - [`actions`] turns a decision into the program's instruction, with a Jupiter route.
//! - [`keeper`] runs the loop: look, decide, act, look again.
//! - [`live`] is the backend for a real cluster; the tests run the same keeper in LiteSVM.

pub mod actions;
pub mod backend;
pub mod config;
pub mod funding;
pub mod keeper;
pub mod live;
pub mod observe;
pub mod policy;

pub use backend::{Backend, RawAccount, Route};
pub use config::{KeeperConfig, Policy};
pub use funding::Funding;
pub use keeper::{Keeper, Report, Settings};
pub use observe::{observe, Observation};
pub use policy::{Action, Decision, Planner};
