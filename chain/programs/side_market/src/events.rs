//! The program's events (D14). Every state change emits its event with `emit_cpi!`: a self-CPI
//! signed by the event authority PDA (`["__event_authority"]`), whose instruction data is
//! `EVENT_IX_TAG_LE || the event's discriminator || its Borsh body`. The runtime records that
//! inner instruction in the transaction's metadata whatever the log holds, so a caller that floods
//! the log cannot hide an event, and only this program can sign as its event authority.

use anchor_lang::prelude::*;

use crate::state::{ConfigParams, MarketStatus};

/// init_config (initialized = true) or set_config: the Config's settings after it.
#[event]
pub struct ConfigSet {
    pub config: Pubkey,
    pub by: Pubkey,
    pub initialized: bool,
    pub mint: Pubkey,
    pub params: ConfigParams,
}

/// open_market: the market exists and its vault holds `deposit`.
#[event]
pub struct MarketOpened {
    pub market: Pubkey,
    pub competition_id: [u8; 16],
    pub escrow: Pubkey,
    pub b: u64,
    pub m_floor_ppm: u32,
    pub fee_rate_ppm: u32,
    pub deposit: u64,
    pub none_mass_ppm: u32,
    pub field_mass_ppm: u32,
    pub lock_ts: i64,
    pub void_after: i64,
}

/// set_windows: the trading windows after it.
#[event]
pub struct WindowsSet {
    pub market: Pubkey,
    pub bounty_from: i64,
    pub bounty_until: i64,
    pub race_from: i64,
    pub race_until: i64,
}

/// add_outcome: an entry joined, taking `mass_ppm` from FIELD.
#[event]
pub struct OutcomeAdded {
    pub market: Pubkey,
    pub outcome_id: u16,
    pub mass_ppm: u32,
    pub field_mass_ppm: u32,
    pub theta: i128,
    pub field_theta: i128,
    pub base: i128,
    pub sum: u128,
}

/// scratch_outcome: an entry unwound at the AMM (D7).
#[event]
pub struct OutcomeScratched {
    pub market: Pubkey,
    pub outcome_id: u16,
    /// Its net shares and NO shares when scratched.
    pub q: i64,
    pub no_shares: u64,
    /// What a YES and a NO share are paid (Q0.64).
    pub yes_value: u64,
    pub no_value: u64,
    /// What it adds to F.
    pub owed: u64,
    pub field_mass_ppm: u32,
    pub base: i128,
    pub sum: u128,
}

/// open_ticket.
#[event]
pub struct TicketOpened {
    pub market: Pubkey,
    pub ticket: Pubkey,
    pub wallet: Pubkey,
}

/// revoke_ticket.
#[event]
pub struct TicketRevoked {
    pub market: Pubkey,
    pub ticket: Pubkey,
    pub wallet: Pubkey,
    pub revoked: bool,
}

/// trade: one leg moved by `delta` (D4).
#[event]
pub struct Traded {
    pub market: Pubkey,
    pub wallet: Pubkey,
    pub outcome_id: u16,
    pub delta: i64,
    /// The leg's position after it.
    pub position: i64,
    /// What the wallet paid into the vault (a buy), before the fee.
    pub pay: u64,
    /// What the vault paid the wallet (a sell), the fee already taken.
    pub receive: u64,
    pub fee: u64,
    /// The outcome's q and the market's N, base and sum after it.
    pub q: i64,
    pub no_total: u64,
    pub base: i128,
    pub sum: u128,
    pub vault: u64,
}

/// halt or resume.
#[event]
pub struct MarketStatusChanged {
    pub market: Pubkey,
    pub by: Pubkey,
    pub status: MarketStatus,
}

/// propose (overridden = false) or override_result (true).
#[event]
pub struct ResultProposed {
    pub market: Pubkey,
    pub by: Pubkey,
    pub winner: u16,
    pub result_sha256: [u8; 32],
    pub dispute_ends_at: i64,
    pub overridden: bool,
}

/// void_market.
#[event]
pub struct MarketVoided {
    pub market: Pubkey,
    pub by: Pubkey,
    pub liability: u64,
    pub vault: u64,
}

/// finalize.
#[event]
pub struct MarketFinalized {
    pub market: Pubkey,
    pub winner: u16,
    pub result_sha256: [u8; 32],
    pub liability: u64,
    pub vault: u64,
}

/// settle_ticket: the wallet was paid and the ticket closed.
#[event]
pub struct TicketSettled {
    pub market: Pubkey,
    pub ticket: Pubkey,
    pub wallet: Pubkey,
    pub payout: u64,
    pub tickets_left: u32,
}

/// sweep_fees.
#[event]
pub struct FeesSwept {
    pub market: Pubkey,
    pub amount: u64,
}

/// close_market: the surplus and fees went to the house, the accounts closed.
#[event]
pub struct MarketClosed {
    pub market: Pubkey,
    pub surplus: u64,
    pub fees: u64,
    /// Unpaid scratch dust (F) left when it closed.
    pub scratch_dust: u64,
}

/// set_sell_until: sells outside every window land until `sell_until` (0: until the market is
/// proposed), never past void_after.
#[event]
pub struct SellUntilSet {
    pub market: Pubkey,
    pub sell_until: i64,
}
