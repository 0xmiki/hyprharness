pub mod audit;
pub mod capture;
pub mod cli;
pub mod error;
pub mod harness;
pub mod input;
pub mod ipc;
pub mod mcp;
pub mod models;
pub mod policy;

pub use error::{HarnessError, Result};
pub use harness::Harness;
