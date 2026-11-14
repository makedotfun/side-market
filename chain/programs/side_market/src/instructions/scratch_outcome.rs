use anchor_lang::prelude::*;
use anchor_spl::token::TokenAccount;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED, VAULT_SEED},
    error::SideMarketError,
    events::OutcomeScratched,
    instructions::{now, require_status},
    math,
    state::{Config, Market, MarketStatus, OutcomeKind},
};

#[event_cpi]
#[derive(Accounts)]
pub struct ScratchOutcome<'info> {
    #[account(address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    /// The market's vault (re-derived): D6 is asserted from its balance.
    #[account(
        seeds = [VAULT_SEED, market.key().as_ref()],
        bump = market.vault_bump,
        token::mint = market.mint,
        token::authority = market,
    )]
    pub vault: Account<'info, TokenAccount>,
}

/// D7: before lock_ts, the operator unwinds a live entry replaced, rejected or withdrawn at the
/// AMM (math::scratch): R = C(q) - C(q - q_k e_k) bracketed in Q64.64, one value v = R / q_k
/// rounded the house's way (its price w_k / sum when q_k is 0); YES shares on it are owed v each
/// and NO shares 1 - v, accrued to F (floor(Y v) + floor(N (1 - v)) <= R + N_k, at least what
/// every leg is paid); its NO shares leave N and its mass returns to FIELD.
pub fn handle_scratch_outcome(ctx: Context<ScratchOutcome>, outcome_id: u16) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
    require!(now()? < market.lock_ts, SideMarketError::ScratchAfterLock);
    let k = market.index_of(outcome_id)?;
    require!(
        market.outcomes[k].kind == OutcomeKind::Entry,
        SideMarketError::NotAnEntry
    );
    require!(
        market.outcomes[k].is_live(),
        SideMarketError::OutcomeNotLive
    );
    let field = market.field_index()?;
    let (b, base, sum) = (market.b, market.base, market.sum);
    let s = math::scratch(&mut market.outcomes, k, field, base, sum, b)?;
    market.base = s.base;
    market.sum = s.sum;

    let o = &mut market.outcomes[k];
    o.yes_value = s.yes;
    o.no_value = s.no;
    let owed = math::value_of(o.yes_shares()?, s.yes)
        .checked_add(math::value_of(o.no_shares, s.no))
        .ok_or(SideMarketError::MathOverflow)?;
    let (no_shares, q, mass) = (o.no_shares, o.q, o.mass_ppm);
    market.scratch_owed = market
        .scratch_owed
        .checked_add(owed)
        .ok_or(SideMarketError::MathOverflow)?;
    market.no_total = market
        .no_total
        .checked_sub(no_shares)
        .ok_or(SideMarketError::MathOverflow)?;
    market.outcomes[field].mass_ppm += mass;
    market.live_entries -= 1;
    if market.status == MarketStatus::Halted {
        market.freeze_marks()?;
    }
    market.assert_solvent(ctx.accounts.vault.amount)?;

    emit_cpi!(OutcomeScratched {
        market: market.key(),
        outcome_id,
        q,
        no_shares,
        yes_value: s.yes,
        no_value: s.no,
        owed,
        field_mass_ppm: market.outcomes[field].mass_ppm,
        base: s.base,
        sum: s.sum,
    });
    Ok(())
}
