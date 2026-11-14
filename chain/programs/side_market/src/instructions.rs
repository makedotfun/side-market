pub mod add_outcome;
pub mod close_market;
pub mod finalize;
pub mod halt;
pub mod init_config;
pub mod open_market;
pub mod open_ticket;
pub mod override_result;
pub mod propose;
pub mod resume;
pub mod revoke_ticket;
pub mod scratch_outcome;
pub mod set_config;
pub mod set_sell_until;
pub mod set_windows;
pub mod settle_ticket;
pub mod sweep_fees;
pub mod trade;
pub mod void_market;

pub use add_outcome::*;
pub use close_market::*;
pub use finalize::*;
pub use halt::*;
pub use init_config::*;
pub use open_market::*;
pub use open_ticket::*;
pub use override_result::*;
pub use propose::*;
pub use resume::*;
pub use revoke_ticket::*;
pub use scratch_outcome::*;
pub use set_config::*;
pub use set_sell_until::*;
pub use set_windows::*;
pub use settle_ticket::*;
pub use sweep_fees::*;
pub use trade::*;
pub use void_market::*;

use anchor_lang::{
    prelude::*,
    solana_program::program_pack::Pack,
    system_program::{
        allocate, assign, create_account, transfer, Allocate, Assign, CreateAccount, Transfer,
    },
};
use anchor_spl::token::{
    initialize_account3, spl_token, transfer_checked, InitializeAccount3, Mint, Token,
    TokenAccount, TransferChecked,
};

use crate::{error::SideMarketError, state::Market};

/// Moves `amount` out of a token account the market owns (its vault or fees account), signed by
/// the market PDA. A zero amount moves nothing.
pub(crate) fn transfer_from_market<'info>(
    token_program: &Program<'info, Token>,
    from: &Account<'info, TokenAccount>,
    to: AccountInfo<'info>,
    mint: &Account<'info, Mint>,
    market: &Account<'info, Market>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let seeds = market.signer_seeds();
    let signer: &[&[&[u8]]] = &[&seeds];
    transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to,
                authority: market.to_account_info(),
            },
            signer,
        ),
        amount,
        mint.decimals,
    )
}

/// Moves `amount` from a token account `authority` signs for. A zero amount moves nothing.
pub(crate) fn transfer_from_signer<'info>(
    token_program: &Program<'info, Token>,
    from: &Account<'info, TokenAccount>,
    to: AccountInfo<'info>,
    mint: &Account<'info, Mint>,
    authority: AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    transfer_checked(
        CpiContext::new(
            token_program.key(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to,
                authority,
            },
        ),
        amount,
        mint.decimals,
    )
}

/// The unix time.
pub(crate) fn now() -> Result<i64> {
    Ok(Clock::get()?.unix_timestamp)
}

/// A u128 liability as a u64 for an event (saturating: only an insolvent market could pass it,
/// and assert_solvent refuses that first).
pub(crate) fn as_u64(v: u128) -> u64 {
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// Refuses unless `status` is one of `allowed`.
pub(crate) fn require_status(
    status: crate::state::MarketStatus,
    allowed: &[crate::state::MarketStatus],
) -> Result<()> {
    require!(allowed.contains(&status), SideMarketError::InvalidStatus);
    Ok(())
}

/// Creates the classic SPL token account of `mint` at the PDA `account` (signed by `seeds`,
/// bump included), owned by `authority`, `payer` paying its rent: open_market's vault and fees
/// account. (Anchor 1.2.1's `init` of a PDA token account calls token_interface, which needs
/// anchor-spl's token_2022 feature; this program carries no Token-2022 code.) An address someone
/// sent lamports to first is still created (topped up, allocated and assigned, as Anchor's `init`
/// does), so a deposit there cannot block the market.
pub(crate) fn create_token_account<'info>(
    payer: AccountInfo<'info>,
    account: AccountInfo<'info>,
    seeds: &[&[u8]],
    mint: &Account<'info, Mint>,
    authority: AccountInfo<'info>,
    token_program: &Program<'info, Token>,
    system_program: &Program<'info, System>,
) -> Result<()> {
    let space = spl_token::state::Account::LEN;
    let rent = Rent::get()?.minimum_balance(space);
    let signer: &[&[&[u8]]] = &[seeds];
    let held = account.lamports();
    if held == 0 {
        create_account(
            CpiContext::new_with_signer(
                system_program.key(),
                CreateAccount {
                    from: payer,
                    to: account.clone(),
                },
                signer,
            ),
            rent,
            space as u64,
            &spl_token::ID,
        )?;
    } else {
        require!(
            account.owner == &system_program.key() && account.data_is_empty(),
            SideMarketError::TokenAccountMismatch
        );
        if held < rent {
            transfer(
                CpiContext::new(
                    system_program.key(),
                    Transfer {
                        from: payer,
                        to: account.clone(),
                    },
                ),
                rent - held,
            )?;
        }
        allocate(
            CpiContext::new_with_signer(
                system_program.key(),
                Allocate {
                    account_to_allocate: account.clone(),
                },
                signer,
            ),
            space as u64,
        )?;
        assign(
            CpiContext::new_with_signer(
                system_program.key(),
                Assign {
                    account_to_assign: account.clone(),
                },
                signer,
            ),
            &spl_token::ID,
        )?;
    }
    initialize_account3(CpiContext::new(
        token_program.key(),
        InitializeAccount3 {
            account,
            mint: mint.to_account_info(),
            authority,
        },
    ))
}

/// A token account's balance read from its data (for an account created in this instruction).
pub(crate) fn token_amount(account: &AccountInfo) -> Result<u64> {
    let data = account.try_borrow_data()?;
    let parsed = TokenAccount::try_deserialize(&mut &data[..])?;
    Ok(parsed.amount)
}
