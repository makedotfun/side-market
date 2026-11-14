use anchor_lang::{prelude::*, solana_program::bpf_loader_upgradeable};
use anchor_spl::token::Mint;

use crate::{
    constants::{CONFIG_SEED, MINT_DECIMALS},
    error::SideMarketError,
    events::ConfigSet,
    state::{Config, ConfigParams},
};

#[event_cpi]
#[derive(Accounts)]
pub struct InitConfig<'info> {
    /// The program's upgrade authority (its ProgramData's upgrade_authority_address); pays the
    /// rent.
    #[account(mut)]
    pub authority: Signer<'info>,

    /// PDA ["config"]: `init`, so it is created once.
    #[account(
        init,
        payer = authority,
        space = Config::SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Account<'info, Config>,

    /// The one mint every market trades in: owned by the classic Token program (Account<Mint>
    /// refuses a Token-2022 mint by its owner), 6 decimals.
    #[account(constraint = mint.decimals == MINT_DECIMALS @ SideMarketError::InvalidMint)]
    pub mint: Account<'info, Mint>,

    /// This program's ProgramData account, ["<program id>"] under the upgradeable loader.
    #[account(
        seeds = [crate::ID.as_ref()],
        bump,
        seeds::program = bpf_loader_upgradeable::ID,
        constraint = program_data.upgrade_authority_address == Some(authority.key())
            @ SideMarketError::NotUpgradeAuthority,
    )]
    pub program_data: Account<'info, ProgramData>,

    pub system_program: Program<'info, System>,
}

/// Only the program's upgrade authority, once: creates the Config with `params` (Config::apply's
/// bounds) and the mint, fixed from here on.
pub fn handle_init_config(ctx: Context<InitConfig>, params: ConfigParams) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.bump = ctx.bumps.config;
    config.mint = ctx.accounts.mint.key();
    config.reserved = [0; 64];
    config.apply(&params)?;
    emit_cpi!(ConfigSet {
        config: config.key(),
        by: ctx.accounts.authority.key(),
        initialized: true,
        mint: config.mint,
        params,
    });
    Ok(())
}
