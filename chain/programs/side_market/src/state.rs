use anchor_lang::prelude::*;

use crate::{
    constants::{
        CONFIG_SEED, FIELD_ID, MARKET_SEED, MAX_DISPUTE_SECS, MAX_FEE_RATE_PPM, MIN_DISPUTE_SECS,
        NONE_ID,
    },
    error::SideMarketError,
    math::{self, Weighted},
};

// ------------------------------------------------------------------------------ the Config

/// The program's one Config, PDA `["config"]` (D14): its roles, the mint every market trades in
/// (fixed at init), the prize escrow program propose reads, the dispute delay and the fee cap.
#[account]
#[derive(InitSpace, Debug, PartialEq, Eq)]
pub struct Config {
    pub bump: u8,
    /// Sets the Config (set_config) and may resume any halted market.
    pub admin: Pubkey,
    /// May halt a market (only escalate): a key never on the server.
    pub guardian: Pubkey,
    /// Overrides a proposed result or voids a market before it is final.
    pub arbiter: Pubkey,
    /// The server's key: opens markets and tickets, sets windows, lists and scratches entries,
    /// halts and resumes, sweeps fees and closes markets.
    pub operator: Pubkey,
    /// Proposes each market's result.
    pub resolver: Pubkey,
    /// Deposits each market's seed and takes its surplus and fees.
    pub house: Pubkey,
    /// The one mint (classic SPL Token, 6 decimals): USDC on mainnet, the house test mint on
    /// devnet and localnet. Fixed at init_config.
    pub mint: Pubkey,
    /// The prize escrow program whose escrow accounts propose reads.
    pub escrow_program: Pubkey,
    /// D10: seconds between propose and the earliest finalize.
    pub min_dispute_secs: i64,
    /// D5: the highest fee rate a market may open with, in ppm (at most MAX_FEE_RATE_PPM).
    pub max_fee_rate_ppm: u32,
    /// Zeroed room for later fields.
    pub reserved: [u8; 64],
}

/// Every Config setting but the mint: init_config's and set_config's argument.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ConfigParams {
    pub admin: Pubkey,
    pub guardian: Pubkey,
    pub arbiter: Pubkey,
    pub operator: Pubkey,
    pub resolver: Pubkey,
    pub house: Pubkey,
    pub escrow_program: Pubkey,
    pub min_dispute_secs: i64,
    pub max_fee_rate_ppm: u32,
}

impl Config {
    pub const SPACE: usize = 8 + Self::INIT_SPACE;

    pub fn pda() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[CONFIG_SEED], &crate::ID)
    }

    /// Writes `params` after checking them: every key set, the dispute delay in
    /// [MIN_DISPUTE_SECS, MAX_DISPUTE_SECS], the fee cap at most MAX_FEE_RATE_PPM.
    pub fn apply(&mut self, params: &ConfigParams) -> Result<()> {
        let keys = [
            params.admin,
            params.guardian,
            params.arbiter,
            params.operator,
            params.resolver,
            params.house,
            params.escrow_program,
        ];
        require!(
            keys.iter().all(|k| *k != Pubkey::default()),
            SideMarketError::InvalidConfig
        );
        require!(
            (MIN_DISPUTE_SECS..=MAX_DISPUTE_SECS).contains(&params.min_dispute_secs),
            SideMarketError::InvalidConfig
        );
        require!(
            params.max_fee_rate_ppm <= MAX_FEE_RATE_PPM,
            SideMarketError::InvalidConfig
        );
        self.admin = params.admin;
        self.guardian = params.guardian;
        self.arbiter = params.arbiter;
        self.operator = params.operator;
        self.resolver = params.resolver;
        self.house = params.house;
        self.escrow_program = params.escrow_program;
        self.min_dispute_secs = params.min_dispute_secs;
        self.max_fee_rate_ppm = params.max_fee_rate_ppm;
        Ok(())
    }
}

// ------------------------------------------------------------------------------ the market

/// Where a market is in its life.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum MarketStatus {
    /// Listing and trading: buys inside a window, sells inside a window or while sells are open
    /// (Market::sells_open).
    Open,
    /// Frozen by the operator or the guardian: no trade, the marks frozen into the outcomes.
    Halted,
    /// The resolver proposed a winner; the arbiter may override or void until dispute_ends_at.
    Proposed,
    /// The winner stands: settle_ticket pays every ticket.
    Final,
    /// Voided: every live leg is paid its frozen mark; a scratched entry's YES legs are paid 0 and
    /// its NO legs 1 a share.
    Void,
}

/// What an outcome stands for (D1).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum OutcomeKind {
    /// Id 0: no entry wins, or the prize is not awarded.
    None,
    /// Id 1: an entry never listed wins. Untradeable.
    Field,
    /// A listed entry, id 2 on.
    Entry,
}

/// Whether an outcome is in the book.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub enum OutcomeStatus {
    Live,
    /// Scratched before lock_ts (D7), settled as an entry that did not win: its legs are paid
    /// yes_value (0) / no_value (1) at settlement.
    Scratched,
}

/// One outcome of a market's book.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct Outcome {
    pub id: u16,
    pub kind: OutcomeKind,
    pub status: OutcomeStatus,
    /// Its mass in ppm: masses of the live outcomes sum to at most 1,000,000 (exactly, until an
    /// entry with q < 0 is scratched: FIELD then takes only the part of its mass its weight still
    /// held, math::scratch_mass_ppm).
    pub mass_ppm: u32,
    /// ln of its mass as Q64.64, rounded so C never rises (math::theta_of_ppm, math::join,
    /// math::scratch: FIELD's is ln of the weight it holds).
    pub theta: i128,
    /// Net shares: YES minus NO, base units. Frozen at its scratch for a scratched outcome; after
    /// resolution, what the unsettled tickets still hold.
    pub q: i64,
    /// NO shares outstanding on it (N_j).
    pub no_shares: u64,
    /// Cached weight exp(x - base) (math.rs), 0 out of the book.
    pub w: u128,
    /// A scratched outcome's YES value (0), or a live one's frozen YES mark (halt, void): Q0.64,
    /// u64::MAX standing for 1 (math::value_of).
    pub yes_value: u64,
    /// The NO counterpart of yes_value: Q0.64 (u64::MAX, 1, for a scratched outcome).
    pub no_value: u64,
}

impl Outcome {
    pub fn is_live(&self) -> bool {
        self.status == OutcomeStatus::Live
    }

    /// YES shares outstanding: q + N_j.
    pub fn yes_shares(&self) -> Result<u64> {
        let yes = i128::from(self.q) + i128::from(self.no_shares);
        u64::try_from(yes).map_err(|_| error!(SideMarketError::MathOverflow))
    }
}

impl Weighted for Outcome {
    fn theta(&self) -> i128 {
        self.theta
    }
    fn set_theta(&mut self, theta: i128) {
        self.theta = theta;
    }
    fn q(&self) -> i64 {
        self.q
    }
    fn set_q(&mut self, q: i64) {
        self.q = q;
    }
    fn w(&self) -> u128 {
        self.w
    }
    fn set_w(&mut self, w: u128) {
        self.w = w;
    }
    fn in_book(&self) -> bool {
        self.is_live()
    }
    fn leave_book(&mut self) {
        self.status = OutcomeStatus::Scratched;
    }
}

/// One competition's market. PDA `["market", competition_id]`; its vault `["vault", market]`
/// and fees account `["fees", market]` are classic SPL token accounts it owns.
#[account]
#[derive(Debug, PartialEq, Eq)]
pub struct Market {
    pub bump: u8,
    pub vault_bump: u8,
    pub fees_bump: u8,
    pub status: MarketStatus,
    /// The guardian halted it: only the admin resumes.
    pub halted_by_guardian: bool,
    pub competition_id: [u8; 16],
    /// Paid the market's rent (the operator at open): close_market returns it here.
    pub payer: Pubkey,
    pub mint: Pubkey,
    /// The competition's prize escrow (prize_escrow program), or Pubkey::default() with none.
    pub escrow: Pubkey,
    /// Liquidity, base units (math::liquidity_for).
    pub b: u64,
    pub m_floor_ppm: u32,
    pub fee_rate_ppm: u32,
    /// What the house deposited at open: ceil(b L) + 1,000.
    pub deposit: u64,
    /// The competition deadline: scratches end here.
    pub lock_ts: i64,
    /// lock_ts + VOID_GRACE: from here anyone may void a market not yet proposed.
    pub void_after: i64,
    pub bounty_from: i64,
    pub bounty_until: i64,
    pub race_from: i64,
    pub race_until: i64,
    /// Position-reducing trades (sells) land until this moment, the moment the competition's
    /// result becomes public (set_sell_until); 0 = no such moment set (sells land until the market
    /// is proposed). Never past void_after either way (sells_open).
    pub sell_until: i64,
    /// The book's frame (math.rs): the reference exponent and the exact sum of the cached weights.
    pub base: i128,
    pub sum: u128,
    /// N: NO shares outstanding on every live outcome.
    pub no_total: u64,
    /// F: what scratched legs are still owed.
    pub scratch_owed: u64,
    /// Fees paid into the fees account, all time.
    pub fees_paid: u64,
    /// Tickets open (opened and not yet settled).
    pub tickets: u32,
    pub live_entries: u16,
    /// The highest outcome id listed (FIELD_ID until the first entry).
    pub last_outcome_id: u16,
    /// The proposed, then final, winner.
    pub winner: u16,
    pub result_sha256: [u8; 32],
    pub opened_at: i64,
    pub proposed_at: i64,
    pub dispute_ends_at: i64,
    /// When it became Final or Void.
    pub resolved_at: i64,
    /// Zeroed room for later fields.
    pub reserved: [u8; 64],
    pub outcomes: Vec<Outcome>,
}

/// The fixed part of a Market's Borsh size, its Vec's length prefix included.
pub const MARKET_FIXED: usize = 3
    + 1
    + 1
    + 16
    + 3 * 32
    + 8
    + 4
    + 4
    + 8
    + 2 * 8
    + 5 * 8
    + 16
    + 16
    + 3 * 8
    + 4
    + 3 * 2
    + 32
    + 4 * 8
    + 64
    + 4;

impl Market {
    /// The account size holding `outcomes` outcomes.
    pub fn space(outcomes: usize) -> usize {
        8 + MARKET_FIXED + outcomes * Outcome::INIT_SPACE
    }

    pub fn pda(competition_id: &[u8; 16]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[MARKET_SEED, competition_id], &crate::ID)
    }

    /// The seeds the market signs its vault and fees transfers with.
    pub fn signer_seeds(&self) -> [&[u8]; 3] {
        [
            MARKET_SEED,
            &self.competition_id,
            core::slice::from_ref(&self.bump),
        ]
    }

    pub fn index_of(&self, id: u16) -> Result<usize> {
        self.outcomes
            .iter()
            .position(|o| o.id == id)
            .ok_or_else(|| error!(SideMarketError::UnknownOutcome))
    }

    pub fn field_index(&self) -> Result<usize> {
        self.index_of(FIELD_ID)
    }

    /// Live outcomes in the book (entries, NONE and FIELD).
    pub fn book_len(&self) -> u32 {
        math::book_len(&self.outcomes)
    }

    /// Whether `now` is inside a trading window (D8): [bounty_from, bounty_until) or
    /// [race_from, race_until). Buys land only inside one; sells inside one too.
    pub fn window_open(&self, now: i64) -> bool {
        (self.bounty_from <= now && now < self.bounty_until)
            || (self.race_from <= now && now < self.race_until)
    }

    /// Whether a sell (a position-reducing trade) may land at `now` outside every window: before
    /// void_after (from there anyone may void at frozen marks, so no trade may move a mark once a
    /// permissionless void is possible) and before sell_until when it is set (the result is
    /// public from there).
    pub fn sells_open(&self, now: i64) -> bool {
        now < self.void_after && (self.sell_until == 0 || now < self.sell_until)
    }

    /// Whether every window has ended at `now`, and lock_ts has passed.
    pub fn trading_ended(&self, now: i64) -> bool {
        now >= self.lock_ts && now >= self.bounty_until && now >= self.race_until
    }

    /// Freezes every live outcome's marks (D11) from the book as it stands.
    pub fn freeze_marks(&mut self) -> Result<()> {
        let n = self.book_len();
        let sum = self.sum;
        for o in self.outcomes.iter_mut().filter(|o| o.is_live()) {
            let (yes, no) = math::marks(o.w, sum, n)?;
            o.yes_value = yes;
            o.no_value = no;
        }
        Ok(())
    }

    /// D6: the most the market can owe in any world still possible, from its status:
    /// - Open, Halted, Proposed: F + N + max(0, max q_j over the live outcomes);
    /// - Final: F + N + q_winner (FIELD's q is 0: F + N);
    /// - Void: F + sum over live outcomes of value_of(Y_j, yes_j) + value_of(N_j, no_j).
    ///
    /// Settlement takes each ticket's legs off q, N and F as it pays them, so this holds to the
    /// end.
    pub fn liability(&self) -> Result<u128> {
        let f = u128::from(self.scratch_owed);
        match self.status {
            MarketStatus::Open | MarketStatus::Halted | MarketStatus::Proposed => {
                let top = self
                    .outcomes
                    .iter()
                    .filter(|o| o.is_live())
                    .map(|o| o.q)
                    .max()
                    .unwrap_or(0)
                    .max(0);
                Ok(f + u128::from(self.no_total) + top as u128)
            }
            MarketStatus::Final => {
                let i = self.index_of(self.winner)?;
                let q = i128::from(self.outcomes[i].q) + i128::from(self.no_total);
                let q = u128::try_from(q).map_err(|_| error!(SideMarketError::MathOverflow))?;
                Ok(f + q)
            }
            MarketStatus::Void => {
                let mut total = f;
                for o in self.outcomes.iter().filter(|o| o.is_live()) {
                    total += u128::from(math::value_of(o.yes_shares()?, o.yes_value));
                    total += u128::from(math::value_of(o.no_shares, o.no_value));
                }
                Ok(total)
            }
        }
    }

    /// D6, asserted from the vault's real balance at the end of every value-moving instruction.
    pub fn assert_solvent(&self, vault_amount: u64) -> Result<()> {
        require!(
            u128::from(vault_amount) >= self.liability()?,
            SideMarketError::Insolvent
        );
        Ok(())
    }

    /// What a leg holding `shares` of outcome `id` is paid once the market is Final or Void.
    pub fn leg_payout(&self, id: u16, shares: i64) -> Result<u64> {
        if shares == 0 {
            return Ok(0);
        }
        let o = &self.outcomes[self.index_of(id)?];
        let n = shares.unsigned_abs();
        let yes = shares > 0;
        if !o.is_live() || self.status == MarketStatus::Void {
            return Ok(math::value_of(
                n,
                if yes { o.yes_value } else { o.no_value },
            ));
        }
        require!(
            self.status == MarketStatus::Final,
            SideMarketError::InvalidStatus
        );
        let won = o.id == self.winner;
        Ok(if yes == won { n } else { 0 })
    }

    /// Takes a settled leg off the market's totals: q and N_j (and N) for a live outcome, F by
    /// what it was paid for a scratched one.
    pub fn release_leg(&mut self, id: u16, shares: i64, paid: u64) -> Result<()> {
        let i = self.index_of(id)?;
        let o = &mut self.outcomes[i];
        if o.is_live() {
            o.q =
                o.q.checked_sub(shares)
                    .ok_or(SideMarketError::MathOverflow)?;
            if shares < 0 {
                let no = shares.unsigned_abs();
                o.no_shares = o
                    .no_shares
                    .checked_sub(no)
                    .ok_or(SideMarketError::MathOverflow)?;
                self.no_total = self
                    .no_total
                    .checked_sub(no)
                    .ok_or(SideMarketError::MathOverflow)?;
            }
        } else {
            self.scratch_owed = self
                .scratch_owed
                .checked_sub(paid)
                .ok_or(SideMarketError::MathOverflow)?;
        }
        Ok(())
    }
}

/// Whether `id` names NONE or FIELD.
pub fn is_house_outcome(id: u16) -> bool {
    id == NONE_ID || id == FIELD_ID
}

// ------------------------------------------------------------------------------ tickets

/// One outcome a ticket traded.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, InitSpace)]
pub struct Leg {
    pub outcome_id: u16,
    /// Signed position: + YES, - NO.
    pub shares: i64,
    /// What the wallet paid into the leg, fees included, all time.
    pub cost_in: u64,
    /// What the leg paid the wallet, fees taken, all time.
    pub proceeds_out: u64,
}

/// A wallet's position in one market (D9). PDA `["ticket", market, wallet]`, opened by the
/// operator after the server's eligibility check; its legs grow by realloc.
#[account]
#[derive(Debug, PartialEq, Eq)]
pub struct Ticket {
    pub bump: u8,
    /// Buys refused; sells never are.
    pub revoked: bool,
    pub market: Pubkey,
    pub wallet: Pubkey,
    /// Paid the ticket's rent: settle_ticket returns it here.
    pub payer: Pubkey,
    pub opened_at: i64,
    pub legs: Vec<Leg>,
}

/// The fixed part of a Ticket's Borsh size, its Vec's length prefix included.
pub const TICKET_FIXED: usize = 1 + 1 + 3 * 32 + 8 + 4;

impl Ticket {
    pub fn space(legs: usize) -> usize {
        8 + TICKET_FIXED + legs * Leg::INIT_SPACE
    }

    /// The size after a trade on `outcome_id`: one leg more when it has none on it.
    pub fn space_after(&self, outcome_id: u16) -> usize {
        let more = usize::from(!self.legs.iter().any(|l| l.outcome_id == outcome_id));
        Self::space(self.legs.len() + more)
    }

    pub fn leg_index(&mut self, outcome_id: u16) -> usize {
        match self.legs.iter().position(|l| l.outcome_id == outcome_id) {
            Some(i) => i,
            None => {
                self.legs.push(Leg {
                    outcome_id,
                    shares: 0,
                    cost_in: 0,
                    proceeds_out: 0,
                });
                self.legs.len() - 1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market(outcomes: usize) -> Market {
        let o = Outcome {
            id: 0,
            kind: OutcomeKind::None,
            status: OutcomeStatus::Live,
            mass_ppm: 0,
            theta: 0,
            q: 0,
            no_shares: 0,
            w: 0,
            yes_value: 0,
            no_value: 0,
        };
        Market {
            bump: 0,
            vault_bump: 0,
            fees_bump: 0,
            status: MarketStatus::Open,
            halted_by_guardian: false,
            competition_id: [0; 16],
            payer: Pubkey::default(),
            mint: Pubkey::default(),
            escrow: Pubkey::default(),
            b: 0,
            m_floor_ppm: 0,
            fee_rate_ppm: 0,
            deposit: 0,
            lock_ts: 0,
            void_after: 0,
            bounty_from: 0,
            bounty_until: 0,
            race_from: 0,
            race_until: 0,
            sell_until: 0,
            base: 0,
            sum: 0,
            no_total: 0,
            scratch_owed: 0,
            fees_paid: 0,
            tickets: 0,
            live_entries: 0,
            last_outcome_id: 0,
            winner: 0,
            result_sha256: [0; 32],
            opened_at: 0,
            proposed_at: 0,
            dispute_ends_at: 0,
            resolved_at: 0,
            reserved: [0; 64],
            outcomes: vec![o; outcomes],
        }
    }

    #[test]
    fn market_space_is_its_borsh_size() {
        for n in [0, 2, 66] {
            let m = market(n);
            let mut buf = Vec::new();
            m.serialize(&mut buf).unwrap();
            assert_eq!(8 + buf.len(), Market::space(n));
        }
        assert_eq!(Outcome::INIT_SPACE, 72);
    }

    #[test]
    fn sells_open_until_void_after_and_sell_until() {
        let mut m = market(0);
        m.lock_ts = 100;
        m.void_after = 1_000;
        // No sell_until: open until void_after.
        assert!(m.sells_open(0));
        assert!(m.sells_open(999));
        assert!(!m.sells_open(1_000));
        assert!(!m.sells_open(5_000));
        // A sell_until before void_after ends them there.
        m.sell_until = 300;
        assert!(m.sells_open(299));
        assert!(!m.sells_open(300));
        assert!(!m.sells_open(999));
        // A sell_until past void_after does not reach past void_after.
        m.sell_until = 2_000;
        assert!(m.sells_open(999));
        assert!(!m.sells_open(1_000));
    }

    #[test]
    fn ticket_space_is_its_borsh_size() {
        let t = Ticket {
            bump: 0,
            revoked: false,
            market: Pubkey::default(),
            wallet: Pubkey::default(),
            payer: Pubkey::default(),
            opened_at: 0,
            legs: vec![
                Leg {
                    outcome_id: 2,
                    shares: 1,
                    cost_in: 0,
                    proceeds_out: 0
                };
                3
            ],
        };
        let mut buf = Vec::new();
        t.serialize(&mut buf).unwrap();
        assert_eq!(8 + buf.len(), Ticket::space(3));
    }
}
