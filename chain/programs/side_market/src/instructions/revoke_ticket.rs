use anchor_lang::prelude::*;

use crate::{
    constants::{CONFIG_SEED, MARKET_SEED, TICKET_SEED},
    error::SideMarketError,
    events::TicketRevoked,
    state::{Config, Market, Ticket},
};

#[event_cpi]
#[derive(Accounts)]
pub struct RevokeTicket<'info> {
    #[account(address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    #[account(
        mut,
        seeds = [TICKET_SEED, market.key().as_ref(), ticket.wallet.as_ref()],
        bump = ticket.bump,
        has_one = market @ SideMarketError::TicketMismatch,
    )]
    pub ticket: Account<'info, Ticket>,
}

/// D9: the operator stops a ticket's buys (`revoked` true: a newly barred account, or a key
/// whose `bet` scope was withdrawn), or lets them again (false). Sells are never stopped, and
/// nothing is sold for the wallet.
pub fn handle_revoke_ticket(ctx: Context<RevokeTicket>, revoked: bool) -> Result<()> {
    let ticket = &mut ctx.accounts.ticket;
    ticket.revoked = revoked;
    emit_cpi!(TicketRevoked {
        market: ticket.market,
        ticket: ticket.key(),
        wallet: ticket.wallet,
        revoked,
    });
    Ok(())
}
