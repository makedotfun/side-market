use anchor_lang::prelude::*;

use crate::{
    constants::{
        CONFIG_SEED, ESCROW_COMPETITION_ID_OFFSET, ESCROW_DISCRIMINATOR, ESCROW_RESULT_OFFSET,
        ESCROW_STATUS_CLAIMABLE, ESCROW_STATUS_CLOSED, ESCROW_STATUS_OFFSET, FIELD_ID, MARKET_SEED,
        NONE_ID,
    },
    error::SideMarketError,
    events::ResultProposed,
    instructions::{now, require_status},
    state::{Config, Market, MarketStatus, OutcomeKind},
};

#[event_cpi]
#[derive(Accounts)]
pub struct Propose<'info> {
    #[account(address = config.resolver @ SideMarketError::UnauthorizedResolver)]
    pub resolver: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    /// CHECK: the competition's prize escrow when the market names one (its address, owner,
    /// discriminator, competition and result are checked in check_escrow); None otherwise.
    pub escrow: Option<UncheckedAccount<'info>>,
}

/// Whether `winner` may win `market`: NONE, FIELD, or a live entry (a scratched entry's legs are
/// already fixed; the server proposes FIELD for one).
pub(crate) fn check_winner(market: &Market, winner: u16) -> Result<()> {
    if winner == NONE_ID || winner == FIELD_ID {
        return Ok(());
    }
    let o = market
        .outcomes
        .iter()
        .find(|o| o.id == winner)
        .ok_or(SideMarketError::InvalidWinner)?;
    require!(
        o.kind == OutcomeKind::Entry && o.is_live(),
        SideMarketError::InvalidWinner
    );
    Ok(())
}

/// D10's escrow cross-check: the account is the market's escrow, owned by the Config's
/// escrow_program, an Escrow by its discriminator (chain/programs/prize_escrow/src/state.rs
/// layout: constants ESCROW_*), of the same competition, Claimable or Closed, with the same
/// result_sha256.
pub(crate) fn check_escrow(
    market: &Market,
    config: &Config,
    escrow: Option<&UncheckedAccount>,
    result_sha256: &[u8; 32],
) -> Result<()> {
    if market.escrow == Pubkey::default() {
        return Ok(());
    }
    let escrow = escrow.ok_or(SideMarketError::EscrowRequired)?;
    require_keys_eq!(escrow.key(), market.escrow, SideMarketError::EscrowMismatch);
    require_keys_eq!(
        *escrow.owner,
        config.escrow_program,
        SideMarketError::EscrowMismatch
    );
    let data = escrow.try_borrow_data()?;
    require!(
        data.len() >= ESCROW_RESULT_OFFSET + 32 && data[..8] == ESCROW_DISCRIMINATOR,
        SideMarketError::EscrowMismatch
    );
    let status = data[ESCROW_STATUS_OFFSET];
    require!(
        status == ESCROW_STATUS_CLAIMABLE || status == ESCROW_STATUS_CLOSED,
        SideMarketError::EscrowNotSettled
    );
    require!(
        data[ESCROW_COMPETITION_ID_OFFSET..ESCROW_COMPETITION_ID_OFFSET + 16]
            == market.competition_id,
        SideMarketError::EscrowNotSettled
    );
    require!(
        data[ESCROW_RESULT_OFFSET..ESCROW_RESULT_OFFSET + 32] == *result_sha256,
        SideMarketError::EscrowNotSettled
    );
    Ok(())
}

/// D10: once trading has ended (lock_ts and both windows past), the resolver proposes the winner
/// (NONE, FIELD or a live entry) and the result's sha256, with the escrow cross-check when the
/// market names one. The dispute delay (Config.min_dispute_secs, read now) starts. Never while
/// the guardian's halt stands (only the admin lifts it; the arbiter may still void).
pub fn handle_propose(ctx: Context<Propose>, winner: u16, result_sha256: [u8; 32]) -> Result<()> {
    let at = now()?;
    let config = &ctx.accounts.config;
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
    require!(
        !(market.status == MarketStatus::Halted && market.halted_by_guardian),
        SideMarketError::InvalidStatus
    );
    require!(market.trading_ended(at), SideMarketError::TradingNotEnded);
    require!(result_sha256 != [0; 32], SideMarketError::InvalidResult);
    check_winner(market, winner)?;
    check_escrow(market, config, ctx.accounts.escrow.as_ref(), &result_sha256)?;
    market.status = MarketStatus::Proposed;
    market.winner = winner;
    market.result_sha256 = result_sha256;
    market.proposed_at = at;
    market.dispute_ends_at = at
        .checked_add(config.min_dispute_secs)
        .ok_or(SideMarketError::MathOverflow)?;
    emit_cpi!(ResultProposed {
        market: market.key(),
        by: ctx.accounts.resolver.key(),
        winner,
        result_sha256,
        dispute_ends_at: market.dispute_ends_at,
        overridden: false,
    });
    Ok(())
}
