use anchor_lang::{prelude::*, solana_program::program_pack::Pack};
use anchor_spl::{
    associated_token::{self, get_associated_token_address_with_program_id, AssociatedToken},
    token::{spl_token, Mint, Token, TokenAccount},
};

use crate::{
    constants::{MARKET_SEED, TICKET_SEED, VAULT_SEED},
    error::SideMarketError,
    events::TicketSettled,
    instructions::{require_status, transfer_from_market},
    state::{Market, MarketStatus, Ticket},
};

#[event_cpi]
#[derive(Accounts)]
pub struct SettleTicket<'info> {
    /// Anyone: pays the wallet's associated token account when it is created here.
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(mut, seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    #[account(address = market.mint @ SideMarketError::InvalidMint)]
    pub mint: Account<'info, Mint>,

    /// PDA ["vault", market].
    #[account(
        mut,
        seeds = [VAULT_SEED, market.key().as_ref()],
        bump = market.vault_bump,
        token::mint = mint,
        token::authority = market,
        token::token_program = token_program,
    )]
    pub vault: Account<'info, TokenAccount>,

    /// CHECK: the ticket's wallet (address pinned to ticket.wallet): owns the destination.
    #[account(address = ticket.wallet @ SideMarketError::TicketMismatch)]
    pub wallet: UncheckedAccount<'info>,

    /// Takes the payout: any classic token account of the market's mint the wallet owns, or the
    /// wallet's associated token account, created here when missing (caller pays). Unused, and
    /// never created, when the payout is 0.
    /// CHECK: check_destination: a token account of the mint the wallet owns, or its new ATA.
    #[account(mut)]
    pub wallet_token: UncheckedAccount<'info>,

    /// PDA ["ticket", market, wallet]: closed, its rent to its payer.
    #[account(
        mut,
        seeds = [TICKET_SEED, market.key().as_ref(), ticket.wallet.as_ref()],
        bump = ticket.bump,
        has_one = market @ SideMarketError::TicketMismatch,
        close = ticket_payer,
    )]
    pub ticket: Account<'info, Ticket>,

    /// CHECK: the ticket's payer (address pinned to ticket.payer): takes its rent back.
    #[account(mut, address = ticket.payer @ SideMarketError::PayerMismatch)]
    pub ticket_payer: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Makes `wallet_token` a destination the ticket's wallet owns, for a payout above 0:
///
/// - a classic SPL token account (owned by the token program) must hold the market's mint and
///   have the ticket's wallet as its owner (authority): any such account, not only the ATA, so a
///   wallet that reassigned its classic ATA's owner (SetAuthority AccountOwner, which classic SPL
///   Token allows) is still paid, and its ticket still closes;
/// - an address the token program does not own must be the wallet's canonical associated token
///   account, not created yet: the associated token program creates it here (idempotent; the
///   caller pays its rent);
/// - anything else, the vault included, is refused (TokenAccountMismatch).
fn check_destination<'info>(a: &SettleTicket<'info>) -> Result<()> {
    let to = &a.wallet_token;
    let wallet = a.ticket.wallet;
    require_keys_neq!(
        to.key(),
        a.vault.key(),
        SideMarketError::TokenAccountMismatch
    );
    if to.owner == &a.token_program.key() {
        let data = to.try_borrow_data()?;
        let account = spl_token::state::Account::unpack(&data[..])
            .map_err(|_| error!(SideMarketError::TokenAccountMismatch))?;
        require_keys_eq!(
            account.mint,
            a.market.mint,
            SideMarketError::TokenAccountMismatch
        );
        require_keys_eq!(account.owner, wallet, SideMarketError::TokenAccountMismatch);
        return Ok(());
    }
    let ata = get_associated_token_address_with_program_id(
        &wallet,
        &a.market.mint,
        &a.token_program.key(),
    );
    require!(
        to.key() == ata && to.owner == &a.system_program.key(),
        SideMarketError::TokenAccountMismatch
    );
    associated_token::create_idempotent(CpiContext::new(
        a.associated_token_program.key(),
        associated_token::Create {
            payer: a.caller.to_account_info(),
            associated_token: to.to_account_info(),
            authority: a.wallet.to_account_info(),
            mint: a.mint.to_account_info(),
            system_program: a.system_program.to_account_info(),
            token_program: a.token_program.to_account_info(),
        },
    ))
}

/// D12: permissionless once the market is Final or Void: pays the ticket's wallet every leg's due
/// (Market::leg_payout: Final pays a share of the winner's YES and of every other outcome's NO 1;
/// Void pays live legs their frozen marks; in both, a scratched entry's YES legs are paid 0 and its
/// NO legs 1 a share)
/// into a token account it owns (check_destination), takes the legs off the market's totals
/// (Market::release_leg) and closes the ticket. A payout of 0 moves nothing and creates no
/// account. D6 is asserted from the vault after the transfer.
pub fn handle_settle_ticket(ctx: Context<SettleTicket>) -> Result<()> {
    let market = &mut ctx.accounts.market;
    require_status(market.status, &[MarketStatus::Final, MarketStatus::Void])?;
    let mut payout: u64 = 0;
    for leg in ctx.accounts.ticket.legs.iter() {
        let paid = market.leg_payout(leg.outcome_id, leg.shares)?;
        market.release_leg(leg.outcome_id, leg.shares, paid)?;
        payout = payout
            .checked_add(paid)
            .ok_or(SideMarketError::MathOverflow)?;
    }
    market.tickets = market
        .tickets
        .checked_sub(1)
        .ok_or(SideMarketError::MathOverflow)?;
    let tickets_left = market.tickets;

    if payout > 0 {
        let a = &ctx.accounts;
        check_destination(a)?;
        transfer_from_market(
            &a.token_program,
            &a.vault,
            a.wallet_token.to_account_info(),
            &a.mint,
            &a.market,
            payout,
        )?;
        ctx.accounts.vault.reload()?;
    }
    let vault = ctx.accounts.vault.amount;
    ctx.accounts.market.assert_solvent(vault)?;

    emit_cpi!(TicketSettled {
        market: ctx.accounts.market.key(),
        ticket: ctx.accounts.ticket.key(),
        wallet: ctx.accounts.ticket.wallet,
        payout,
        tickets_left,
    });
    Ok(())
}
