use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::{
    constants::{CONFIG_SEED, FEES_SEED, MARKET_SEED},
    error::SideMarketError,
    events::FeesSwept,
    instructions::transfer_from_market,
    state::{Config, Market},
};

#[event_cpi]
#[derive(Accounts)]
pub struct SweepFees<'info> {
    #[account(address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    #[account(seeds = [MARKET_SEED, market.competition_id.as_ref()], bump = market.bump)]
    pub market: Account<'info, Market>,

    #[account(address = market.mint @ SideMarketError::InvalidMint)]
    pub mint: Account<'info, Mint>,

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

    /// A token account of the Config's house.
    #[account(
        mut,
        token::mint = mint,
        token::authority = config.house,
        token::token_program = token_program,
    )]
    pub house_token: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

/// The operator moves the fees account's whole balance to the house. Fees sit outside the D6
/// invariant, so this never touches what the vault owes. Any status.
pub fn handle_sweep_fees(ctx: Context<SweepFees>) -> Result<()> {
    let a = &ctx.accounts;
    let amount = a.fees.amount;
    transfer_from_market(
        &a.token_program,
        &a.fees,
        a.house_token.to_account_info(),
        &a.mint,
        &a.market,
        amount,
    )?;
    emit_cpi!(FeesSwept {
        market: ctx.accounts.market.key(),
        amount,
    });
    Ok(())
}
