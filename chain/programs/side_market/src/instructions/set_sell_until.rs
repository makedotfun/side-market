use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED},
    error::SideMarketError,
    events::SellUntilSet,
    instructions::require_status,
    state::{Config, Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetSellUntil<'info> {
    #[account(address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,
}

/// The operator sets the moment the competition's result becomes public: sells outside every
/// window land until it (Market::sells_open), and never past void_after. 0 clears it (sells land
/// until the market is proposed); any other value is at or after lock_ts (InvalidWindow), so the
/// deadline-to-result stretch always stays sellable. Open or Halted markets only.
pub fn handle_set_sell_until(ctx: Context<SetSellUntil>, sell_until: i64) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
    require!(
        sell_until == 0 || sell_until >= market.lock_ts,
        SideMarketError::InvalidWindow
    );
    market.sell_until = sell_until;
    emit_cpi!(SellUntilSet {
        market: market.key(),
        sell_until,
    });
    Ok(())
}
