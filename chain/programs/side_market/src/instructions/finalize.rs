use anchor_lang::prelude::*;
use anchor_spl::token::TokenAccount;

use crate::{
    constants::{MARKET_SEED, VAULT_SEED},
    error::SideMarketError,
    events::MarketFinalized,
    instructions::{as_u64, now, require_status},
    state::{Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct Finalize<'info> {
    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    /// The market's vault (re-derived): D6 in the winning world is asserted from its balance.
    #[account(
        seeds = [VAULT_SEED, market.key().as_ref()],
        bump = market.vault_bump,
        token::mint = market.mint,
        token::authority = market,
    )]
    pub vault: Account<'info, TokenAccount>,
}

/// D10: permissionless once the dispute delay has passed: the proposed winner stands (Final).
pub fn handle_finalize(ctx: Context<Finalize>) -> Result<()> {
    let at = now()?;
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Proposed])?;
    require!(at >= market.dispute_ends_at, SideMarketError::DisputeOpen);
    market.status = MarketStatus::Final;
    market.resolved_at = at;
    let vault = ctx.accounts.vault.amount;
    market.assert_solvent(vault)?;
    let liability = as_u64(market.liability()?);
    emit_cpi!(MarketFinalized {
        market: market.key(),
        winner: market.winner,
        result_sha256: market.result_sha256,
        liability,
        vault,
    });
    Ok(())
}
