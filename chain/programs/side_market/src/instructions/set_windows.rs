use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED},
    error::SideMarketError,
    events::WindowsSet,
    instructions::require_status,
    state::{Config, Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetWindows<'info> {
    #[account(address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,
}

/// D8: the operator sets both trading windows, [bounty_from, bounty_until) and [race_from,
/// race_until), from the toggles (0, 0 closes one). Each must have from <= until, and the bounty
/// window ends by lock_ts. Open or Halted markets only.
pub fn handle_set_windows(
    ctx: Context<SetWindows>,
    bounty_from: i64,
    bounty_until: i64,
    race_from: i64,
    race_until: i64,
) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
    require!(
        0 <= bounty_from && bounty_from <= bounty_until && bounty_until <= market.lock_ts,
        SideMarketError::InvalidWindow
    );
    require!(
        0 <= race_from && race_from <= race_until,
        SideMarketError::InvalidWindow
    );
    market.bounty_from = bounty_from;
    market.bounty_until = bounty_until;
    market.race_from = race_from;
    market.race_until = race_until;
    emit_cpi!(WindowsSet {
        market: market.key(),
        bounty_from,
        bounty_until,
        race_from,
        race_until,
    });
    Ok(())
}
