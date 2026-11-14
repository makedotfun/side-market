use anchor_lang::prelude::*;

use crate::{
    constants::CONFIG_SEED,
    error::SideMarketError,
    events::ConfigSet,
    state::{Config, ConfigParams},
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetConfig<'info> {
    #[account(address = config.admin @ SideMarketError::UnauthorizedAdmin)]
    pub admin: Signer<'info>,

    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

/// The admin replaces every Config setting but the mint (Config::apply's bounds). A market keeps
/// the fee rate it opened with; a dispute delay applies to proposals after it.
pub fn handle_set_config(ctx: Context<SetConfig>, params: ConfigParams) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.apply(&params)?;
    emit_cpi!(ConfigSet {
        config: config.key(),
        by: ctx.accounts.admin.key(),
        initialized: false,
        mint: config.mint,
        params,
    });
    Ok(())
}
