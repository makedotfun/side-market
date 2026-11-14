use anchor_lang::prelude::*;

use crate::math::MathError;

/// Every error the program returns. Codes start at 6000 in declaration order and are part of the
/// wire interface (the generated client and the server read them): append, never reorder.
#[error_code]
pub enum SideMarketError {
    #[msg("The signer is not the program's upgrade authority")]
    NotUpgradeAuthority,
    #[msg("The signer is not the Config's admin")]
    UnauthorizedAdmin,
    #[msg("The signer is not the Config's operator")]
    UnauthorizedOperator,
    #[msg("The signer is not the Config's resolver")]
    UnauthorizedResolver,
    #[msg("The signer is not the Config's arbiter")]
    UnauthorizedArbiter,
    #[msg("The signer is neither the operator nor the guardian")]
    UnauthorizedHalt,
    #[msg("The signer may not resume this market")]
    UnauthorizedResume,
    #[msg("The signer is not the Config's house")]
    UnauthorizedHouse,
    #[msg("A Config key is unset or a Config value is out of bounds")]
    InvalidConfig,
    #[msg("The mint is not a 6-decimal classic SPL Token mint")]
    InvalidMint,
    #[msg("The market's parameters are out of bounds")]
    InvalidMarketParams,
    #[msg("The market's status does not allow this instruction")]
    InvalidStatus,
    #[msg("A trading window is malformed")]
    InvalidWindow,
    #[msg("Trading is closed: no window is open")]
    TradingClosed,
    #[msg("No outcome has this id")]
    UnknownOutcome,
    #[msg("The outcome is not live")]
    OutcomeNotLive,
    #[msg("FIELD is never tradeable")]
    FieldNotTradeable,
    #[msg("Outcome ids rise from 2 and are never reused")]
    InvalidOutcomeId,
    #[msg("The market holds the most live entries it may")]
    TooManyEntries,
    #[msg("The market holds the most outcomes it may")]
    TooManyOutcomes,
    #[msg("A mass is below the market's m_floor, or would leave FIELD below it")]
    MassBelowFloor,
    #[msg("Only an entry can be scratched")]
    NotAnEntry,
    #[msg("Scratches end at lock_ts")]
    ScratchAfterLock,
    #[msg("The ticket is revoked: buys are refused, sells are not")]
    TicketRevoked,
    #[msg("The ticket belongs to another market or wallet")]
    TicketMismatch,
    #[msg("A trade of zero shares")]
    ZeroDelta,
    #[msg("A trade may not take a position across zero: a flip is two trades")]
    CrossesZero,
    #[msg("A buy must cost at least one base unit")]
    BuyTooSmall,
    #[msg("The trade pays more, or receives less, than its limit")]
    LimitExceeded,
    #[msg("The ticket holds the most legs it may")]
    TooManyLegs,
    #[msg("A fixed-point value left its range")]
    MathOverflow,
    #[msg("A fixed-point argument is outside its domain")]
    MathDomain,
    #[msg("The vault would hold less than the market's worst-case liability (D6)")]
    Insolvent,
    #[msg("Trading has not ended")]
    TradingNotEnded,
    #[msg("The winner must be NONE, FIELD or a live entry")]
    InvalidWinner,
    #[msg("The result hash must not be zero")]
    InvalidResult,
    #[msg("The market names a prize escrow: pass it")]
    EscrowRequired,
    #[msg("The escrow account is not the one the market names, or not the escrow program's")]
    EscrowMismatch,
    #[msg("The escrow is not Claimable or Closed, or holds another competition or result")]
    EscrowNotSettled,
    #[msg("The dispute delay has not passed")]
    DisputeOpen,
    #[msg("The dispute delay has passed")]
    DisputeOver,
    #[msg("void_after has not passed: only the arbiter may void now")]
    VoidNotYet,
    #[msg("Tickets are still open")]
    TicketsOpen,
    #[msg("The token account is not this market's mint or owner")]
    TokenAccountMismatch,
    #[msg("The fee rate is above the Config's max_fee_rate_ppm")]
    FeeRateTooHigh,
    #[msg("The account is not the market's or the ticket's rent payer")]
    PayerMismatch,
    #[msg("A trading window is open: only the arbiter may void now")]
    VoidWhileTrading,
}

impl From<MathError> for SideMarketError {
    fn from(e: MathError) -> Self {
        match e {
            MathError::Overflow => SideMarketError::MathOverflow,
            MathError::Domain => SideMarketError::MathDomain,
            MathError::ZeroDelta => SideMarketError::ZeroDelta,
            MathError::CrossesZero => SideMarketError::CrossesZero,
            MathError::BuyTooSmall => SideMarketError::BuyTooSmall,
        }
    }
}

impl From<MathError> for anchor_lang::error::Error {
    fn from(e: MathError) -> Self {
        SideMarketError::from(e).into()
    }
}
