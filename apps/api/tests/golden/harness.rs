#![allow(dead_code, unused_imports)]
//! The golden harness now lives in the library (`email_ai_api::evals::safety`) so the same code
//! runs in CI and in the product. The golden tests are thin wrappers over it.

pub use email_ai_api::evals::safety::*;
