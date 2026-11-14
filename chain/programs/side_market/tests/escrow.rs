//! D10's escrow cross-check against a real prize escrow: prize_escrow's release build
//! (tests/program_so prize_escrow_bytes) runs beside side_market, creates its Config and one
//! competition's escrow, commits the entries, posts the result and settles it (Claimable), all by
//! hand-encoded instructions (this crate never depends on another program's), and side_market's
//! propose reads that account by its discriminator and layout (constants ESCROW_*).

mod common;
mod mainnet;
mod program_so;

use {
    anchor_lang::solana_program::{
        bpf_loader_upgradeable,
        instruction::{AccountMeta, Instruction},
        system_program,
    },
    anchor_spl::{associated_token, token},
    common::*,
    side_market::{
        SideMarketError, ESCROW_COMPETITION_ID_OFFSET, ESCROW_DISCRIMINATOR, ESCROW_RESULT_OFFSET,
        ESCROW_STATUS_CLAIMABLE, ESCROW_STATUS_OFFSET, FIRST_ENTRY_ID,
    },
    solana_keypair::Keypair,
    solana_signer::Signer,
};

use anchor_lang::prelude::Pubkey;

/// prize_escrow's instruction discriminators (chain/idl/prize_escrow.json).
const INITIALIZE_CONFIG: [u8; 8] = [208, 127, 21, 1, 194, 190, 196, 70];
const OPEN_ESCROW: [u8; 8] = [82, 178, 155, 253, 74, 41, 161, 219];
const COMMIT_ENTRIES: [u8; 8] = [21, 44, 252, 51, 214, 90, 243, 177];
const POST_RESULT: [u8; 8] = [209, 11, 193, 110, 192, 1, 142, 9];
const SETTLE: [u8; 8] = [175, 42, 185, 87, 144, 131, 102, 212];

/// prize_escrow's release MIN_REVEAL_GRACE (5 minutes).
const REVEAL_GRACE: i64 = 5 * 60;
const HOUR: i64 = 60 * 60;
const RESULT: [u8; 32] = [9; 32];
const ESCROW_TOTAL: u64 = 1_000_000_000;

/// A Borsh writer for the hand-encoded arguments.
#[derive(Default)]
struct Enc(Vec<u8>);

impl Enc {
    fn bytes(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(b);
        self
    }
    fn u8(self, v: u8) -> Self {
        self.bytes(&[v])
    }
    fn u32(self, v: u32) -> Self {
        self.bytes(&v.to_le_bytes())
    }
    fn u64(self, v: u64) -> Self {
        self.bytes(&v.to_le_bytes())
    }
    fn i64(self, v: i64) -> Self {
        self.bytes(&v.to_le_bytes())
    }
    fn key(self, k: &Pubkey) -> Self {
        self.bytes(k.as_ref())
    }
    fn keys(self, ks: &[Pubkey]) -> Self {
        let mut e = self.u32(ks.len() as u32);
        for k in ks {
            e = e.key(k);
        }
        e
    }
}

fn pe(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &prize_escrow_id()).0
}

fn meta(key: Pubkey, writable: bool, signer: bool) -> AccountMeta {
    if writable {
        AccountMeta::new(key, signer)
    } else {
        AccountMeta::new_readonly(key, signer)
    }
}

struct Escrow {
    sponsor: Keypair,
    resolver: Keypair,
    address: Pubkey,
    cid: [u8; 16],
}

/// prize_escrow's Config, then one escrow for `cid` taken to Claimable on RESULT.
fn claimable_escrow(w: &mut World, cid: [u8; 16]) -> Escrow {
    let pid = prize_escrow_id();
    w.svm
        .add_program(pid, program_so::prize_escrow_bytes())
        .expect("add prize_escrow");
    let authority = Keypair::new();
    w.svm.airdrop(&authority.pubkey(), 100 * SOL).unwrap();
    w.set_upgrade_authority(&pid, Some(authority.pubkey()));

    let sponsor = Keypair::new();
    let resolver = Keypair::new();
    let compliance = Pubkey::new_unique();
    let arbiter = Pubkey::new_unique();
    let regraders = [
        Pubkey::new_unique(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
    ];
    for k in [&sponsor, &resolver] {
        w.svm.airdrop(&k.pubkey(), 100 * SOL).unwrap();
    }
    let config = pe(&[b"config"]);
    let events = pe(&[b"__event_authority"]);
    let program_data = Pubkey::find_program_address(&[pid.as_ref()], &bpf_loader_upgradeable::ID).0;

    // initialize_config: open limits, no floors, the mint, the sponsor and three regraders.
    let mut init = Enc::default()
        .bytes(&INITIALIZE_CONFIG)
        .key(&Pubkey::new_unique())
        .key(&Pubkey::new_unique())
        .key(&arbiter)
        .u64(u64::MAX / 4)
        .u64(u64::MAX / 2)
        .u64(u64::MAX / 4)
        .i64(DAY)
        .u64(0)
        .u8(5)
        .i64(0)
        .i64(0)
        .i64(0)
        .i64(0)
        .i64(REVEAL_GRACE)
        .keys(&[w.mint])
        .keys(&[sponsor.pubkey()])
        .u32(3);
    for r in &regraders {
        init = init.key(r).u8(0);
    }
    let ix = Instruction {
        program_id: pid,
        accounts: vec![
            meta(authority.pubkey(), true, true),
            meta(config, true, false),
            meta(program_data, false, false),
            meta(system_program::ID, false, false),
            meta(events, false, false),
            meta(pid, false, false),
        ],
        data: init.0,
    };
    expect_ok(
        w.send(&[ix], &authority, &[]),
        "prize_escrow initialize_config",
    );

    // open_escrow: 1,000 USDC, every window at the release minimums.
    let escrow = pe(&[b"escrow", sponsor.pubkey().as_ref(), &cid]);
    let vault = associated_token::get_associated_token_address(&escrow, &w.mint);
    let sponsor_token = Pubkey::new_unique();
    let mint = w.mint;
    w.put_token_account(sponsor_token, mint, sponsor.pubkey(), ESCROW_TOTAL);
    let now = w.time();
    let entries_deadline = now + DAY;
    let final_end = now + 2 * DAY;
    let resolve_deadline = now + 3 * DAY;
    let (challenge, attest, hold, claim) = (24 * HOUR, 24 * HOUR, 24 * HOUR, 8 * DAY);
    let hard = resolve_deadline + challenge + attest + hold + claim + 7 * DAY + DAY;
    let open = Enc::default()
        .bytes(&OPEN_ESCROW)
        .bytes(&cid)
        .u64(ESCROW_TOTAL)
        .bytes(&[1; 32])
        .key(&resolver.pubkey())
        .key(&compliance)
        .key(&arbiter)
        .key(&sponsor.pubkey())
        .keys(&regraders)
        .u8(2)
        .u64(10_000_000)
        .i64(entries_deadline)
        .i64(final_end)
        .i64(resolve_deadline)
        .i64(challenge)
        .i64(attest)
        .i64(hold)
        .i64(claim)
        .i64(hard);
    let ix = Instruction {
        program_id: pid,
        accounts: vec![
            meta(sponsor.pubkey(), true, true),
            meta(escrow, true, false),
            meta(w.mint, false, false),
            meta(sponsor_token, true, false),
            meta(vault, true, false),
            meta(token::ID, false, false),
            meta(associated_token::ID, false, false),
            meta(system_program::ID, false, false),
            meta(config, true, false),
            meta(events, false, false),
            meta(pid, false, false),
        ],
        data: open.0,
    };
    expect_ok(w.send(&[ix], &sponsor, &[]), "prize_escrow open_escrow");

    // commit_entries, then post_result once the entries deadline passes.
    let ix = Instruction {
        program_id: pid,
        accounts: vec![
            meta(resolver.pubkey(), false, true),
            meta(escrow, true, false),
            meta(config, false, false),
            meta(events, false, false),
            meta(pid, false, false),
        ],
        data: Enc::default()
            .bytes(&COMMIT_ENTRIES)
            .bytes(&[4; 32])
            .u32(2)
            .0,
    };
    expect_ok(w.send(&[ix], &resolver, &[]), "prize_escrow commit_entries");
    w.set_time(entries_deadline);
    let post = Instruction {
        program_id: pid,
        accounts: vec![
            meta(resolver.pubkey(), true, true),
            meta(escrow, true, false),
            meta(w.mint, false, false),
            meta(vault, false, false),
            meta(token::ID, false, false),
            meta(system_program::ID, false, false),
            meta(config, false, false),
            meta(events, false, false),
            meta(pid, false, false),
        ],
        data: Enc::default()
            .bytes(&POST_RESULT)
            .bytes(&[2; 32])
            .bytes(&RESULT)
            .bytes(&[3; 32])
            .u64(ESCROW_TOTAL)
            .0,
    };
    expect_ok(w.send(&[post], &resolver, &[]), "prize_escrow post_result");

    Escrow {
        sponsor,
        resolver,
        address: escrow,
        cid,
    }
}

/// prize_escrow's settle of an unchallenged result past its challenge window: Claimable.
fn settle_escrow(w: &mut World, e: &Escrow) {
    let pid = prize_escrow_id();
    let vault = associated_token::get_associated_token_address(&e.address, &w.mint);
    let ix = Instruction {
        program_id: pid,
        accounts: vec![
            meta(e.address, true, false),
            meta(w.mint, false, false),
            meta(vault, false, false),
            meta(e.resolver.pubkey(), true, false),
            // No challenger: the optional account is the program id.
            meta(pid, false, false),
            meta(token::ID, false, false),
            meta(pe(&[b"config"]), false, false),
            meta(pe(&[b"__event_authority"]), false, false),
            meta(pid, false, false),
        ],
        data: SETTLE.to_vec(),
    };
    let payer = Keypair::new();
    w.svm.airdrop(&payer.pubkey(), SOL).unwrap();
    expect_ok(w.send(&[ix], &payer, &[]), "prize_escrow settle");
    let _ = &e.sponsor;
}

#[test]
fn propose_reads_a_real_escrow() {
    let mut w = World::new();
    let id = cid(42);
    let e = claimable_escrow(&mut w, id);

    // The market names the escrow; its deadline before the escrow's settle.
    let mut args = w.args();
    args.escrow = e.address;
    args.lock_ts = w.time() + 60;
    let m = w.open(42, Some(args));
    assert_eq!(m.cid, e.cid);
    w.add(&m, FIRST_ENTRY_ID, 100_000);
    let t = w.time();
    w.set_time(t + 120);

    // Still Challengeable: refused.
    let ix = w.propose_ix(
        &m,
        w.k.resolver.pubkey(),
        FIRST_ENTRY_ID,
        RESULT,
        Some(e.address),
    );
    let r = w.k.resolver.insecure_clone();
    let sent = w.send(&[ix], &r, &[]);
    expect_code(
        &sent,
        SideMarketError::EscrowNotSettled,
        "propose on a Challengeable escrow",
    );

    // Settled past its challenge window: Claimable, the layout the constants name.
    let t = w.time();
    w.set_time(t + DAY + 1);
    settle_escrow(&mut w, &e);
    let data = w.svm.get_account(&e.address).unwrap().data;
    assert_eq!(data[..8], ESCROW_DISCRIMINATOR);
    assert_eq!(data[ESCROW_STATUS_OFFSET], ESCROW_STATUS_CLAIMABLE);
    assert_eq!(
        data[ESCROW_COMPETITION_ID_OFFSET..ESCROW_COMPETITION_ID_OFFSET + 16],
        id
    );
    assert_eq!(
        data[ESCROW_RESULT_OFFSET..ESCROW_RESULT_OFFSET + 32],
        RESULT
    );

    // No escrow account: refused.
    let ix = w.propose_ix(&m, w.k.resolver.pubkey(), FIRST_ENTRY_ID, RESULT, None);
    let sent = w.send(&[ix], &r, &[]);
    expect_code(
        &sent,
        SideMarketError::EscrowRequired,
        "propose without the escrow",
    );

    // Another account in its place: refused.
    let fake = Pubkey::new_unique();
    let mut account = w.svm.get_account(&e.address).unwrap();
    w.svm.set_account(fake, account.clone()).unwrap();
    let ix = w.propose_ix(
        &m,
        w.k.resolver.pubkey(),
        FIRST_ENTRY_ID,
        RESULT,
        Some(fake),
    );
    let sent = w.send(&[ix], &r, &[]);
    expect_code(
        &sent,
        SideMarketError::EscrowMismatch,
        "propose with another escrow account",
    );

    // The same bytes under another owner at the named address: refused.
    let real = account.clone();
    account.owner = Pubkey::new_unique();
    w.svm.set_account(e.address, account).unwrap();
    let ix = w.propose_ix(
        &m,
        w.k.resolver.pubkey(),
        FIRST_ENTRY_ID,
        RESULT,
        Some(e.address),
    );
    let sent = w.send(&[ix], &r, &[]);
    expect_code(
        &sent,
        SideMarketError::EscrowMismatch,
        "propose with an escrow of another owner",
    );
    w.svm.set_account(e.address, real).unwrap();

    // Another result: refused.
    let ix = w.propose_ix(
        &m,
        w.k.resolver.pubkey(),
        FIRST_ENTRY_ID,
        [8; 32],
        Some(e.address),
    );
    let sent = w.send(&[ix], &r, &[]);
    expect_code(
        &sent,
        SideMarketError::EscrowNotSettled,
        "propose another result",
    );

    // The escrow's own result: proposed.
    let ix = w.propose_ix(
        &m,
        w.k.resolver.pubkey(),
        FIRST_ENTRY_ID,
        RESULT,
        Some(e.address),
    );
    expect_ok(w.send(&[ix], &r, &[]), "propose with the escrow");
    let market = w.market(&m);
    assert_eq!(market.winner, FIRST_ENTRY_ID);
    assert_eq!(market.result_sha256, RESULT);
}
