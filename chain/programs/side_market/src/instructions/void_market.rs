use anchor_lang::prelude::*;
use anchor_spl::token::TokenAccount;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED, VAULT_SEED},
    error::SideMarketError,
    events::MarketVoided,
    instructions::{as_u64, now, require_status},
    state::{Config, Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct VoidMarket<'info> {
    /// The Config's arbiter, or anyone once void_after has passed.
    pub signer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    /// The market's vault (re-derived): D6 at the marks is asserted from its balance.
    #[account(
        seeds = [VAULT_SEED, market.key().as_ref()],
        bump = market.vault_bump,
        token::mint = market.mint,
        token::authority = market,
    )]
    pub vault: Account<'info, TokenAccount>,
}

/// D11: voids a market that is not final. The arbiter may void an Open, Halted or Proposed one;
/// anyone may void an Open or Halted one (never a proposed result) from void_after on, while no
/// trading window is open (a buy and a void in one transaction would otherwise take the gap
/// between the buy's price and its frozen mark from the house; a sell outside every window lands
/// only before void_after, Market::sells_open, so it never shares one with this void). Every live
/// outcome's marks are frozen from the book as it stands (as halt froze them: the book does not
/// move while Halted), so void pays every live leg its mark (YES p, NO 1 - p) and every scratched
/// leg its scratch value. The vault must cover that (D6 at the marks).
pub fn handle_void_market(ctx: Context<VoidMarket>) -> Result<()> {
    let at = now()?;
    let by = ctx.accounts.signer.key();
    let market = &mut ctx.accounts.market;
    if by == ctx.accounts.config.arbiter {
        require_status(
            market.status,
            &[
                MarketStatus::Open,
                MarketStatus::Halted,
                MarketStatus::Proposed,
            ],
        )?;
    } else {
        require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
        require!(at >= market.void_after, SideMarketError::VoidNotYet);
        require!(!market.window_open(at), SideMarketError::VoidWhileTrading);
    }
    market.freeze_marks()?;
    market.status = MarketStatus::Void;
    market.resolved_at = at;
    let vault = ctx.accounts.vault.amount;
    market.assert_solvent(vault)?;
    let liability = as_u64(market.liability()?);
    emit_cpi!(MarketVoided {
        market: market.key(),
        by,
        liability,
        vault,
    });
    Ok(())
}
