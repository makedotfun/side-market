//! The program bytes the Rust tests run: the release build at chain/target/deploy/side_market.so
//! (scripts/chain/build.ts programFiles releaseSo), read from disk the first time a test asks for
//! it, never embedded at compile time (an `include_bytes!` is only as fresh as cargo's mtime check;
//! prize_escrow's tests/program_so says why).
//!
//! - SIDE_MARKET_SO, when set, names another file to load (a cached pair under review).
//! - SIDE_MARKET_SO_SHA256, when set, is the sha256 (hex) the bytes must have: the land gate sets it
//!   from the pair being landed (the release build's provenance record), and every test that loads
//!   the program then fails on any other bytes.
//!
//! The escrow cross-check (tests/escrow.rs) also runs prize_escrow's release build, from
//! PRIZE_ESCROW_SO or chain/target/deploy/prize_escrow.so, held to PRIZE_ESCROW_SO_SHA256 when set.
//!
//! Each test binary writes one line to stderr per program naming the file and its sha256, past
//! libtest's output capture, so a run's log records which bytes it tested.

#![allow(dead_code, reason = "each test file uses its own subset")]

use std::{io::Write, path::PathBuf, sync::OnceLock};

/// Where side_market's release build lands.
pub const RELEASE_SO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/deploy/side_market.so"
);

/// Where prize_escrow's release build lands.
pub const PRIZE_ESCROW_RELEASE_SO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/deploy/prize_escrow.so"
);

fn env_path(var: &str, default: &str) -> PathBuf {
    match std::env::var(var) {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path.trim()),
        _ => PathBuf::from(default),
    }
}

fn env_pin(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|sha| sha.trim().to_ascii_lowercase())
        .filter(|sha| !sha.is_empty())
}

/// The file the tests load: SIDE_MARKET_SO, or RELEASE_SO.
pub fn path() -> PathBuf {
    env_path("SIDE_MARKET_SO", RELEASE_SO)
}

/// The sha256 the bytes must have (SIDE_MARKET_SO_SHA256), or None when unset.
pub fn pinned_sha256() -> Option<String> {
    env_pin("SIDE_MARKET_SO_SHA256")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    solana_sha256_hasher::hash(bytes)
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `bytes` match the pin `expected` (a 64-digit hex sha256) named by `var`; every bytes
/// do with no pin.
pub fn check(bytes: &[u8], expected: Option<&str>, var: &str) -> Result<String, String> {
    let actual = sha256_hex(bytes);
    let Some(expected) = expected else {
        return Ok(actual);
    };
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "{var} {expected:?} is not a sha256 (64 hex digits)"
        ));
    }
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "its sha256 is {actual}, not the pinned {var} {expected}: these are not the bytes being landed"
        ));
    }
    Ok(actual)
}

fn load(name: &str, path: PathBuf, var: &str, hint: &str) -> Vec<u8> {
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}: {hint}", path.display()));
    let pin = env_pin(var);
    match check(&bytes, pin.as_deref(), var) {
        Ok(sha) => {
            let pinned = if pin.is_some() {
                format!("= {var}")
            } else {
                format!("({var} unset: not pinned)")
            };
            let _ = writeln!(
                std::io::stderr(),
                "{name} {} ({} bytes) sha256 {sha} {pinned}",
                path.display(),
                bytes.len()
            );
        }
        Err(e) => panic!("{}: {e}", path.display()),
    }
    bytes
}

/// side_market's bytes, read once per test binary and held to the pin; panics (so every test
/// that loads them fails) when the file is missing or the pin differs.
pub fn bytes() -> &'static [u8] {
    static SO: OnceLock<Vec<u8>> = OnceLock::new();
    SO.get_or_init(|| {
        load(
            "side_market.so",
            path(),
            "SIDE_MARKET_SO_SHA256",
            "build it with `node scripts/chain/build.ts release --program side_market`, or name the pair under test with SIDE_MARKET_SO",
        )
    })
}

/// prize_escrow's bytes, for the escrow cross-check.
pub fn prize_escrow_bytes() -> &'static [u8] {
    static SO: OnceLock<Vec<u8>> = OnceLock::new();
    SO.get_or_init(|| {
        load(
            "prize_escrow.so",
            env_path("PRIZE_ESCROW_SO", PRIZE_ESCROW_RELEASE_SO),
            "PRIZE_ESCROW_SO_SHA256",
            "build it with `node scripts/chain/build.ts release`, or name a release build with PRIZE_ESCROW_SO",
        )
    })
}
