//! make.fun side market: per-entry YES/NO prediction markets in USDC (Design D1-D14 of the
//! on-chain side bets plan).
//!
//! One market per competition (PDA ["market", competition_id]) prices its outcomes with a
//! mass-weighted LMSR (math.rs, Q64.64, mirrored bit for bit by server/side-market/math.ts):
//! every listed entry (ids from 2, never reused, at most MAX_LIVE_ENTRIES live), NONE (id 0, no
//! entry wins) and FIELD (id 1, an unlisted entry wins: never traded, the mass every join takes
//! from and every scratch returns to). The house deposits ceil(b ln(1/m_floor)) + 1,000 base
//! units at open, its worst-case loss plus a cushion. A wallet trades YES and NO shares of any
//! tradeable outcome through its Ticket (["ticket", market, wallet], opened by the operator after
//! the server's eligibility check): pays round up, receipts round down, and a trade never takes a
//! position across zero. The vault is held to D6 at the end of every value-moving instruction:
//! vault >= F + N + max(0, max q_j), the worst case of every world (F the scratch liabilities, N
//! the NO shares).
//!
//! Before lock_ts (the competition deadline) the operator may scratch an entry replaced, rejected
//! or withdrawn: it is unwound at the AMM price and its legs are paid fixed values. Buys land
//! only inside the windows set_windows opens (bounty phase, live race). A position can be closed
//! whenever the market is open: a sell (a trade that shrinks the position) lands inside a window
//! and also outside every window, until the moment set_sell_until names (when the result becomes
//! public; unset, until the market is proposed) and never at or after void_after, so a sell and a
//! permissionless void never land together. A halted market trades nothing. After the windows
//! and lock_ts pass the resolver proposes the winner (with a prize escrow's result cross-checked when the market names
//! one); the arbiter may override or void during Config.min_dispute_secs; anyone finalizes after
//! it, and anyone may void a market still unresolved 14 days after lock_ts. A void pays frozen
//! marks (YES p, NO 1 - p), never refunds at cost. settle_ticket pays every ticket; close_market
//! returns the surplus and the fees to the house.
//!
//! Compute (tests/compute.rs, release build on LiteSVM, measured 2026-11-13): a trade at 66
//! outcomes (64 live entries, NONE, FIELD) takes about 80k CU (a first buy on a new leg 82k, a
//! sale 80k); one that rebases the whole book about 270k, and 313k on the largest book
//! (MAX_OUTCOMES slots, 95 legs); halt 138k and void 147k at 66 outcomes. Every figure is under
//! 1.2M, so the maximum of 64 live entries stands; a trade can exceed the default 200k only when it
//! rebases, so a client sets a ComputeBudget limit (400k covers it) on every trade.
//!
//! Every instruction emits its event through a self-CPI (emit_cpi!), which a truncated log cannot
//! hide.

pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod math;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use error::*;
pub use events::*;
pub use instructions::*;
pub use state::*;

declare_id!("D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH");

// Where a security researcher reports a vulnerability (https://github.com/neodyme-labs/solana-security-txt).
// The build stamps the source revision (build.rs reads SOURCE_REVISION or GITHUB_SHA), a commit
// of the platform repository.
#[cfg(not(feature = "no-entrypoint"))]
solana_security_txt::security_txt! {
    name: "make.fun side market",
    project_url: "https://make.fun",
    contacts: "email:security@make.fun",
    policy: "Report vulnerabilities privately to security@make.fun, never in a public issue or post.",
    source_code: "https://github.com/makedotfun/side-market",
    source_revision: env!("SIDE_MARKET_SOURCE_REVISION")
}

#[program]
pub mod side_market {
    use super::*;

    // ------------------------------------------------------------------ the Config

    /// The program's upgrade authority creates the Config at ["config"], once, with its roles,
    /// the mint (classic SPL Token, 6 decimals), the escrow program, the dispute delay and the fee
    /// cap.
    pub fn init_config(ctx: Context<InitConfig>, params: ConfigParams) -> Result<()> {
        instructions::init_config::handle_init_config(ctx, params)
    }

    /// The admin replaces every Config setting but the mint.
    pub fn set_config(ctx: Context<SetConfig>, params: ConfigParams) -> Result<()> {
        instructions::set_config::handle_set_config(ctx, params)
    }

    // ------------------------------------------------------------------ the market

    /// The operator opens the market for `competition_id` (NONE and FIELD only), the house
    /// depositing ceil(b ln(1/m_floor)) + 1,000 base units into its vault.
    pub fn open_market(
        ctx: Context<OpenMarket>,
        competition_id: [u8; 16],
        args: OpenMarketArgs,
    ) -> Result<()> {
        instructions::open_market::handle_open_market(ctx, competition_id, args)
    }

    /// The operator sets the bounty and race trading windows (0, 0 closes one).
    pub fn set_windows(
        ctx: Context<SetWindows>,
        bounty_from: i64,
        bounty_until: i64,
        race_from: i64,
        race_until: i64,
    ) -> Result<()> {
        instructions::set_windows::handle_set_windows(
            ctx,
            bounty_from,
            bounty_until,
            race_from,
            race_until,
        )
    }

    /// The operator sets when the competition's result becomes public (at or after lock_ts; 0
    /// clears it): sells outside every window land until then, never past void_after.
    pub fn set_sell_until(ctx: Context<SetSellUntil>, sell_until: i64) -> Result<()> {
        instructions::set_sell_until::handle_set_sell_until(ctx, sell_until)
    }

    /// The operator lists an entry, its mass split from FIELD so C never rises.
    pub fn add_outcome(ctx: Context<AddOutcome>, outcome_id: u16, mass_ppm: u32) -> Result<()> {
        instructions::add_outcome::handle_add_outcome(ctx, outcome_id, mass_ppm)
    }

    /// The operator unwinds an entry at the AMM before lock_ts.
    pub fn scratch_outcome(ctx: Context<ScratchOutcome>, outcome_id: u16) -> Result<()> {
        instructions::scratch_outcome::handle_scratch_outcome(ctx, outcome_id)
    }

    // ------------------------------------------------------------------ tickets and trades

    /// The operator opens `wallet`'s ticket.
    pub fn open_ticket(ctx: Context<OpenTicket>, wallet: Pubkey) -> Result<()> {
        instructions::open_ticket::handle_open_ticket(ctx, wallet)
    }

    /// The operator stops (or restores) a ticket's buys; sells are never stopped.
    pub fn revoke_ticket(ctx: Context<RevokeTicket>, revoked: bool) -> Result<()> {
        instructions::revoke_ticket::handle_revoke_ticket(ctx, revoked)
    }

    /// The wallet moves its leg on `outcome_id` by `delta` shares (+ YES, - NO), paying at most
    /// or receiving at least `limit` base units, fee included: a buy inside a window, a sell
    /// inside a window or whenever sells are open (before sell_until and void_after).
    pub fn trade(ctx: Context<Trade>, outcome_id: u16, delta: i64, limit: u64) -> Result<()> {
        instructions::trade::handle_trade(ctx, outcome_id, delta, limit)
    }

    // ------------------------------------------------------------------ halts and resolution

    /// The operator or the guardian halts an Open market, freezing its marks.
    pub fn halt(ctx: Context<Halt>) -> Result<()> {
        instructions::halt::handle_halt(ctx)
    }

    /// The admin (or the operator, for its own halt) resumes a Halted market.
    pub fn resume(ctx: Context<Resume>) -> Result<()> {
        instructions::resume::handle_resume(ctx)
    }

    /// The resolver proposes the winner and the result hash once trading has ended (never under
    /// the guardian's halt).
    pub fn propose(ctx: Context<Propose>, winner: u16, result_sha256: [u8; 32]) -> Result<()> {
        instructions::propose::handle_propose(ctx, winner, result_sha256)
    }

    /// The arbiter replaces the proposed winner during the dispute delay.
    pub fn override_result(
        ctx: Context<OverrideResult>,
        winner: u16,
        result_sha256: [u8; 32],
    ) -> Result<()> {
        instructions::override_result::handle_override_result(ctx, winner, result_sha256)
    }

    /// The arbiter voids a market not yet final, or anyone an unresolved one after void_after
    /// while no window is open: every leg is paid at its frozen mark or scratch value.
    pub fn void_market(ctx: Context<VoidMarket>) -> Result<()> {
        instructions::void_market::handle_void_market(ctx)
    }

    /// Anyone, after the dispute delay: the proposed winner stands.
    pub fn finalize(ctx: Context<Finalize>) -> Result<()> {
        instructions::finalize::handle_finalize(ctx)
    }

    /// Anyone, once Final or Void: pays a ticket to a token account its wallet owns (its
    /// associated token account, created when missing, or any other of the mint) and closes it.
    pub fn settle_ticket(ctx: Context<SettleTicket>) -> Result<()> {
        instructions::settle_ticket::handle_settle_ticket(ctx)
    }

    /// The operator sends the market's fees to the house.
    pub fn sweep_fees(ctx: Context<SweepFees>) -> Result<()> {
        instructions::sweep_fees::handle_sweep_fees(ctx)
    }

    /// The operator closes a settled market: surplus and fees to the house, rent to its payer.
    pub fn close_market(ctx: Context<CloseMarket>) -> Result<()> {
        instructions::close_market::handle_close_market(ctx)
    }
}
