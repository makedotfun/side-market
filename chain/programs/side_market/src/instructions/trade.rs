use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::{
    constants::{CONFIG_SEED, FEES_SEED, MARKET_SEED, MAX_LEGS, TICKET_SEED, VAULT_SEED},
    error::SideMarketError,
    events::Traded,
    instructions::{now, transfer_from_market, transfer_from_signer},
    math,
    state::{Config, Market, MarketStatus, OutcomeKind, Ticket},
};

#[event_cpi]
#[derive(Accounts)]
#[instruction(outcome_id: u16)]
pub struct Trade<'info> {
    /// The ticket's wallet: signs for its token account.
    pub wallet: Signer<'info>,

    /// Pays a new leg's growth of the ticket (the operator on the server's transactions; the
    /// wallet itself may pay, and a sell never grows the ticket).
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

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

    /// PDA ["ticket", market, wallet].
    #[account(
        mut,
        seeds = [TICKET_SEED, market.key().as_ref(), wallet.key().as_ref()],
        bump = ticket.bump,
        has_one = market @ SideMarketError::TicketMismatch,
        has_one = wallet @ SideMarketError::TicketMismatch,
        realloc = ticket.space_after(outcome_id),
        realloc::payer = payer,
        realloc::zero = false,
    )]
    pub ticket: Account<'info, Ticket>,

    /// The wallet's token account: pays a buy, receives a sell.
    #[account(
        mut,
        token::mint = mint,
        token::authority = wallet,
        token::token_program = token_program,
    )]
    pub wallet_token: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// D4: moves the wallet's leg on `outcome_id` by `delta` (+ YES / - NO shares; buy NO d is delta
/// -d, sell NO d is +d) on an Open market, never on FIELD and never across zero.
/// cash = C(q + delta e_i) - C(q) + dN (math::trade, math::trade_flows; the cost's high bound,
/// rounded up, plus 1): a buy (|position| grows) pays cash >= 1 plus the fee and needs an
/// unrevoked ticket; a sell receives max(0, -cash) less the fee. `limit` is the most a buy pays or
/// the least a sell receives, fee included. D6 is asserted from the vault after the transfers.
///
/// When it may land (the owner's rule: a position can be closed whenever the market is open):
/// - a buy only inside a window (D8);
/// - a sell (delta against the position's sign: |position| shrinks) inside a window, or outside
///   every window while sells are open (Market::sells_open: before sell_until when set, and
///   always before void_after, so a sell and a permissionless void never land together).
///
/// A halted, proposed or resolved market trades nothing (InvalidStatus).
pub fn handle_trade(ctx: Context<Trade>, outcome_id: u16, delta: i64, limit: u64) -> Result<()> {
    let at = now()?;
    let market = &mut ctx.accounts.market;
    require!(
        market.status == MarketStatus::Open,
        SideMarketError::InvalidStatus
    );
    // A sell moves the position toward zero; anything else (a flat position, the same sign, a
    // zero delta) is a buy and keeps the buy's rule. A delta that would cross zero is a sell
    // here and refused as CrossesZero by math::trade_flows, which classifies every trade it
    // accepts the same way (Flows::buy).
    let held = ctx
        .accounts
        .ticket
        .legs
        .iter()
        .find(|l| l.outcome_id == outcome_id)
        .map_or(0, |l| l.shares);
    let sell = (held > 0 && delta < 0) || (held < 0 && delta > 0);
    require!(
        market.window_open(at) || (sell && market.sells_open(at)),
        SideMarketError::TradingClosed
    );
    let i = market.index_of(outcome_id)?;
    require!(
        market.outcomes[i].kind != OutcomeKind::Field,
        SideMarketError::FieldNotTradeable
    );
    require!(
        market.outcomes[i].is_live(),
        SideMarketError::OutcomeNotLive
    );

    let ticket = &mut ctx.accounts.ticket;
    let leg = ticket.leg_index(outcome_id);
    require!(
        ticket.legs.len() <= usize::from(MAX_LEGS),
        SideMarketError::TooManyLegs
    );
    let position = ticket.legs[leg].shares;

    let (b, base, sum, fee_rate) = (market.b, market.base, market.sum, market.fee_rate_ppm);
    let (base, sum, dc) = math::trade(&mut market.outcomes, i, delta, base, sum, b)?;
    let flows = math::trade_flows(position, delta, dc, fee_rate)?;
    let (paid_in, paid_out) = if flows.buy {
        require!(!ticket.revoked, SideMarketError::TicketRevoked);
        let total = flows
            .pay
            .checked_add(flows.fee)
            .ok_or(SideMarketError::MathOverflow)?;
        require!(total <= limit, SideMarketError::LimitExceeded);
        (total, 0)
    } else {
        let net = flows.receive - flows.fee;
        require!(net >= limit, SideMarketError::LimitExceeded);
        (0, net)
    };

    market.base = base;
    market.sum = sum;
    let o = &mut market.outcomes[i];
    o.no_shares = u64::try_from(i128::from(o.no_shares) + i128::from(flows.dn))
        .map_err(|_| error!(SideMarketError::MathOverflow))?;
    let q = o.q;
    market.no_total = u64::try_from(i128::from(market.no_total) + i128::from(flows.dn))
        .map_err(|_| error!(SideMarketError::MathOverflow))?;
    market.fees_paid = market
        .fees_paid
        .checked_add(flows.fee)
        .ok_or(SideMarketError::MathOverflow)?;
    let no_total = market.no_total;

    let l = &mut ticket.legs[leg];
    l.shares = flows.position;
    l.cost_in = l
        .cost_in
        .checked_add(paid_in)
        .ok_or(SideMarketError::MathOverflow)?;
    l.proceeds_out = l
        .proceeds_out
        .checked_add(paid_out)
        .ok_or(SideMarketError::MathOverflow)?;

    let a = &ctx.accounts;
    if flows.buy {
        transfer_from_signer(
            &a.token_program,
            &a.wallet_token,
            a.vault.to_account_info(),
            &a.mint,
            a.wallet.to_account_info(),
            flows.pay,
        )?;
        transfer_from_signer(
            &a.token_program,
            &a.wallet_token,
            a.fees.to_account_info(),
            &a.mint,
            a.wallet.to_account_info(),
            flows.fee,
        )?;
    } else {
        transfer_from_market(
            &a.token_program,
            &a.vault,
            a.wallet_token.to_account_info(),
            &a.mint,
            &a.market,
            paid_out,
        )?;
        transfer_from_market(
            &a.token_program,
            &a.vault,
            a.fees.to_account_info(),
            &a.mint,
            &a.market,
            flows.fee,
        )?;
    }
    ctx.accounts.vault.reload()?;
    let vault = ctx.accounts.vault.amount;
    ctx.accounts.market.assert_solvent(vault)?;

    emit_cpi!(Traded {
        market: ctx.accounts.market.key(),
        wallet: ctx.accounts.wallet.key(),
        outcome_id,
        delta,
        position: flows.position,
        pay: flows.pay,
        receive: paid_out,
        fee: flows.fee,
        q,
        no_total,
        base,
        sum,
        vault,
    });
    Ok(())
}
