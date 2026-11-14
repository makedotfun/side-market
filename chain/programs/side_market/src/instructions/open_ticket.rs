use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED, TICKET_SEED},
    error::SideMarketError,
    events::TicketOpened,
    instructions::{now, require_status},
    state::{Config, Market, MarketStatus, Ticket},
};

#[event_cpi]
#[derive(Accounts)]
#[instruction(wallet: Pubkey)]
pub struct OpenTicket<'info> {
    /// The Config's operator: pays the ticket's rent, after the server's eligibility check.
    #[account(mut, address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    /// PDA ["ticket", market, wallet].
    #[account(
        init,
        payer = operator,
        space = Ticket::space(0),
        seeds = [TICKET_SEED, market.key().as_ref(), wallet.as_ref()],
        bump,
    )]
    pub ticket: Account<'info, Ticket>,

    pub system_program: Program<'info, System>,
}

/// D9: the operator opens `wallet`'s ticket (once per market) while the market is Open or
/// Halted. Without one a wallet cannot buy.
pub fn handle_open_ticket(ctx: Context<OpenTicket>, wallet: Pubkey) -> Result<()> {
    require!(wallet != Pubkey::default(), SideMarketError::TicketMismatch);
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Open, MarketStatus::Halted])?;
    market.tickets = market
        .tickets
        .checked_add(1)
        .ok_or(SideMarketError::MathOverflow)?;
    let market_key = market.key();
    ctx.accounts.ticket.set_inner(Ticket {
        bump: ctx.bumps.ticket,
        revoked: false,
        market: market_key,
        wallet,
        payer: ctx.accounts.operator.key(),
        opened_at: now()?,
        legs: Vec::new(),
    });
    emit_cpi!(TicketOpened {
        market: market_key,
        ticket: ctx.accounts.ticket.key(),
        wallet,
    });
    Ok(())
}
