//! What every LiteSVM test file shares: a world with the release build loaded under mainnet's
//! feature set, its Config created the way a cluster creates it (init_config, signed by the
//! program's upgrade authority), the 6-dp mint, the house's token account, and a builder per
//! instruction and a reader per account.

#![allow(dead_code, reason = "each test file uses its own subset")]

use {
    crate::{mainnet, program_so},
    anchor_lang::{
        event::EVENT_IX_TAG_LE,
        prelude::{Clock, Pubkey},
        solana_program::{
            bpf_loader_upgradeable,
            instruction::{AccountMeta, Instruction},
            program_option::COption,
            program_pack::Pack,
            system_program,
        },
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::{
        associated_token::{self, get_associated_token_address},
        token::{self, spl_token},
    },
    litesvm::LiteSVM,
    side_market::{
        accounts as accts, instruction as ixs, math, ConfigParams, Market, OpenMarketArgs, Ticket,
        CONFIG_SEED, EVENT_AUTHORITY_AND_BUMP, FEES_SEED, MARKET_SEED, TICKET_SEED, VAULT_SEED,
    },
    solana_account::Account,
    solana_keypair::Keypair,
    solana_signer::Signer,
};

pub const NOW: i64 = 1_800_000_000;
pub const DAY: i64 = 24 * 60 * 60;
/// The competition deadline every test market opens with.
pub const LOCK: i64 = NOW + 10 * DAY;
pub const SEED: u64 = 50_000_000;
pub const M_FLOOR: u32 = 5_000;
pub const NONE_MASS: u32 = 50_000;
pub const FEE_RATE: u32 = 20_000;
pub const MAX_FEE_RATE: u32 = 70_000;
pub const DISPUTE: i64 = 120;
pub const SOL: u64 = 1_000_000_000;
/// The prize escrow program id (chain/programs/prize_escrow declare_id!).
pub const PRIZE_ESCROW_ID: &str = "sHC7T46CaBxEdRZjZCVX5D1o9cyomsr4oUA1bHH2MDd";
/// The SPL Token-2022 program: a mint it owns must be refused.
pub const TOKEN_2022_ID: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
/// The ComputeBudget program.
pub const COMPUTE_BUDGET_ID: &str = "ComputeBudget111111111111111111111111111111";

pub fn prize_escrow_id() -> Pubkey {
    PRIZE_ESCROW_ID.parse().unwrap()
}

/// A transaction's result.
pub struct Sent {
    pub ok: bool,
    /// The custom error code, when it failed with one.
    pub code: Option<u32>,
    pub error: String,
    pub logs: String,
    pub cu: u64,
    /// The bodies of the events the program emitted (emit_cpi! inner instructions).
    pub events: Vec<Vec<u8>>,
}

/// The transaction succeeded.
#[track_caller]
pub fn expect_ok(sent: Sent, what: &str) -> Sent {
    assert!(sent.ok, "{what} failed: {}\n{}", sent.error, sent.logs);
    sent
}

/// The transaction failed with exactly `code`.
#[track_caller]
pub fn expect_code(sent: &Sent, code: impl Into<u32>, what: &str) {
    let code = code.into();
    assert_eq!(
        sent.code,
        Some(code),
        "{what}: expected error {code}, got {} {}\n{}",
        if sent.ok { "success" } else { "failure" },
        sent.error,
        sent.logs
    );
}

/// The transaction failed (any error).
#[track_caller]
pub fn expect_fail(sent: &Sent, what: &str) {
    assert!(
        !sent.ok,
        "{what}: expected a failure, got success\n{}",
        sent.logs
    );
}

/// An Anchor framework error's code.
pub fn anchor_code(code: anchor_lang::error::ErrorCode) -> u32 {
    code as u32
}

pub struct Keys {
    pub upgrade: Keypair,
    pub admin: Keypair,
    pub guardian: Keypair,
    pub arbiter: Keypair,
    pub operator: Keypair,
    pub resolver: Keypair,
    pub house: Keypair,
    pub stranger: Keypair,
}

/// One market's addresses.
#[derive(Clone, Copy, Debug)]
pub struct Mkt {
    pub cid: [u8; 16],
    pub market: Pubkey,
    pub vault: Pubkey,
    pub fees: Pubkey,
}

impl Mkt {
    pub fn of(cid: [u8; 16]) -> Self {
        let market = market_pda(&cid);
        Mkt {
            cid,
            market,
            vault: vault_pda(&market),
            fees: fees_pda(&market),
        }
    }
}

/// A trader: its key and its funded token account.
pub struct Trader {
    pub key: Keypair,
    pub token: Pubkey,
}

pub struct World {
    pub svm: LiteSVM,
    pub k: Keys,
    pub mint: Pubkey,
    pub house_token: Pubkey,
}

pub fn config_pda() -> Pubkey {
    Pubkey::find_program_address(&[CONFIG_SEED], &side_market::ID).0
}

pub fn market_pda(cid: &[u8; 16]) -> Pubkey {
    Pubkey::find_program_address(&[MARKET_SEED, cid], &side_market::ID).0
}

pub fn vault_pda(market: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[VAULT_SEED, market.as_ref()], &side_market::ID).0
}

pub fn fees_pda(market: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[FEES_SEED, market.as_ref()], &side_market::ID).0
}

pub fn ticket_pda(market: &Pubkey, wallet: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[TICKET_SEED, market.as_ref(), wallet.as_ref()],
        &side_market::ID,
    )
    .0
}

pub fn program_data_pda(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program.as_ref()], &bpf_loader_upgradeable::ID).0
}

pub fn event_authority() -> Pubkey {
    EVENT_AUTHORITY_AND_BUMP.0
}

pub fn ix(accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction::new_with_bytes(
        side_market::ID,
        &data.data(),
        accounts.to_account_metas(None),
    )
}

/// A ComputeBudget SetComputeUnitLimit instruction.
pub fn cu_limit(units: u32) -> Instruction {
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET_ID.parse().unwrap(),
        accounts: vec![],
        data,
    }
}

pub fn cid(n: u8) -> [u8; 16] {
    let mut c = [0u8; 16];
    c[0] = 0x5e;
    c[15] = n;
    c
}

/// The bytes of a classic SPL mint of `decimals`.
pub fn mint_data(decimals: u8, supply: u64) -> Vec<u8> {
    let mut data = vec![0u8; spl_token::state::Mint::LEN];
    spl_token::state::Mint {
        mint_authority: COption::Some(Pubkey::new_unique()),
        supply,
        decimals,
        is_initialized: true,
        freeze_authority: COption::None,
    }
    .pack_into_slice(&mut data);
    data
}

/// floor(shares value / 2^64), and `shares` at u64::MAX (which stands for 2^64).
pub fn paid_at(shares: u128, value: u64) -> u128 {
    if value == u64::MAX {
        shares
    } else {
        (shares * u128::from(value)) >> 64
    }
}

/// D6's liability recomputed here from a decoded Market's raw fields, independently of the
/// program's Market::liability():
/// - Open, Halted, Proposed: F + N + max(0, max q over the live entries and NONE);
/// - Final: F + N + the winner's q;
/// - Void: F + over the live outcomes floor(Y yes_value / 2^64) + floor(N_j no_value / 2^64),
///   with Y = q + N_j, a value of u64::MAX paying every share 1 (it stands for 2^64).
///
/// F is scratch_owed, N is no_total; N is also checked to be the sum of the live outcomes' NO
/// shares, and FIELD (never traded) to hold no shares.
#[track_caller]
pub fn liability_of(market: &Market) -> u128 {
    use side_market::{MarketStatus, OutcomeKind};
    let f = u128::from(market.scratch_owed);
    let n = u128::from(market.no_total);
    let live = || market.outcomes.iter().filter(|o| o.is_live());
    let no_sum: u128 = live().map(|o| u128::from(o.no_shares)).sum();
    assert_eq!(
        no_sum, n,
        "N: no_total {} != the live outcomes' NO shares {no_sum}",
        market.no_total
    );
    for o in market
        .outcomes
        .iter()
        .filter(|o| o.kind == OutcomeKind::Field)
    {
        assert_eq!((o.q, o.no_shares), (0, 0), "FIELD holds shares");
    }
    match market.status {
        MarketStatus::Open | MarketStatus::Halted | MarketStatus::Proposed => {
            let top = live()
                .filter(|o| matches!(o.kind, OutcomeKind::Entry | OutcomeKind::None))
                .map(|o| i128::from(o.q))
                .max()
                .unwrap_or(0)
                .max(0);
            f + n + u128::try_from(top).unwrap()
        }
        MarketStatus::Final => {
            let winner = market
                .outcomes
                .iter()
                .find(|o| o.id == market.winner)
                .expect("the winner's outcome");
            let owed = i128::from(winner.q) + i128::from(market.no_total);
            f + u128::try_from(owed).expect("the winner's q + N is not negative")
        }
        MarketStatus::Void => {
            let mut total = f;
            for o in live() {
                let yes = i128::from(o.q) + i128::from(o.no_shares);
                let yes = u128::try_from(yes).expect("YES shares are not negative");
                total += paid_at(yes, o.yes_value);
                total += paid_at(u128::from(o.no_shares), o.no_value);
            }
            total
        }
    }
}

impl World {
    /// The program loaded, the clock at NOW, every key funded, the mint and the house's token
    /// account in place; no Config yet.
    pub fn bare() -> Self {
        let mut svm = mainnet::svm();
        svm.add_program(side_market::ID, program_so::bytes())
            .expect("add side_market");
        let k = Keys {
            upgrade: Keypair::new(),
            admin: Keypair::new(),
            guardian: Keypair::new(),
            arbiter: Keypair::new(),
            operator: Keypair::new(),
            resolver: Keypair::new(),
            house: Keypair::new(),
            stranger: Keypair::new(),
        };
        let mut w = World {
            svm,
            k,
            mint: Pubkey::new_unique(),
            house_token: Pubkey::new_unique(),
        };
        w.set_time(NOW);
        for key in [
            w.k.upgrade.pubkey(),
            w.k.admin.pubkey(),
            w.k.guardian.pubkey(),
            w.k.arbiter.pubkey(),
            w.k.operator.pubkey(),
            w.k.resolver.pubkey(),
            w.k.house.pubkey(),
            w.k.stranger.pubkey(),
        ] {
            w.svm.airdrop(&key, 100 * SOL).unwrap();
        }
        w.set_upgrade_authority(&side_market::ID, Some(w.k.upgrade.pubkey()));
        let mint = w.mint;
        w.put_mint(mint, 6, token::ID);
        let (house_token, house) = (w.house_token, w.k.house.pubkey());
        w.put_token_account(house_token, mint, house, 1_000_000_000_000);
        w
    }

    /// `bare` with the Config created.
    pub fn new() -> Self {
        let mut w = Self::bare();
        let params = w.params();
        let ix = w.init_config_ix(&w.k.upgrade.pubkey(), w.mint, params);
        let upgrade = w.k.upgrade.insecure_clone();
        expect_ok(w.send(&[ix], &upgrade, &[]), "init_config");
        w
    }

    /// A copy of the whole world (LiteSVM is Clone): a fork to play one ending in.
    pub fn fork(&self) -> Self {
        World {
            svm: self.svm.clone(),
            k: Keys {
                upgrade: self.k.upgrade.insecure_clone(),
                admin: self.k.admin.insecure_clone(),
                guardian: self.k.guardian.insecure_clone(),
                arbiter: self.k.arbiter.insecure_clone(),
                operator: self.k.operator.insecure_clone(),
                resolver: self.k.resolver.insecure_clone(),
                house: self.k.house.insecure_clone(),
                stranger: self.k.stranger.insecure_clone(),
            },
            mint: self.mint,
            house_token: self.house_token,
        }
    }

    pub fn params(&self) -> ConfigParams {
        ConfigParams {
            admin: self.k.admin.pubkey(),
            guardian: self.k.guardian.pubkey(),
            arbiter: self.k.arbiter.pubkey(),
            operator: self.k.operator.pubkey(),
            resolver: self.k.resolver.pubkey(),
            house: self.k.house.pubkey(),
            escrow_program: prize_escrow_id(),
            min_dispute_secs: DISPUTE,
            max_fee_rate_ppm: MAX_FEE_RATE,
        }
    }

    // ------------------------------------------------------------------ the chain

    pub fn send(&mut self, ixs: &[Instruction], payer: &Keypair, signers: &[&Keypair]) -> Sent {
        use {
            solana_message::{Message, VersionedMessage},
            solana_transaction::versioned::VersionedTransaction,
            solana_transaction_error::TransactionError,
        };
        self.svm.expire_blockhash();
        let msg =
            Message::new_with_blockhash(ixs, Some(&payer.pubkey()), &self.svm.latest_blockhash());
        let mut all = vec![payer];
        for s in signers {
            if all.iter().all(|kp| kp.pubkey() != s.pubkey()) {
                all.push(s);
            }
        }
        let tx = match VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &all) {
            Ok(tx) => tx,
            Err(e) => {
                return Sent {
                    ok: false,
                    code: None,
                    error: e.to_string(),
                    logs: String::new(),
                    cu: 0,
                    events: vec![],
                }
            }
        };
        match self.svm.send_transaction(tx) {
            Ok(meta) => Sent {
                ok: true,
                code: None,
                error: String::new(),
                logs: meta.pretty_logs(),
                cu: meta.compute_units_consumed,
                events: meta
                    .inner_instructions
                    .iter()
                    .flatten()
                    .filter_map(|i| {
                        i.instruction
                            .data
                            .strip_prefix(EVENT_IX_TAG_LE)
                            .map(<[u8]>::to_vec)
                    })
                    .collect(),
            },
            Err(failed) => Sent {
                ok: false,
                code: match &failed.err {
                    TransactionError::InstructionError(
                        _,
                        anchor_lang::solana_program::instruction::error::InstructionError::Custom(
                            code,
                        ),
                    ) => Some(*code),
                    _ => None,
                },
                error: format!("{:?}", failed.err),
                logs: failed.meta.pretty_logs(),
                cu: failed.meta.compute_units_consumed,
                events: vec![],
            },
        }
    }

    pub fn time(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn set_time(&mut self, t: i64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.unix_timestamp = t;
        clock.slot += 1;
        self.svm.set_sysvar::<Clock>(&clock);
    }

    pub fn set_upgrade_authority(&mut self, program: &Pubkey, authority: Option<Pubkey>) {
        let address = program_data_pda(program);
        let mut account = self.svm.get_account(&address).expect("ProgramData");
        assert_eq!(account.data[..4], [3, 0, 0, 0]);
        match authority {
            Some(key) => {
                account.data[12] = 1;
                account.data[13..45].copy_from_slice(key.as_ref());
            }
            None => {
                account.data[12] = 0;
                account.data[13..45].fill(0);
            }
        }
        self.svm.set_account(address, account).unwrap();
    }

    pub fn put_mint(&mut self, address: Pubkey, decimals: u8, owner: Pubkey) {
        let data = mint_data(decimals, 1 << 60);
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                address,
                Account {
                    lamports,
                    data,
                    owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    pub fn put_token_account_owned(
        &mut self,
        address: Pubkey,
        mint: Pubkey,
        owner: Pubkey,
        amount: u64,
        program: Pubkey,
    ) {
        let mut data = vec![0u8; spl_token::state::Account::LEN];
        spl_token::state::Account {
            mint,
            owner,
            amount,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        }
        .pack_into_slice(&mut data);
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                address,
                Account {
                    lamports,
                    data,
                    owner: program,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    pub fn put_token_account(&mut self, address: Pubkey, mint: Pubkey, owner: Pubkey, amount: u64) {
        self.put_token_account_owned(address, mint, owner, amount, token::ID);
    }

    /// A token account's balance; 0 when it does not exist (or was closed).
    pub fn tokens(&self, address: &Pubkey) -> u64 {
        match self.svm.get_account(address) {
            Some(a) if a.data.len() == spl_token::state::Account::LEN && a.lamports > 0 => {
                spl_token::state::Account::unpack(&a.data).map_or(0, |t| t.amount)
            }
            _ => 0,
        }
    }

    pub fn exists(&self, address: &Pubkey) -> bool {
        self.svm
            .get_account(address)
            .is_some_and(|a| a.lamports > 0)
    }

    pub fn lamports(&self, address: &Pubkey) -> u64 {
        self.svm.get_account(address).map_or(0, |a| a.lamports)
    }

    pub fn anchor<T: AccountDeserialize>(&self, address: &Pubkey) -> Option<T> {
        let account = self.svm.get_account(address)?;
        if account.lamports == 0 || account.owner != side_market::ID {
            return None;
        }
        T::try_deserialize(&mut account.data.as_slice()).ok()
    }

    pub fn market(&self, m: &Mkt) -> Market {
        self.anchor(&m.market).expect("the market")
    }

    pub fn ticket(&self, m: &Mkt, wallet: &Pubkey) -> Ticket {
        self.anchor(&ticket_pda(&m.market, wallet))
            .expect("the ticket")
    }

    /// A funded trader with a token account holding `amount`.
    pub fn trader(&mut self, amount: u64) -> Trader {
        let key = Keypair::new();
        self.svm.airdrop(&key.pubkey(), 10 * SOL).unwrap();
        let token = Pubkey::new_unique();
        let mint = self.mint;
        self.put_token_account(token, mint, key.pubkey(), amount);
        Trader { key, token }
    }

    // ------------------------------------------------------------------ builders

    pub fn init_config_ix(
        &self,
        authority: &Pubkey,
        mint: Pubkey,
        params: ConfigParams,
    ) -> Instruction {
        ix(
            accts::InitConfig {
                authority: *authority,
                config: config_pda(),
                mint,
                program_data: program_data_pda(&side_market::ID),
                system_program: system_program::ID,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::InitConfig { params },
        )
    }

    pub fn set_config_ix(&self, admin: &Pubkey, params: ConfigParams) -> Instruction {
        ix(
            accts::SetConfig {
                admin: *admin,
                config: config_pda(),
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::SetConfig { params },
        )
    }

    pub fn args(&self) -> OpenMarketArgs {
        OpenMarketArgs {
            seed: SEED,
            m_floor_ppm: M_FLOOR,
            none_mass_ppm: NONE_MASS,
            fee_rate_ppm: FEE_RATE,
            lock_ts: LOCK,
            escrow: Pubkey::default(),
        }
    }

    pub fn open_market_ix_with(
        &self,
        m: &Mkt,
        args: OpenMarketArgs,
        operator: Pubkey,
        house: Pubkey,
        house_token: Pubkey,
    ) -> Instruction {
        ix(
            accts::OpenMarket {
                operator,
                house,
                config: config_pda(),
                mint: self.mint,
                house_token,
                market: m.market,
                vault: m.vault,
                fees: m.fees,
                token_program: token::ID,
                system_program: system_program::ID,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::OpenMarket {
                competition_id: m.cid,
                args,
            },
        )
    }

    pub fn open_market_ix(&self, m: &Mkt, args: OpenMarketArgs) -> Instruction {
        self.open_market_ix_with(
            m,
            args,
            self.k.operator.pubkey(),
            self.k.house.pubkey(),
            self.house_token,
        )
    }

    /// Opens a market (the default args unless given) and returns its addresses.
    pub fn open(&mut self, n: u8, args: Option<OpenMarketArgs>) -> Mkt {
        let m = Mkt::of(cid(n));
        let args = args.unwrap_or_else(|| self.args());
        let ix = self.open_market_ix(&m, args);
        let (op, house) = (
            self.k.operator.insecure_clone(),
            self.k.house.insecure_clone(),
        );
        expect_ok(self.send(&[ix], &op, &[&house]), "open_market");
        m
    }

    pub fn set_windows_ix(&self, m: &Mkt, signer: Pubkey, w: [i64; 4]) -> Instruction {
        ix(
            accts::SetWindows {
                operator: signer,
                config: config_pda(),
                market: m.market,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::SetWindows {
                bounty_from: w[0],
                bounty_until: w[1],
                race_from: w[2],
                race_until: w[3],
            },
        )
    }

    pub fn set_windows(&mut self, m: &Mkt, w: [i64; 4]) {
        let ix = self.set_windows_ix(m, self.k.operator.pubkey(), w);
        let op = self.k.operator.insecure_clone();
        expect_ok(self.send(&[ix], &op, &[]), "set_windows");
    }

    /// The bounty window [NOW, LOCK) open.
    pub fn open_bounty(&mut self, m: &Mkt) {
        self.set_windows(m, [NOW, LOCK, 0, 0]);
    }

    pub fn set_sell_until_ix(&self, m: &Mkt, signer: Pubkey, sell_until: i64) -> Instruction {
        ix(
            accts::SetSellUntil {
                operator: signer,
                config: config_pda(),
                market: m.market,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::SetSellUntil { sell_until },
        )
    }

    pub fn set_sell_until(&mut self, m: &Mkt, sell_until: i64) -> Sent {
        let ix = self.set_sell_until_ix(m, self.k.operator.pubkey(), sell_until);
        let op = self.k.operator.insecure_clone();
        expect_ok(self.send(&[ix], &op, &[]), "set_sell_until")
    }

    pub fn add_outcome_ix(&self, m: &Mkt, signer: Pubkey, id: u16, mass: u32) -> Instruction {
        ix(
            accts::AddOutcome {
                operator: signer,
                config: config_pda(),
                market: m.market,
                system_program: system_program::ID,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::AddOutcome {
                outcome_id: id,
                mass_ppm: mass,
            },
        )
    }

    pub fn add(&mut self, m: &Mkt, id: u16, mass: u32) {
        let ix = self.add_outcome_ix(m, self.k.operator.pubkey(), id, mass);
        let op = self.k.operator.insecure_clone();
        expect_ok(self.send(&[ix], &op, &[]), "add_outcome");
    }

    pub fn scratch_ix(&self, m: &Mkt, signer: Pubkey, id: u16) -> Instruction {
        ix(
            accts::ScratchOutcome {
                operator: signer,
                config: config_pda(),
                market: m.market,
                vault: m.vault,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::ScratchOutcome { outcome_id: id },
        )
    }

    pub fn scratch(&mut self, m: &Mkt, id: u16) -> Sent {
        let ix = self.scratch_ix(m, self.k.operator.pubkey(), id);
        let op = self.k.operator.insecure_clone();
        self.send(&[ix], &op, &[])
    }

    pub fn open_ticket_ix(&self, m: &Mkt, signer: Pubkey, wallet: Pubkey) -> Instruction {
        ix(
            accts::OpenTicket {
                operator: signer,
                config: config_pda(),
                market: m.market,
                ticket: ticket_pda(&m.market, &wallet),
                system_program: system_program::ID,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::OpenTicket { wallet },
        )
    }

    pub fn open_ticket(&mut self, m: &Mkt, wallet: Pubkey) {
        let ix = self.open_ticket_ix(m, self.k.operator.pubkey(), wallet);
        let op = self.k.operator.insecure_clone();
        expect_ok(self.send(&[ix], &op, &[]), "open_ticket");
    }

    pub fn revoke_ix(&self, m: &Mkt, signer: Pubkey, wallet: Pubkey, revoked: bool) -> Instruction {
        ix(
            accts::RevokeTicket {
                operator: signer,
                config: config_pda(),
                market: m.market,
                ticket: ticket_pda(&m.market, &wallet),
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::RevokeTicket { revoked },
        )
    }

    /// The trade instruction with every account named (the substitution tests replace one).
    pub fn trade_accounts(
        &self,
        m: &Mkt,
        wallet: Pubkey,
        payer: Pubkey,
        wallet_token: Pubkey,
    ) -> accts::Trade {
        accts::Trade {
            wallet,
            payer,
            config: config_pda(),
            market: m.market,
            mint: self.mint,
            vault: m.vault,
            fees: m.fees,
            ticket: ticket_pda(&m.market, &wallet),
            wallet_token,
            token_program: token::ID,
            system_program: system_program::ID,
            event_authority: event_authority(),
            program: side_market::ID,
        }
    }

    pub fn trade_ix(&self, m: &Mkt, t: &Trader, id: u16, delta: i64, limit: u64) -> Instruction {
        ix(
            self.trade_accounts(m, t.key.pubkey(), t.key.pubkey(), t.token),
            ixs::Trade {
                outcome_id: id,
                delta,
                limit,
            },
        )
    }

    /// A trade the trader signs and pays for, with no limit (u64::MAX on a buy, 0 on a sale).
    pub fn trade(&mut self, m: &Mkt, t: &Trader, id: u16, delta: i64) -> Sent {
        let leg = self
            .anchor::<Ticket>(&ticket_pda(&m.market, &t.key.pubkey()))
            .and_then(|tk| {
                tk.legs
                    .iter()
                    .find(|l| l.outcome_id == id)
                    .map(|l| l.shares)
            })
            .unwrap_or(0);
        let after = leg + delta;
        let buy = after.unsigned_abs() > leg.unsigned_abs();
        let limit = if buy { u64::MAX } else { 0 };
        let ix = self.trade_ix(m, t, id, delta, limit);
        let key = t.key.insecure_clone();
        self.send(&[cu_limit(1_400_000), ix], &key, &[])
    }

    pub fn halt_ix(&self, m: &Mkt, signer: Pubkey) -> Instruction {
        ix(
            accts::Halt {
                signer,
                config: config_pda(),
                market: m.market,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::Halt {},
        )
    }

    pub fn resume_ix(&self, m: &Mkt, signer: Pubkey) -> Instruction {
        ix(
            accts::Resume {
                signer,
                config: config_pda(),
                market: m.market,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::Resume {},
        )
    }

    pub fn propose_ix(
        &self,
        m: &Mkt,
        signer: Pubkey,
        winner: u16,
        result: [u8; 32],
        escrow: Option<Pubkey>,
    ) -> Instruction {
        ix(
            accts::Propose {
                resolver: signer,
                config: config_pda(),
                market: m.market,
                escrow,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::Propose {
                winner,
                result_sha256: result,
            },
        )
    }

    pub fn propose(&mut self, m: &Mkt, winner: u16) -> Sent {
        let ix = self.propose_ix(m, self.k.resolver.pubkey(), winner, [7; 32], None);
        let r = self.k.resolver.insecure_clone();
        self.send(&[ix], &r, &[])
    }

    pub fn override_ix(
        &self,
        m: &Mkt,
        signer: Pubkey,
        winner: u16,
        result: [u8; 32],
    ) -> Instruction {
        ix(
            accts::OverrideResult {
                arbiter: signer,
                config: config_pda(),
                market: m.market,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::OverrideResult {
                winner,
                result_sha256: result,
            },
        )
    }

    pub fn void_ix(&self, m: &Mkt, signer: Pubkey) -> Instruction {
        ix(
            accts::VoidMarket {
                signer,
                config: config_pda(),
                market: m.market,
                vault: m.vault,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::VoidMarket {},
        )
    }

    pub fn finalize_ix(&self, m: &Mkt) -> Instruction {
        ix(
            accts::Finalize {
                market: m.market,
                vault: m.vault,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::Finalize {},
        )
    }

    pub fn settle_accounts(&self, m: &Mkt, caller: Pubkey, wallet: Pubkey) -> accts::SettleTicket {
        accts::SettleTicket {
            caller,
            market: m.market,
            mint: self.mint,
            vault: m.vault,
            wallet,
            wallet_token: get_associated_token_address(&wallet, &self.mint),
            ticket: ticket_pda(&m.market, &wallet),
            ticket_payer: self.k.operator.pubkey(),
            token_program: token::ID,
            associated_token_program: associated_token::ID,
            system_program: system_program::ID,
            event_authority: event_authority(),
            program: side_market::ID,
        }
    }

    pub fn settle_ix(&self, m: &Mkt, caller: Pubkey, wallet: Pubkey) -> Instruction {
        ix(
            self.settle_accounts(m, caller, wallet),
            ixs::SettleTicket {},
        )
    }

    /// settle_ticket, the stranger paying: returns what the wallet's ATA gained.
    pub fn settle(&mut self, m: &Mkt, wallet: Pubkey) -> u64 {
        let ata = get_associated_token_address(&wallet, &self.mint);
        let before = self.tokens(&ata);
        let ix = self.settle_ix(m, self.k.stranger.pubkey(), wallet);
        let s = self.k.stranger.insecure_clone();
        expect_ok(
            self.send(&[cu_limit(1_400_000), ix], &s, &[]),
            "settle_ticket",
        );
        self.tokens(&ata) - before
    }

    pub fn sweep_ix(&self, m: &Mkt, signer: Pubkey, house_token: Pubkey) -> Instruction {
        ix(
            accts::SweepFees {
                operator: signer,
                config: config_pda(),
                market: m.market,
                mint: self.mint,
                fees: m.fees,
                house_token,
                token_program: token::ID,
                event_authority: event_authority(),
                program: side_market::ID,
            },
            ixs::SweepFees {},
        )
    }

    pub fn close_accounts(&self, m: &Mkt, signer: Pubkey) -> accts::CloseMarket {
        accts::CloseMarket {
            operator: signer,
            config: config_pda(),
            market: m.market,
            rent_to: self.k.operator.pubkey(),
            mint: self.mint,
            vault: m.vault,
            fees: m.fees,
            house_token: self.house_token,
            token_program: token::ID,
            event_authority: event_authority(),
            program: side_market::ID,
        }
    }

    pub fn close_ix(&self, m: &Mkt, signer: Pubkey) -> Instruction {
        ix(self.close_accounts(m, signer), ixs::CloseMarket {})
    }

    /// Sends `ix` signed by the operator alone.
    pub fn as_operator(&mut self, ix: Instruction) -> Sent {
        let op = self.k.operator.insecure_clone();
        self.send(&[ix], &op, &[])
    }

    /// Sends `ix` signed by `signer` (who pays).
    pub fn as_key(&mut self, ix: Instruction, signer: &Keypair) -> Sent {
        let s = signer.insecure_clone();
        self.send(&[ix], &s, &[])
    }

    // ------------------------------------------------------------------ checks

    /// D6 from the outside: the vault token account's real balance against the liability this test
    /// recomputes from the decoded Market's raw fields (liability_of), never the program's own
    /// liability(); the two must also agree. Returns the liability.
    #[track_caller]
    pub fn assert_d6(&self, m: &Mkt) -> u128 {
        let market = self.market(m);
        let vault = self.tokens(&m.vault);
        let owed = liability_of(&market);
        assert_eq!(
            market.liability().expect("liability"),
            owed,
            "the program's liability() and the test's recomputation disagree ({:?})",
            market.status
        );
        assert!(
            u128::from(vault) >= owed,
            "D6: vault {vault} < liability {owed} ({:?})",
            market.status
        );
        owed
    }

    /// Overwrites a classic SPL token account's balance (the LiteSVM account setter): a known-bad
    /// state for the Insolvent tests.
    pub fn set_tokens(&mut self, address: &Pubkey, amount: u64) {
        let mut account = self.svm.get_account(address).expect("the token account");
        let mut parsed = spl_token::state::Account::unpack(&account.data).expect("a token account");
        parsed.amount = amount;
        parsed.pack_into_slice(&mut account.data);
        self.svm.set_account(*address, account).unwrap();
    }

    /// Every live outcome's price in ppm, by id.
    pub fn prices(&self, m: &Mkt) -> Vec<(u16, u32)> {
        let market = self.market(m);
        market
            .outcomes
            .iter()
            .filter(|o| o.is_live())
            .map(|o| (o.id, math::price_ppm(o.w, market.sum).unwrap()))
            .collect()
    }

    /// An account meta list with the account at `index` replaced by `with` (writability kept).
    pub fn swap_meta(mut metas: Vec<AccountMeta>, index: usize, with: Pubkey) -> Vec<AccountMeta> {
        metas[index].pubkey = with;
        metas
    }
}
