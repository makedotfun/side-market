//! The independent review's fixes on the release build (LiteSVM under mainnet's feature set): a
//! scratch of a hedged book pays its YES 0 and its NO 1 a share whatever the net (F1, under the
//! rule that a scratched entry loses); settle_ticket pays into
//! any token account of the mint the ticket's wallet owns when its associated token account was
//! reassigned (F2), and creates nothing for a payout of 0; anyone's void waits for every trading
//! window to close; propose waits out a guardian's halt; a trade to i64::MIN NO shares is an error,
//! never a panic. And for the rule that a position can be sold whenever the market is open: a
//! sell outside every window and anyone's void never land in one transaction (sells stop at
//! void_after, anyone's void starts there); and the known-bad state, a vault short of the
//! liability, refuses a 1-unit sell, finalize and settle_ticket as Insolvent, changing nothing.

mod common;
mod mainnet;
mod program_so;

use {
    anchor_lang::prelude::Pubkey,
    anchor_spl::{associated_token::get_associated_token_address, token},
    common::*,
    side_market::{
        instruction as ixs, math, MarketStatus, SideMarketError, FIRST_ENTRY_ID, NONE_ID,
        VOID_GRACE,
    },
    solana_signer::Signer,
};

const E1: u16 = FIRST_ENTRY_ID;
const USDC: u64 = 1_000_000;

fn trader(w: &mut World, m: &Mkt, amount: u64) -> Trader {
    let t = w.trader(amount);
    w.open_ticket(m, t.key.pubkey());
    t
}

/// A market with one entry at mass 0.5 (NONE 0.05, FIELD 0.45), the bounty window open.
fn market1(w: &mut World, n: u8) -> Mkt {
    let m = w.open(n, None);
    w.add(&m, E1, 500_000);
    w.open_bounty(&m);
    m
}

/// Proposes `winner` at lock_ts and finalizes it after the dispute delay.
fn finalize(w: &mut World, m: &Mkt, winner: u16) {
    w.set_time(LOCK);
    expect_ok(w.propose(m, winner), "propose");
    w.set_time(LOCK + DISPUTE);
    expect_ok(w.as_operator(w.finalize_ix(m)), "finalize");
}

/// settle_ticket for `wallet`'s ticket paying into `to`, the stranger calling.
fn settle_into(w: &mut World, m: &Mkt, wallet: Pubkey, to: Pubkey) -> Sent {
    let s = w.k.stranger.insecure_clone();
    let mut acc = w.settle_accounts(m, s.pubkey(), wallet);
    acc.wallet_token = to;
    let ix = ix(acc, ixs::SettleTicket {});
    w.send(&[cu_limit(1_400_000), ix], &s, &[])
}

#[test]
fn a_hedged_scratch_pays_yes_nothing_and_no_one_whatever_the_net() {
    for (n, net) in [
        (30u8, 0i64),
        (31, 1),
        (32, -1),
        (33, 3),
        (34, -3),
        (35, 100),
        (36, -100),
        (37, 1_000_000),
        (38, -1_000_000),
        (39, 40_000_000),
    ] {
        let mut w = World::new();
        let m = market1(&mut w, n);
        let a = trader(&mut w, &m, 10_000 * USDC);
        let b = trader(&mut w, &m, 10_000 * USDC);
        // 100 USDC of YES from a, NO from b down to a net q of `net`.
        let y = 100 * USDC as i64;
        expect_ok(w.trade(&m, &a, E1, y), "YES");
        expect_ok(w.trade(&m, &b, E1, -(y - net)), "NO");
        let before = w.market(&m);
        let k = before.index_of(E1).unwrap();
        assert_eq!(before.outcomes[k].q, net);
        let liability = w.assert_d6(&m);

        expect_ok(w.scratch(&m, E1), "scratch");
        assert!(
            w.assert_d6(&m) <= liability,
            "net {net}: the liability rose"
        );
        let o = w.market(&m).outcomes[k];
        assert_eq!((o.yes_value, o.no_value), (0, u64::MAX), "net {net}");
        assert_eq!(w.market(&m).scratch_owed, (y - net) as u64, "net {net}: F");

        // NONE wins: the scratched YES is paid nothing, every NO share 1.
        finalize(&mut w, &m, NONE_ID);
        let got_a = w.settle(&m, a.key.pubkey());
        let got_b = w.settle(&m, b.key.pubkey());
        assert_eq!(got_a, 0, "net {net}");
        assert_eq!(got_b, (y - net) as u64, "net {net}");
        assert_eq!(got_a, math::value_of(y as u64, o.yes_value));
        assert_eq!(got_b, math::value_of((y - net) as u64, o.no_value));
        assert_eq!(w.market(&m).scratch_owed, 0, "net {net}: F paid out");
        w.assert_d6(&m);
        let market = w.market(&m);
        let surplus = w.tokens(&m.vault);
        expect_ok(
            w.as_operator(w.close_ix(&m, w.k.operator.pubkey())),
            "close",
        );
        // The house keeps the unwind R when q_k >= 0; when q_k < 0 FIELD takes k's current
        // weight, so C does not move and the house's LMSR bound stands as it was.
        let loss = market.deposit.saturating_sub(surplus);
        let bound = math::max_loss(market.b, math::loss_per_b(M_FLOOR).unwrap()).unwrap();
        assert!(loss <= bound, "net {net}: house lost {loss} > {bound}");
    }
}

#[test]
fn settle_pays_another_account_of_the_wallet_when_its_ata_was_reassigned() {
    let mut w = World::new();
    let m = market1(&mut w, 40);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let b = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "a YES");
    expect_ok(w.trade(&m, &b, E1, 4 * USDC as i64), "b YES");
    finalize(&mut w, &m, E1);

    let wallet = a.key.pubkey();
    let mint = w.mint;
    let ata = get_associated_token_address(&wallet, &mint);
    // The state SetAuthority(AccountOwner) leaves: classic SPL Token lets a wallet hand its own
    // ATA to another key (spl-token 3.5 processor.rs:435-449). init_if_needed with
    // associated_token::authority = wallet refused this ticket for good.
    let elsewhere = Pubkey::new_unique();
    w.put_token_account(ata, mint, elsewhere, 0);
    let mismatch = SideMarketError::TokenAccountMismatch;
    expect_code(
        &settle_into(&mut w, &m, wallet, ata),
        mismatch,
        "the reassigned ATA",
    );

    // Refused: another mint's account of the wallet, the mint's account of someone else, a
    // Token-2022 account, an address that is neither a token account nor the ATA, the vault.
    let other_mint = Pubkey::new_unique();
    w.put_mint(other_mint, 6, token::ID);
    let wrong_mint = Pubkey::new_unique();
    w.put_token_account(wrong_mint, other_mint, wallet, 0);
    let strangers = Pubkey::new_unique();
    w.put_token_account(strangers, mint, w.k.stranger.pubkey(), 0);
    let t22 = Pubkey::new_unique();
    w.put_token_account_owned(t22, mint, wallet, 0, TOKEN_2022_ID.parse().unwrap());
    for (to, what) in [
        (wrong_mint, "another mint"),
        (strangers, "another owner"),
        (t22, "a Token-2022 account"),
        (Pubkey::new_unique(), "not a token account, not the ATA"),
        (m.vault, "the vault"),
    ] {
        expect_code(&settle_into(&mut w, &m, wallet, to), mismatch, what);
    }

    // A second token account of the mint the wallet owns takes the payout; the ticket closes.
    let second = Pubkey::new_unique();
    w.put_token_account(second, mint, wallet, 0);
    expect_ok(
        settle_into(&mut w, &m, wallet, second),
        "settle into a second account",
    );
    assert_eq!(w.tokens(&second), 10 * USDC);
    assert_eq!(w.tokens(&ata), 0);
    assert!(!w.exists(&ticket_pda(&m.market, &wallet)));
    w.assert_d6(&m);

    // b's own funded account (not an ATA) is a destination too.
    let before = w.tokens(&b.token);
    expect_ok(
        settle_into(&mut w, &m, b.key.pubkey(), b.token),
        "settle into the trader's account",
    );
    assert_eq!(w.tokens(&b.token) - before, 4 * USDC);

    // Every ticket settled: the market closes and the house surplus is not locked.
    assert_eq!(w.market(&m).tickets, 0);
    expect_ok(
        w.as_operator(w.close_ix(&m, w.k.operator.pubkey())),
        "close",
    );
}

#[test]
fn a_settlement_of_nothing_creates_no_account() {
    let mut w = World::new();
    let m = market1(&mut w, 41);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let b = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "a YES E1");
    expect_ok(w.trade(&m, &b, NONE_ID, 3 * USDC as i64), "b YES NONE");
    finalize(&mut w, &m, NONE_ID);

    let s = w.k.stranger.pubkey();
    let ata = get_associated_token_address(&a.key.pubkey(), &w.mint);
    let lamports = w.lamports(&s);
    let vault = w.tokens(&m.vault);
    assert_eq!(w.settle(&m, a.key.pubkey()), 0, "a lost");
    assert!(!w.exists(&ata), "no ATA for a payout of 0");
    assert!(!w.exists(&ticket_pda(&m.market, &a.key.pubkey())));
    assert_eq!(w.tokens(&m.vault), vault, "nothing moved");
    let rent = w.svm.minimum_balance_for_rent_exemption(165);
    assert!(
        lamports - w.lamports(&s) < rent,
        "the caller paid no account's rent"
    );
    w.assert_d6(&m);

    // A winner's ATA is still created when missing, the caller paying.
    let b_ata = get_associated_token_address(&b.key.pubkey(), &w.mint);
    assert!(!w.exists(&b_ata));
    assert_eq!(w.settle(&m, b.key.pubkey()), 3 * USDC);
    assert!(w.exists(&b_ata));
    assert_eq!(w.market(&m).tickets, 0);
}

#[test]
fn anyone_voids_only_while_no_window_is_open() {
    let mut w = World::new();
    let m = market1(&mut w, 42);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    let at = LOCK + VOID_GRACE;
    w.set_time(at);
    // The operator opens a race window over void_after (allowed any time before propose).
    w.set_windows(&m, [NOW, LOCK, at - 100, at + DAY]);
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(w.void_ix(&m, s.pubkey()), &s),
        SideMarketError::VoidWhileTrading,
        "anyone's void with the race window open",
    );
    // A buy and a void in one transaction: refused the same way.
    let buy = w.trade_ix(&m, &a, E1, USDC as i64, u64::MAX);
    let void = w.void_ix(&m, a.key.pubkey());
    let key = a.key.insecure_clone();
    expect_code(
        &w.send(&[cu_limit(1_400_000), buy, void], &key, &[]),
        SideMarketError::VoidWhileTrading,
        "buy then void",
    );
    // The arbiter still may.
    let mut f = w.fork();
    let arb = f.k.arbiter.insecure_clone();
    expect_ok(f.as_key(f.void_ix(&m, arb.pubkey()), &arb), "arbiter void");
    // Once the window closes, anyone may.
    w.set_time(at + DAY);
    expect_ok(
        w.as_key(w.void_ix(&m, s.pubkey()), &s),
        "void after the window",
    );
    assert_eq!(w.market(&m).status, MarketStatus::Void);
    w.assert_d6(&m);
}

#[test]
fn propose_waits_out_a_guardian_halt() {
    let mut w = World::new();
    let m = market1(&mut w, 43);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    w.set_time(LOCK);
    let g = w.k.guardian.insecure_clone();
    expect_ok(w.as_key(w.halt_ix(&m, g.pubkey()), &g), "guardian halt");
    expect_code(
        &w.propose(&m, E1),
        SideMarketError::InvalidStatus,
        "propose under a guardian halt",
    );
    // The arbiter may still void it.
    let mut f = w.fork();
    let arb = f.k.arbiter.insecure_clone();
    expect_ok(f.as_key(f.void_ix(&m, arb.pubkey()), &arb), "arbiter void");

    // The admin lifts it; an operator's halt does not stop propose.
    let admin = w.k.admin.insecure_clone();
    expect_ok(
        w.as_key(w.resume_ix(&m, admin.pubkey()), &admin),
        "admin resume",
    );
    expect_ok(
        w.as_operator(w.halt_ix(&m, w.k.operator.pubkey())),
        "operator halt",
    );
    expect_ok(w.propose(&m, E1), "propose under the operator's halt");
    assert_eq!(w.market(&m).status, MarketStatus::Proposed);
}

#[test]
fn a_trade_to_i64_min_no_shares_is_an_error_not_a_panic() {
    let mut w = World::new();
    let m = market1(&mut w, 44);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_code(
        &w.trade(&m, &a, E1, i64::MIN),
        SideMarketError::MathOverflow,
        "NO to i64::MIN",
    );
    w.assert_d6(&m);
}

#[test]
fn a_sell_and_a_permissionless_void_never_land_in_one_transaction() {
    let mut w = World::new();
    let m = market1(&mut w, 45);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    let void_after = w.market(&m).void_after;
    assert_eq!(void_after, LOCK + VOID_GRACE);
    let key = a.key.insecure_clone();
    let sell = |w: &World| w.trade_ix(&m, &a, E1, -(USDC as i64), 0);
    // Anyone's void, signed by the seller itself.
    let void = |w: &World| w.void_ix(&m, a.key.pubkey());

    // One second before void_after, outside every window: the sell lands alone, anyone's void
    // does not yet, so the pair fails in either order.
    w.set_time(void_after - 1);
    let mut f = w.fork();
    expect_ok(
        f.trade(&m, &a, E1, -(USDC as i64)),
        "a sell before void_after",
    );
    f.assert_d6(&m);
    expect_code(
        &w.send(&[cu_limit(1_400_000), sell(&w), void(&w)], &key, &[]),
        SideMarketError::VoidNotYet,
        "a sell then anyone's void before void_after",
    );
    expect_code(
        &w.send(&[cu_limit(1_400_000), void(&w), sell(&w)], &key, &[]),
        SideMarketError::VoidNotYet,
        "anyone's void then a sell before void_after",
    );

    // At void_after: the sell is refused (a sell_until later than void_after does not reach past
    // it), so the pair fails in either order; anyone's void lands alone.
    w.set_time(void_after);
    w.set_sell_until(&m, void_after + DAY);
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::TradingClosed,
        "a sell at void_after",
    );
    expect_code(
        &w.send(&[cu_limit(1_400_000), sell(&w), void(&w)], &key, &[]),
        SideMarketError::TradingClosed,
        "a sell then anyone's void at void_after",
    );
    expect_code(
        &w.send(&[cu_limit(1_400_000), void(&w), sell(&w)], &key, &[]),
        SideMarketError::InvalidStatus,
        "anyone's void then a sell at void_after",
    );

    // A race window open over void_after keeps today's rule: inside it a sell lands (as a buy
    // does), and anyone's void waits for it to close.
    let mut g = w.fork();
    g.set_windows(&m, [NOW, LOCK, void_after - 100, void_after + DAY]);
    expect_ok(
        g.trade(&m, &a, E1, -(USDC as i64)),
        "a sell inside a race window open over void_after",
    );
    expect_code(
        &g.send(&[cu_limit(1_400_000), sell(&g), void(&g)], &key, &[]),
        SideMarketError::VoidWhileTrading,
        "a sell then anyone's void inside the race window",
    );

    expect_ok(w.as_key(void(&w), &key), "anyone's void at void_after");
    assert_eq!(w.market(&m).status, MarketStatus::Void);
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::InvalidStatus,
        "a sell once void",
    );
    w.assert_d6(&m);
}

/// Every account a value-moving instruction can write, as (lamports, data).
fn snapshot(w: &World, addresses: &[Pubkey]) -> Vec<Option<(u64, Vec<u8>)>> {
    addresses
        .iter()
        .map(|a| w.svm.get_account(a).map(|x| (x.lamports, x.data)))
        .collect()
}

/// Sends `ixs` signed by `signer` and expects Insolvent, with every account in `watched` as it was.
#[track_caller]
fn refused_insolvent(
    w: &mut World,
    ixs: &[anchor_lang::solana_program::instruction::Instruction],
    signer: &solana_keypair::Keypair,
    watched: &[Pubkey],
    what: &str,
) {
    let before = snapshot(w, watched);
    let s = signer.insecure_clone();
    let sent = w.send(ixs, &s, &[]);
    expect_code(&sent, SideMarketError::Insolvent, what);
    assert!(snapshot(w, watched) == before, "{what}: the state changed");
}

#[test]
fn a_vault_short_of_the_liability_refuses_sells_finalize_and_settle_as_insolvent() {
    let mut w = World::new();
    let m = market1(&mut w, 46);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let b = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "a YES E1");
    expect_ok(w.trade(&m, &b, E1, 4 * USDC as i64), "b YES E1");
    expect_ok(w.trade(&m, &b, NONE_ID, -(3 * USDC as i64)), "b NO NONE");
    // F + N + max q = 0 + 3 + 14 USDC.
    let owed = w.assert_d6(&m);
    assert_eq!(owed, u128::from(17 * USDC));
    let honest = w.tokens(&m.vault);
    let s = w.k.stranger.insecure_clone();
    let (a_ata, b_ata) = (
        get_associated_token_address(&a.key.pubkey(), &w.mint),
        get_associated_token_address(&b.key.pubkey(), &w.mint),
    );
    let watched = [
        m.market,
        m.vault,
        m.fees,
        ticket_pda(&m.market, &a.key.pubkey()),
        ticket_pda(&m.market, &b.key.pubkey()),
        a.token,
        b.token,
        a_ata,
        b_ata,
    ];

    // The known-bad state: the vault 1,000 base units short of the liability.
    let short = u64::try_from(owed).unwrap() - 1_000;
    w.set_tokens(&m.vault, short);
    let sell_a = w.trade_ix(&m, &a, E1, -1, 0);
    refused_insolvent(
        &mut w,
        &[cu_limit(1_400_000), sell_a.clone()],
        &a.key,
        &watched,
        "a 1-unit sell inside the window",
    );
    w.set_time(LOCK + DAY);
    refused_insolvent(
        &mut w,
        &[cu_limit(1_400_000), sell_a],
        &a.key,
        &watched,
        "a 1-unit sell outside every window",
    );
    let sell_b = w.trade_ix(&m, &b, NONE_ID, 1, 0);
    refused_insolvent(
        &mut w,
        &[cu_limit(1_400_000), sell_b],
        &b.key,
        &watched,
        "a 1-unit NO sell outside every window",
    );
    // With the vault whole again the same sell lands.
    w.set_tokens(&m.vault, honest);
    expect_ok(w.trade(&m, &a, E1, -1), "the 1-unit sell, solvent");
    let owed = w.assert_d6(&m);

    // finalize asserts D6 at the final liability (F + N + q_winner: E1 wins).
    expect_ok(w.propose(&m, E1), "propose");
    let honest = w.tokens(&m.vault);
    w.set_tokens(&m.vault, u64::try_from(owed).unwrap() - 1_000);
    w.set_time(LOCK + DAY + DISPUTE);
    let fin = w.finalize_ix(&m);
    refused_insolvent(&mut w, &[fin], &s, &watched, "finalize");
    w.set_tokens(&m.vault, honest);
    expect_ok(w.as_key(w.finalize_ix(&m), &s), "finalize, solvent");
    let owed = w.assert_d6(&m);

    // settle_ticket asserts D6 after paying: b (4 USDC of E1's YES, 3 of NONE's NO) and a's
    // winning YES are each refused, the ticket left open and nothing paid.
    w.set_tokens(&m.vault, u64::try_from(owed).unwrap() - 1_000);
    for (t, what) in [(&b, "settle b"), (&a, "settle a")] {
        let ix = w.settle_ix(&m, s.pubkey(), t.key.pubkey());
        refused_insolvent(&mut w, &[cu_limit(1_400_000), ix], &s, &watched, what);
    }
    w.set_tokens(&m.vault, honest);
    assert_eq!(w.settle(&m, b.key.pubkey()), 7 * USDC);
    assert_eq!(w.settle(&m, a.key.pubkey()), 10 * USDC - 1);
    w.assert_d6(&m);
}
