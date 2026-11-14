use anchor_lang::prelude::*;

/// Seed of the Config PDA: `["config"]`.
#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

/// Seed of a Market PDA: `["market", competition_id]`, competition_id being the competition's
/// 16-byte UUID: one market per competition.
#[constant]
pub const MARKET_SEED: &[u8] = b"market";

/// Seed of a market's vault, a classic SPL token account at `["vault", market]` whose authority
/// is the market.
#[constant]
pub const VAULT_SEED: &[u8] = b"vault";

/// Seed of a market's fees account, a classic SPL token account at `["fees", market]` whose
/// authority is the market: trade fees land there, outside the D6 invariant.
#[constant]
pub const FEES_SEED: &[u8] = b"fees";

/// Seed of a Ticket PDA: `["ticket", market, wallet]`.
#[constant]
pub const TICKET_SEED: &[u8] = b"ticket";

/// Outcome id of NONE: no entry wins, or the prize is not awarded. Tradeable.
#[constant]
pub const NONE_ID: u16 = 0;

/// Outcome id of FIELD: an entry that was never listed wins. Never tradeable: the mass every join
/// takes from and every scratch returns to.
#[constant]
pub const FIELD_ID: u16 = 1;

/// The first entry's outcome id; ids rise from here and are never reused.
#[constant]
pub const FIRST_ENTRY_ID: u16 = 2;

/// D1: the most entries live at once. A trade at 66 outcomes (64 entries, NONE, FIELD) is the
/// compute-unit ceiling the program's tests measure.
#[constant]
pub const MAX_LIVE_ENTRIES: u16 = 64;

/// The most outcomes a market ever holds, NONE, FIELD and scratched entries included (a scratched
/// entry keeps its slot: its values pay its legs at settlement): 64 live entries and 30 scratched
/// ones. Bounded by the program's 32 KiB heap: Borsh reads a Vec of 96 Outcomes (80 bytes each in
/// memory) in two allocations (51, then 102 slots: 12.2 KiB), and a Ticket's 95 legs in one, so the
/// largest trade's market, ticket and book stay well inside it.
#[constant]
pub const MAX_OUTCOMES: u16 = 96;

/// The most legs a Ticket holds: one per tradeable outcome.
#[constant]
pub const MAX_LEGS: u16 = MAX_OUTCOMES - 1;

/// The smallest m_floor (the minimum world share), in ppm.
#[constant]
pub const MIN_M_FLOOR_PPM: u32 = 100;

/// The largest m_floor, in ppm.
#[constant]
pub const MAX_M_FLOOR_PPM: u32 = 250_000;

/// D5: the highest fee rate any Config may allow, in ppm (7%).
#[constant]
pub const MAX_FEE_RATE_PPM: u32 = 70_000;

/// D11: a market not final by lock_ts + VOID_GRACE may be voided by anyone: 14 days.
#[constant]
pub const VOID_GRACE: i64 = 14 * 24 * 60 * 60;

/// The shortest dispute delay a Config accepts, in seconds.
#[constant]
pub const MIN_DISPUTE_SECS: i64 = 60;

/// The longest dispute delay a Config accepts, in seconds: 30 days.
#[constant]
pub const MAX_DISPUTE_SECS: i64 = 30 * 24 * 60 * 60;

/// The mint's decimals: USDC and the house test mint are both 6-dp.
#[constant]
pub const MINT_DECIMALS: u8 = 6;

/// The prize escrow account layout propose reads (chain/programs/prize_escrow/src/state.rs
/// Escrow): its Anchor discriminator, sha256("account:Escrow")[..8].
pub const ESCROW_DISCRIMINATOR: [u8; 8] = [31, 213, 123, 187, 186, 22, 218, 155];
/// Escrow.status: after the discriminator and bump, decimals (one byte each).
pub const ESCROW_STATUS_OFFSET: usize = 10;
/// Escrow.competition_id: after status, k and n.
pub const ESCROW_COMPETITION_ID_OFFSET: usize = 13;
/// Escrow.result_sha256: after competition_id, seven keys (sponsor, resolver, compliance,
/// arbiter, refund_to, mint, challenger) and three hashes (rules, entries root, payout root).
pub const ESCROW_RESULT_OFFSET: usize = 13 + 16 + 7 * 32 + 3 * 32;
/// EscrowStatus::Claimable.
pub const ESCROW_STATUS_CLAIMABLE: u8 = 3;
/// EscrowStatus::Closed.
pub const ESCROW_STATUS_CLOSED: u8 = 5;
