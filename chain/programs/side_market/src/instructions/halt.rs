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
pub struct Halt<'info> {
    /// The Config's operator or guardian.
    #[account(
        constraint = signer.key() == config.operator || signer.key() == config.guardian
            @ SideMarketError::UnauthorizedHalt,
    )]
    pub signer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,
}

/// D11: the operator or the guardian halts an Open market: no trade until resume, and every live
/// outcome's marks frozen into it (Market::freeze_marks; a join or scratch while Halted refreezes
/// them). A guardian's halt only the admin lifts.
pub fn handle_halt(ctx: Context<Halt>) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open])?;
    market.status = MarketStatus::Halted;
    market.halted_by_guardian = ctx.accounts.signer.key() == ctx.accounts.config.guardian;
    market.freeze_marks()?;
    emit_cpi!(MarketStatusChanged {
        market: market.key(),
        by: ctx.accounts.signer.key(),
        status: MarketStatus::Halted,
    });
    Ok(())
}
