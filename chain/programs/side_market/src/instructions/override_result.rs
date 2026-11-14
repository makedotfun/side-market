use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED},
    error::SideMarketError,
    events::ResultProposed,
    instructions::{now, propose::check_winner, require_status},
    state::{Config, Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct OverrideResult<'info> {
    #[account(address = config.arbiter @ SideMarketError::UnauthorizedArbiter)]
    pub arbiter: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,
}

/// D10: during the dispute delay the arbiter replaces the proposed winner and result hash. The
/// delay's end stays where propose set it.
pub fn handle_override_result(
    ctx: Context<OverrideResult>,
    winner: u16,
    result_sha256: [u8; 32],
) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Proposed])?;
    require!(
        now()? < market.dispute_ends_at,
        SideMarketError::DisputeOver
    );
    require!(result_sha256 != [0; 32], SideMarketError::InvalidResult);
    check_winner(market, winner)?;
    market.winner = winner;
    market.result_sha256 = result_sha256;
    emit_cpi!(ResultProposed {
        market: market.key(),
        by: ctx.accounts.arbiter.key(),
        winner,
        result_sha256,
        dispute_ends_at: market.dispute_ends_at,
        overridden: true,
    });
    Ok(())
}
