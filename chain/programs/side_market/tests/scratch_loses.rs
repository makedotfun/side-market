//! A scratched entry loses (D7) on the release build (LiteSVM under mainnet's feature set): its
//! YES shares are paid 0 and its NO shares 1 USDC each, fixed by side and share count alone, so
//! no trade by anyone, on the entry or another outcome, before or after, moves another wallet's
//! scratch payout. Here the outside report's proof of concept (a NO buy just before the scratch,
//! which under the one-value cash-out took 4.5 USDC from a YES holder) with its assertions turned
//! to the rule, and the two variants that broke the ordered unwind: a hedge with a sell from below
//! (A buys M YES, B M NO, the victim buys, A or B sells, then the scratch) and the cross-outcome
//! sandwich (NONE bought just before the scratch and sold after). Then a NO holder is paid 1 a
//! share whatever others traded; a scratch at q_k < 0 and one at q_k > 0 each hold D6 and leave the
//! liability no higher than before; a void after a scratch pays the scratched legs 0 and 1; and
//! trades on other outcomes after a scratch keep D6 through settlement.

mod common;
mod mainnet;
mod program_so;

use {
    anchor_lang::prelude::Pubkey,
    common::*,
    side_market::{MarketStatus, OpenMarketArgs, FIRST_ENTRY_ID, NONE_ID},
    solana_signer::Signer,
};

const E1: u16 = FIRST_ENTRY_ID;
const E2: u16 = FIRST_ENTRY_ID + 1;
const USDC: u64 = 1_000_000;

/// The victim's position in every variant: 25 USDC-shares.
const VICTIM: i64 = 25_000_000;

/// The report's market: the live parameters at fee 0.
fn args() -> OpenMarketArgs {
    OpenMarketArgs {
        seed: SEED,
        m_floor_ppm: M_FLOOR,
        none_mass_ppm: NONE_MASS,
        fee_rate_ppm: 0,
        lock_ts: LOCK,
        escrow: Pubkey::default(),
    }
}

/// The report's market: one entry at 5% (E1), and a second at 10% (E2) when `two`; the bounty
/// window open (buys until LOCK).
fn market(w: &mut World, n: u8, two: bool) -> Mkt {
    let m = w.open(n, Some(args()));
    w.add(&m, E1, 50_000);
    if two {
        w.add(&m, E2, 100_000);
    }
    w.open_bounty(&m);
    m
}

fn trader(w: &mut World, m: &Mkt, amount: u64) -> Trader {
    let t = w.trader(amount);
    w.open_ticket(m, t.key.pubkey());
    t
}

/// What a wallet holds in USDC: its funded account and its associated token account (where
/// settle_ticket pays).
fn total(w: &World, t: &Trader) -> i128 {
    let ata = anchor_spl::associated_token::get_associated_token_address(&t.key.pubkey(), &w.mint);
    i128::from(w.tokens(&t.token)) + i128::from(w.tokens(&ata))
}

/// Proposes `winner` at lock_ts and finalizes it after the dispute delay.
fn finalize(w: &mut World, m: &Mkt, winner: u16) {
    w.set_time(LOCK);
    expect_ok(w.propose(m, winner), "propose");
    w.set_time(LOCK + DISPUTE);
    expect_ok(w.as_operator(w.finalize_ix(m)), "finalize");
    assert_eq!(w.market(m).status, MarketStatus::Final);
}

/// Scratches E1, asserting D6 before and after and that the liability did not rise.
fn scratch_e1(w: &mut World, m: &Mkt) {
    let before = w.assert_d6(m);
    expect_ok(w.scratch(m, E1), "scratch E1");
    let after = w.assert_d6(m);
    assert!(
        after <= before,
        "the scratch raised the liability: {before} -> {after}"
    );
    let market = w.market(m);
    let o = market.outcomes[market.index_of(E1).unwrap()];
    assert_eq!((o.yes_value, o.no_value), (0, u64::MAX), "YES 0, NO 1");
}

/// What a victim holding `side` (+1 YES, -1 NO) on E1 is owed under the rule.
fn owed(side: i64) -> u64 {
    if side > 0 {
        0
    } else {
        VICTIM as u64
    }
}

#[test]
fn the_reports_front_run_moves_nothing_from_the_victim() {
    let mut base = World::new();
    let m = market(&mut base, 7, false);
    let victim = trader(&mut base, &m, 1_000 * USDC);
    let v_start = total(&base, &victim);
    expect_ok(base.trade(&m, &victim, E1, VICTIM), "victim YES");
    base.assert_d6(&m);
    let v_cost = v_start - total(&base, &victim);
    assert!(
        v_cost > 4 * USDC as i128,
        "the victim's 25 YES cost {v_cost}"
    );

    // Honest: the operator scratches.
    let mut honest = base.fork();
    scratch_e1(&mut honest, &m);
    finalize(&mut honest, &m, NONE_ID);
    let v_paid_honest = honest.settle(&m, victim.key.pubkey());
    honest.assert_d6(&m);
    let v_honest = total(&honest, &victim) - v_start;

    // Attack: a fresh wallet buys 70 USDC-shares of NO, then the operator scratches.
    let mut atk = base.fork();
    let attacker = trader(&mut atk, &m, 10_000 * USDC);
    let a_start = total(&atk, &attacker);
    expect_ok(atk.trade(&m, &attacker, E1, -70_000_000), "attacker NO");
    let a_cost = a_start - total(&atk, &attacker);
    atk.assert_d6(&m);
    scratch_e1(&mut atk, &m);
    finalize(&mut atk, &m, NONE_ID);
    let v_paid_atk = atk.settle(&m, victim.key.pubkey());
    let a_paid = atk.settle(&m, attacker.key.pubkey());
    atk.assert_d6(&m);
    let v_atk = total(&atk, &victim) - v_start;
    let a_net = total(&atk, &attacker) - a_start;

    println!(
        "victim net honest = {v_honest:+} base; attack = {v_atk:+} base; attacker = {a_net:+} \
         base (paid {a_paid} for 70 NO bought at {a_cost})"
    );
    // The victim's settlement is the same in both branches: 0 for its YES.
    assert_eq!(v_paid_honest, 0);
    assert_eq!(v_paid_atk, 0);
    assert_eq!(v_honest, v_atk, "the attack moved the victim's result");
    assert_eq!(v_honest, -v_cost);
    // The attacker's 70 NO are paid 70: what it gains it gains from the house (foreknowledge),
    // never from the victim.
    assert_eq!(a_paid, 70 * USDC);
    assert_eq!(a_net, 70 * USDC as i128 - a_cost);
}

/// The victim's E1 payout on a fresh market: `setup` (the other wallets' trades before the
/// victim's buy), the victim's 25 shares on `side` (+1 YES, -1 NO), `between` (trades after it),
/// the scratch, `after` (trades after the scratch), then NONE wins and every ticket settles, D6
/// asserted at each step, and the market closes.
fn victim_payout(
    n: u8,
    side: i64,
    setup: impl FnOnce(&mut World, &Mkt) -> Vec<Trader>,
    between: impl FnOnce(&mut World, &Mkt, &[Trader]),
    after: impl FnOnce(&mut World, &Mkt, &[Trader]),
) -> u64 {
    let mut w = World::new();
    let m = market(&mut w, n, false);
    let others = setup(&mut w, &m);
    let victim = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &victim, E1, side * VICTIM), "victim");
    w.assert_d6(&m);
    between(&mut w, &m, &others);
    w.assert_d6(&m);
    scratch_e1(&mut w, &m);
    after(&mut w, &m, &others);
    w.assert_d6(&m);
    finalize(&mut w, &m, NONE_ID);
    let paid = w.settle(&m, victim.key.pubkey());
    w.assert_d6(&m);
    for t in &others {
        w.settle(&m, t.key.pubkey());
        w.assert_d6(&m);
    }
    expect_ok(
        w.as_operator(w.close_ix(&m, w.k.operator.pubkey())),
        "close",
    );
    paid
}

#[test]
fn a_hedge_and_a_sell_from_below_move_nothing_from_the_victim() {
    let mut n = 10u8;
    for side in [1i64, -1] {
        let alone = victim_payout(n, side, |_, _| vec![], |_, _, _| {}, |_, _, _| {});
        n += 1;
        assert_eq!(alone, owed(side), "side {side} alone");
        for mm in [10i64, 30, 70] {
            let size = mm * USDC as i64;
            // A buys M YES and B buys M NO; after the victim's buy, A (then, in a second run, B)
            // sells its M back, just before the scratch.
            for seller in [0usize, 1] {
                let paid = victim_payout(
                    n,
                    side,
                    |w, m| {
                        let a = trader(w, m, 10_000 * USDC);
                        let b = trader(w, m, 10_000 * USDC);
                        expect_ok(w.trade(m, &a, E1, size), "A YES");
                        expect_ok(w.trade(m, &b, E1, -size), "B NO");
                        vec![a, b]
                    },
                    |w, m, ab| {
                        let back = if seller == 0 { -size } else { size };
                        expect_ok(w.trade(m, &ab[seller], E1, back), "the sell");
                    },
                    |_, _, _| {},
                );
                n += 1;
                assert_eq!(
                    paid, alone,
                    "side {side}, M {mm}, seller {seller}: the victim was paid {paid}, alone {alone}"
                );
            }
        }
    }
}

#[test]
fn a_none_sandwich_moves_nothing_from_the_victim() {
    let mut n = 40u8;
    for side in [1i64, -1] {
        let alone = victim_payout(n, side, |_, _| vec![], |_, _, _| {}, |_, _, _| {});
        n += 1;
        for x in [20i64, 100] {
            let size = x * USDC as i64;
            let paid = victim_payout(
                n,
                side,
                |w, m| vec![trader(w, m, 10_000 * USDC)],
                |w, m, a| {
                    expect_ok(w.trade(m, &a[0], NONE_ID, size), "buy NONE just before");
                },
                |w, m, a| {
                    expect_ok(w.trade(m, &a[0], NONE_ID, -size), "sell NONE just after");
                },
            );
            n += 1;
            assert_eq!(
                paid, alone,
                "side {side}, X {x}: the victim was paid {paid}, alone {alone}"
            );
            assert_eq!(paid, owed(side));
        }
    }
}

#[test]
fn a_no_holder_is_paid_one_a_share_whatever_others_traded() {
    // The victim's 25 NO on E1 against: nothing; large YES; large NO; a hedge sold from either
    // side; NONE and E2 trades before and after; all of them.
    let mut results = Vec::new();
    for (case, n) in (0u8..6).zip(60u8..) {
        let mut w = World::new();
        let m = market(&mut w, n, true);
        let a = trader(&mut w, &m, 10_000 * USDC);
        let b = trader(&mut w, &m, 10_000 * USDC);
        if case == 1 || case == 5 {
            expect_ok(w.trade(&m, &a, E1, 200 * USDC as i64), "A YES");
        }
        if case == 2 || case == 5 {
            expect_ok(w.trade(&m, &b, E1, -(200 * USDC as i64)), "B NO");
        }
        let victim = trader(&mut w, &m, 1_000 * USDC);
        expect_ok(w.trade(&m, &victim, E1, -VICTIM), "victim NO");
        if case == 3 {
            expect_ok(w.trade(&m, &a, E1, 50 * USDC as i64), "A YES");
            expect_ok(w.trade(&m, &b, E1, -(50 * USDC as i64)), "B NO");
            expect_ok(w.trade(&m, &a, E1, -(50 * USDC as i64)), "A sells");
        }
        if case == 4 || case == 5 {
            expect_ok(w.trade(&m, &a, NONE_ID, 80 * USDC as i64), "A NONE");
            expect_ok(w.trade(&m, &b, E2, -(60 * USDC as i64)), "B NO E2");
        }
        scratch_e1(&mut w, &m);
        if case == 4 || case == 5 {
            expect_ok(
                w.trade(&m, &a, NONE_ID, -(80 * USDC as i64)),
                "A sells NONE",
            );
            expect_ok(w.trade(&m, &b, E2, 30 * USDC as i64), "B sells NO E2");
            w.assert_d6(&m);
        }
        finalize(&mut w, &m, E2);
        let paid = w.settle(&m, victim.key.pubkey());
        w.assert_d6(&m);
        w.settle(&m, a.key.pubkey());
        w.settle(&m, b.key.pubkey());
        w.assert_d6(&m);
        assert_eq!(w.market(&m).scratch_owed, 0, "case {case}: F paid out");
        results.push(paid);
    }
    assert!(
        results.iter().all(|p| *p == VICTIM as u64),
        "the NO victim's payouts: {results:?}"
    );
}

#[test]
fn a_scratch_at_negative_and_positive_q_holds_d6_and_lowers_the_liability() {
    for (n, net_no) in [(70u8, 0i64), (71, 70)] {
        let mut w = World::new();
        let m = market(&mut w, n, true);
        let yes = trader(&mut w, &m, 10_000 * USDC);
        let no = trader(&mut w, &m, 10_000 * USDC);
        expect_ok(w.trade(&m, &yes, E1, VICTIM), "YES");
        if net_no > 0 {
            expect_ok(w.trade(&m, &no, E1, -net_no * USDC as i64), "NO");
        }
        // Another outcome carries the book's top q, so the scratch's own q is not the max.
        expect_ok(w.trade(&m, &yes, E2, 5 * USDC as i64), "YES E2");
        let market = w.market(&m);
        let k = market.index_of(E1).unwrap();
        let q = market.outcomes[k].q;
        assert_eq!(q > 0, net_no == 0, "q_k {q}");
        let no_k = market.outcomes[k].no_shares;
        let (f, total_no) = (market.scratch_owed, market.no_total);
        let before = w.assert_d6(&m);
        scratch_e1(&mut w, &m);
        let after_m = w.market(&m);
        assert_eq!(after_m.scratch_owed, f + no_k, "q_k {q}: F takes N_k");
        assert_eq!(after_m.no_total, total_no - no_k, "q_k {q}: N drops N_k");
        let after = w.assert_d6(&m);
        // The max-q term loses k: with q_k > 0 the liability falls by q_k's lead over E2's q.
        if q > 0 {
            assert!(after < before, "q_k {q}: {before} -> {after}");
        } else {
            assert_eq!(after, before, "q_k {q}: {before} -> {after}");
        }
        finalize(&mut w, &m, NONE_ID);
        assert_eq!(
            w.settle(&m, yes.key.pubkey()),
            0,
            "q_k {q}: YES on E1 and E2 lose"
        );
        assert_eq!(
            w.settle(&m, no.key.pubkey()),
            no_k,
            "q_k {q}: NO paid 1 a share"
        );
        w.assert_d6(&m);
    }
}

#[test]
fn a_void_after_a_scratch_pays_the_scratched_legs_nothing_and_one() {
    let mut w = World::new();
    let m = market(&mut w, 80, true);
    let yes = trader(&mut w, &m, 10_000 * USDC);
    let no = trader(&mut w, &m, 10_000 * USDC);
    expect_ok(w.trade(&m, &yes, E1, VICTIM), "YES E1");
    expect_ok(w.trade(&m, &no, E1, -(10 * USDC as i64)), "NO E1");
    expect_ok(w.trade(&m, &yes, E2, 7 * USDC as i64), "YES E2");
    expect_ok(w.trade(&m, &no, E2, -(3 * USDC as i64)), "NO E2");
    scratch_e1(&mut w, &m);
    let arb = w.k.arbiter.insecure_clone();
    expect_ok(w.as_key(w.void_ix(&m, arb.pubkey()), &arb), "void");
    let market = w.market(&m);
    assert_eq!(market.status, MarketStatus::Void);
    let e1 = market.outcomes[market.index_of(E1).unwrap()];
    assert_eq!(
        (e1.yes_value, e1.no_value),
        (0, u64::MAX),
        "void keeps the scratch's values"
    );
    assert_eq!(market.leg_payout(E1, VICTIM).unwrap(), 0);
    assert_eq!(
        market.leg_payout(E1, -(10 * USDC as i64)).unwrap(),
        10 * USDC
    );
    w.assert_d6(&m);
    // Each wallet: its E1 leg at 0 / 1, its E2 leg at the frozen mark.
    let want_yes = market.leg_payout(E2, 7 * USDC as i64).unwrap();
    let want_no = 10 * USDC + market.leg_payout(E2, -(3 * USDC as i64)).unwrap();
    assert_eq!(w.settle(&m, yes.key.pubkey()), want_yes);
    w.assert_d6(&m);
    assert_eq!(w.settle(&m, no.key.pubkey()), want_no);
    w.assert_d6(&m);
    assert_eq!(w.market(&m).scratch_owed, 0, "F paid out");
}

#[test]
fn trades_on_other_outcomes_after_a_scratch_keep_d6() {
    let mut w = World::new();
    let m = market(&mut w, 90, true);
    let ts: Vec<Trader> = (0..3).map(|_| trader(&mut w, &m, 10_000 * USDC)).collect();
    expect_ok(w.trade(&m, &ts[0], E1, 40 * USDC as i64), "YES E1");
    expect_ok(w.trade(&m, &ts[1], E1, -(90 * USDC as i64)), "NO E1");
    scratch_e1(&mut w, &m);
    let moves: [(usize, u16, i64); 10] = [
        (0, NONE_ID, 30),
        (1, E2, 80),
        (2, E2, -120),
        (0, NONE_ID, -15),
        (1, NONE_ID, -60),
        (2, E2, 70),
        (1, E2, -80),
        (0, E2, 200),
        (2, NONE_ID, 45),
        (1, NONE_ID, 30),
    ];
    for (i, id, d) in moves {
        expect_ok(
            w.trade(&m, &ts[i], id, d * USDC as i64),
            "a trade after the scratch",
        );
        w.assert_d6(&m);
    }
    let owed_f = w.market(&m).scratch_owed;
    assert_eq!(owed_f, 90 * USDC, "F still holds E1's NO");
    finalize(&mut w, &m, E2);
    w.assert_d6(&m);
    let mut paid = 0;
    for t in &ts {
        paid += w.settle(&m, t.key.pubkey());
        w.assert_d6(&m);
    }
    assert!(paid >= 90 * USDC);
    assert_eq!(w.market(&m).scratch_owed, 0, "F paid out");
    expect_ok(
        w.as_operator(w.close_ix(&m, w.k.operator.pubkey())),
        "close",
    );
}
