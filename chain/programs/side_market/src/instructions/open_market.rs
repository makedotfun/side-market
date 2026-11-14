use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::{
    constants::{
        CONFIG_SEED, FEES_SEED, FIELD_ID, MARKET_SEED, MAX_M_FLOOR_PPM, MIN_M_FLOOR_PPM, NONE_ID,
        VAULT_SEED, VOID_GRACE,
    },
    error::SideMarketError,
    events::MarketOpened,
    instructions::{create_token_account, now, token_amount, transfer_from_signer},
    math::{self, PPM},
    state::{Config, Market, MarketStatus, Outcome, OutcomeKind, OutcomeStatus},
};

/// open_market's settings (D2, D3, D5, D7, D10, D11).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct OpenMarketArgs {
    /// The house's seed, base units: b = floor(seed / ln(1/m_floor)), and the house deposits
    /// ceil(b ln(1/m_floor)) + 1,000 (math::liquidity_for).
    pub seed: u64,
    /// The minimum world share, ppm, in [MIN_M_FLOOR_PPM, MAX_M_FLOOR_PPM].
    pub m_floor_ppm: u32,
    /// NONE's opening mass, ppm, at least m_floor; FIELD takes the rest, at least m_floor too.
    pub none_mass_ppm: u32,
    /// The market's fee rate, ppm, at most the Config's max_fee_rate_ppm.
    pub fee_rate_ppm: u32,
    /// The competition deadline (unix seconds): scratches end here, and void_after is 14 days on.
    pub lock_ts: i64,
    /// The competition's prize escrow (owned by Config.escrow_program), or Pubkey::default():
    /// propose then requires it Claimable or Closed on the same result.
    pub escrow: Pubkey,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(competition_id: [u8; 16])]
pub struct OpenMarket<'info> {
    /// The Config's operator: pays the rent of the market, its vault and its fees account.
    #[account(mut, address = config.operator @ SideMarketError::UnauthorizedOperator)]
    pub operator: Signer<'info>,

    /// The Config's house: deposits the seed.
    #[account(address = config.house @ SideMarketError::UnauthorizedHouse)]
    pub house: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    /// The Config's mint (classic Token program only).
    #[account(address = config.mint @ SideMarketError::InvalidMint)]
    pub mint: Account<'info, Mint>,

    /// The house's token account the deposit comes from.
    #[account(
        mut,
        token::mint = mint,
        token::authority = house,
        token::token_program = token_program,
    )]
    pub house_token: Account<'info, TokenAccount>,

    /// PDA ["market", competition_id]: one market per competition.
    #[account(
        init,
        payer = operator,
        space = Market::space(2),
        seeds = [MARKET_SEED, competition_id.as_ref()],
        bump,
    )]
    pub market: Account<'info, Market>,

    /// CHECK: PDA ["vault", market] (seeds checked): created here as the market's classic SPL
    /// token account of the mint (instructions::create_token_account).
    #[account(mut, seeds = [VAULT_SEED, market.key().as_ref()], bump)]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: PDA ["fees", market] (seeds checked): created here as the market's fee account.
    #[account(mut, seeds = [FEES_SEED, market.key().as_ref()], bump)]
    pub fees: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

/// Opens the market for `competition_id` with NONE (id 0) and FIELD (id 1) and moves the house
/// deposit into its vault. Windows are closed until set_windows opens one.
pub fn handle_open_market(
    ctx: Context<OpenMarket>,
    competition_id: [u8; 16],
    args: OpenMarketArgs,
) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(
        (MIN_M_FLOOR_PPM..=MAX_M_FLOOR_PPM).contains(&args.m_floor_ppm),
        SideMarketError::InvalidMarketParams
    );
    require!(
        args.fee_rate_ppm <= config.max_fee_rate_ppm,
        SideMarketError::FeeRateTooHigh
    );
    let field_mass = u64::from(args.none_mass_ppm);
    require!(field_mass < PPM, SideMarketError::MassBelowFloor);
    let field_mass = (PPM - field_mass) as u32;
    require!(
        args.none_mass_ppm >= args.m_floor_ppm && field_mass >= args.m_floor_ppm,
        SideMarketError::MassBelowFloor
    );
    require!(args.lock_ts > 0, SideMarketError::InvalidMarketParams);
    let void_after = args
        .lock_ts
        .checked_add(VOID_GRACE)
        .ok_or(SideMarketError::InvalidMarketParams)?;
    let (b, deposit) = math::liquidity_for(args.seed, args.m_floor_ppm)
        .map_err(|_| error!(SideMarketError::InvalidMarketParams))?;

    let outcome = |id: u16, kind: OutcomeKind, mass_ppm: u32| -> Result<Outcome> {
        Ok(Outcome {
            id,
            kind,
            status: OutcomeStatus::Live,
            mass_ppm,
            theta: math::theta_of_ppm(mass_ppm)?,
            q: 0,
            no_shares: 0,
            w: 0,
            yes_value: 0,
            no_value: 0,
        })
    };
    let mut outcomes = vec![
        outcome(NONE_ID, OutcomeKind::None, args.none_mass_ppm)?,
        outcome(FIELD_ID, OutcomeKind::Field, field_mass)?,
    ];
    let (base, sum) = math::open_book(&mut outcomes, b)?;

    let opened_at = now()?;
    let market = &mut ctx.accounts.market;
    market.set_inner(Market {
        bump: ctx.bumps.market,
        vault_bump: ctx.bumps.vault,
        fees_bump: ctx.bumps.fees,
        status: MarketStatus::Open,
        halted_by_guardian: false,
        competition_id,
        payer: ctx.accounts.operator.key(),
        mint: ctx.accounts.mint.key(),
        escrow: args.escrow,
        b,
        m_floor_ppm: args.m_floor_ppm,
        fee_rate_ppm: args.fee_rate_ppm,
        deposit,
        lock_ts: args.lock_ts,
        void_after,
        bounty_from: 0,
        bounty_until: 0,
        race_from: 0,
        race_until: 0,
        sell_until: 0,
        base,
        sum,
        no_total: 0,
        scratch_owed: 0,
        fees_paid: 0,
        tickets: 0,
        live_entries: 0,
        last_outcome_id: FIELD_ID,
        winner: 0,
        result_sha256: [0; 32],
        opened_at,
        proposed_at: 0,
        dispute_ends_at: 0,
        resolved_at: 0,
        reserved: [0; 64],
        outcomes,
    });

    let a = &ctx.accounts;
    let market_key = a.market.key();
    for (account, seed, bump) in [
        (&a.vault, VAULT_SEED, ctx.bumps.vault),
        (&a.fees, FEES_SEED, ctx.bumps.fees),
    ] {
        create_token_account(
            a.operator.to_account_info(),
            account.to_account_info(),
            &[seed, market_key.as_ref(), &[bump]],
            &a.mint,
            a.market.to_account_info(),
            &a.token_program,
            &a.system_program,
        )?;
    }
    transfer_from_signer(
        &a.token_program,
        &a.house_token,
        a.vault.to_account_info(),
        &a.mint,
        a.house.to_account_info(),
        deposit,
    )?;
    let vault = token_amount(&a.vault.to_account_info())?;
    require!(vault == deposit, SideMarketError::Insolvent);
    a.market.assert_solvent(vault)?;

    emit_cpi!(MarketOpened {
        market: ctx.accounts.market.key(),
        competition_id,
        escrow: args.escrow,
        b,
        m_floor_ppm: args.m_floor_ppm,
        fee_rate_ppm: args.fee_rate_ppm,
        deposit,
        none_mass_ppm: args.none_mass_ppm,
        field_mass_ppm: field_mass,
        lock_ts: args.lock_ts,
        void_after,
    });
    Ok(())
}
