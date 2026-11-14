use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED},
    error::SideMarketError,
    events::MarketStatusChanged,
    instructions::require_status,
    state::{Config, Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct Resume<'info> {
    /// The Config's admin, or its operator for a halt that was not the guardian's.
    pub signer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,
}

/// Lifts a halt: Halted back to Open. The admin always may; the operator only when the guardian
/// did not halt it.
pub fn handle_resume(ctx: Context<Resume>) -> Result<()> {
    let signer = ctx.accounts.signer.key();
    let config = &ctx.accounts.config;
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Halted])?;
    require!(
        signer == config.admin || (signer == config.operator && !market.halted_by_guardian),
        SideMarketError::UnauthorizedResume
    );
    market.status = MarketStatus::Open;
    market.halted_by_guardian = false;
    emit_cpi!(MarketStatusChanged {
        market: market.key(),
        by: signer,
        status: MarketStatus::Open,
    });
    Ok(())
}
