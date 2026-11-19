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

/// D7: before lock_ts, the operator scratches a live entry replaced, rejected or withdrawn
/// (math::scratch): it is settled as an entry that did not win. Its YES shares are owed 0 and its
/// NO shares 1 base unit each (yes_value 0, no_value u64::MAX: math::SCRATCH_YES, SCRATCH_NO),
/// whatever anyone traded on it or on another outcome, before or after; F takes the N_k its NO
/// shares are owed as they leave N, and the max-q term loses k, so the liability never rises.
/// Its legs are paid when the market settles, final or void. FIELD absorbs it (math::scratch):
/// its base mass when q_k >= 0 (C falls, the house keeps it), its current weight when q_k < 0 (C
/// and every other price stay put, so the house's guarantee is never spent on its NO holders),
/// and FIELD's mass_ppm grows by math::scratch_mass_ppm (k's mass, or the part of it k's weight
/// still held, rounded down: the live masses then sum to at most 1,000,000).
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
    let merged_mass = {
        let o = &market.outcomes[k];
        math::scratch_mass_ppm(o.mass_ppm, o.q, b)?
    };
    let s = math::scratch(&mut market.outcomes, k, field, base, sum, b)?;
    market.base = s.base;
    market.sum = s.sum;

    let o = &mut market.outcomes[k];
    o.yes_value = s.yes;
    o.no_value = s.no;
    let owed = math::value_of(o.yes_shares()?, s.yes)
        .checked_add(math::value_of(o.no_shares, s.no))
        .ok_or(SideMarketError::MathOverflow)?;
    let (no_shares, q) = (o.no_shares, o.q);
    market.scratch_owed = market
        .scratch_owed
        .checked_add(owed)
        .ok_or(SideMarketError::MathOverflow)?;
    market.no_total = market
        .no_total
        .checked_sub(no_shares)
        .ok_or(SideMarketError::MathOverflow)?;
    market.outcomes[field].mass_ppm = market.outcomes[field]
        .mass_ppm
        .checked_add(merged_mass)
        .ok_or(SideMarketError::MathOverflow)?;
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
