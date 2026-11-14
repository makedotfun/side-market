use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, FIRST_ENTRY_ID, MARKET_SEED, MAX_LIVE_ENTRIES, MAX_OUTCOMES},
    error::SideMarketError,
    events::OutcomeAdded,
    instructions::require_status,
    math,
    state::{Config, Market, MarketStatus, Outcome, OutcomeKind, OutcomeStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct AddOutcome<'info> {
    /// The Config's operator: pays the market's growth.
    #[account(mut, address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(
        mut,
        seeds = [MARKET_SEED, market.competition_id.as_ref()],
        bump = market.bump,
        realloc = Market::space(market.outcomes.len() + 1),
        realloc::payer = operator,
        realloc::zero = false,
    )]
    pub market: Account<'info, Market>,

    pub system_program: Program<'info, System>,
}

/// D3: lists entry `outcome_id` (above every id listed before, from 2) with `mass_ppm` split from
/// FIELD (math::join, rounded so C never rises): the entry's mass and FIELD's rest each at least
/// m_floor, at most MAX_LIVE_ENTRIES live entries. Open or Halted, before or after lock_ts.
pub fn handle_add_outcome(ctx: Context<AddOutcome>, outcome_id: u16, mass_ppm: u32) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
    require!(
        outcome_id >= FIRST_ENTRY_ID && outcome_id > market.last_outcome_id,
        SideMarketError::InvalidOutcomeId
    );
    require!(
        market.live_entries < MAX_LIVE_ENTRIES,
        SideMarketError::TooManyEntries
    );
    require!(
        market.outcomes.len() < usize::from(MAX_OUTCOMES),
        SideMarketError::TooManyOutcomes
    );
    let field = market.field_index()?;
    let field_mass = market.outcomes[field].mass_ppm;
    require!(
        mass_ppm >= market.m_floor_ppm
            && field_mass
                .checked_sub(mass_ppm)
                .is_some_and(|rest| rest >= market.m_floor_ppm),
        SideMarketError::MassBelowFloor
    );

    market.outcomes.push(Outcome {
        id: outcome_id,
        kind: OutcomeKind::Entry,
        status: OutcomeStatus::Live,
        mass_ppm,
        theta: 0,
        q: 0,
        no_shares: 0,
        w: 0,
        yes_value: 0,
        no_value: 0,
    });
    let new_index = market.outcomes.len() - 1;
    let (b, base, sum) = (market.b, market.base, market.sum);
    let (base, sum) = math::join(
        &mut market.outcomes,
        field,
        new_index,
        mass_ppm,
        base,
        sum,
        b,
    )?;
    market.base = base;
    market.sum = sum;
    market.outcomes[field].mass_ppm = field_mass - mass_ppm;
    market.live_entries += 1;
    market.last_outcome_id = outcome_id;
    if market.status == MarketStatus::Halted {
        market.freeze_marks()?;
    }

    emit_cpi!(OutcomeAdded {
        market: market.key(),
        outcome_id,
        mass_ppm,
        field_mass_ppm: market.outcomes[field].mass_ppm,
        theta: market.outcomes[new_index].theta,
        field_theta: market.outcomes[field].theta,
        base,
        sum,
    });
    Ok(())
}
