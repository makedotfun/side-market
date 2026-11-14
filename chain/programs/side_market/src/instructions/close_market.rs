use anchor_lang::prelude::*;
use anchor_spl::token::{close_account, CloseAccount, Mint, Token, TokenAccount};

use crate::{
    constants::{CONFIG_SEED, FEES_SEED, MARKET_SEED, VAULT_SEED},
    error::SideMarketError,
    events::MarketClosed,
    instructions::{require_status, transfer_from_market},
    state::{Config, Market, MarketStatus},
};

#[event_cpi]
#[derive(Accounts)]
pub struct CloseMarket<'info> {
    #[account(address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    /// Closed, its rent to its payer.
    #[account(
        mut,
        seeds = [MARKET_SEED, market.competition_id.as_ref()],
        bump = market.bump,
        close = rent_to,
    )]
    pub market: Account<'info, Market>,

    /// CHECK: the market's payer (address pinned to market.payer): takes back every account's
    /// rent.
    #[account(mut, address = market.payer @ SideMarketError::PayerMismatch)]
    pub rent_to: UncheckedAccount<'info>,

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

    /// PDA ["fees", market].
    #[account(
        mut,
        seeds = [FEES_SEED, market.key().as_ref()],
        bump = market.fees_bump,
        token::mint = mint,
        token::authority = market,
        token::token_program = token_program,
    )]
    pub fees: Account<'info, TokenAccount>,

    /// A token account of the Config's house: takes the surplus and the fees.
    #[account(
        mut,
        token::mint = mint,
        token::authority = config.house,
        token::token_program = token_program,
    )]
    pub house_token: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

/// D12: once a Final or Void market has no ticket left, the operator sends the vault's surplus
/// and the fees to the house and closes the vault, the fees account and the market, every rent
/// to the market's payer.
pub fn handle_close_market(ctx: Context<CloseMarket>) -> Result<()> {
    let a = &ctx.accounts;
    require_status(a.market.status, &[MarketStatus::Final, MarketStatus::Void])?;
    require!(a.market.tickets == 0, SideMarketError::TicketsOpen);
    let surplus = a.vault.amount;
    let fees = a.fees.amount;
    let scratch_dust = a.market.scratch_owed;
    for (from, amount) in [(&a.vault, surplus), (&a.fees, fees)] {
        transfer_from_market(
            &a.token_program,
            from,
            a.house_token.to_account_info(),
            &a.mint,
            &a.market,
            amount,
        )?;
        let seeds = a.market.signer_seeds();
        let signer: &[&[&[u8]]] = &[&seeds];
        close_account(CpiContext::new_with_signer(
            a.token_program.key(),
            CloseAccount {
                account: from.to_account_info(),
                destination: a.rent_to.to_account_info(),
                authority: a.market.to_account_info(),
            },
            signer,
        ))?;
    }
    emit_cpi!(MarketClosed {
        market: ctx.accounts.market.key(),
        surplus,
        fees,
        scratch_dust,
    });
    Ok(())
}
