//! A scratch of an entry with more NO than YES (q_k < 0) on the release build (LiteSVM under
//! mainnet's feature set): FIELD takes k's current weight, not its base mass, so the book's cost
//! and every other price stay where they were, and the house's guarantee is not spent paying k's
//! NO holders 1 a share. Before this rule the merge raised C by |R| and D6 later refused buys on a
//! low-mass outcome: on the report's market (seed 50 USDC, m_floor 0.5%, NONE 5%, an entry K at 5%
//! and J at the floor) one such scratch capped J near p 0.95-0.97, three near 0.85, and a 60% K
//! near 0.40, and a holder could not sell all its YES on NONE.
//!
//! Here, after one, two and three such scratches (N = 10, 50 and 1,000 NO on each K) and after a
//! 60% K's, a buyer drives J and NONE to p >= 0.999 with every buy landing; a holder sells all its
//! YES on NONE and another wallet buys NO on NONE; the other prices do not move at the scratch
//! (to the rounding bound) and FIELD's mass grows by math::scratch_mass_ppm; add_outcome after such
//! a scratch lists a new entry down to FIELD's floor. A scratch at q_k >= 0 still merges k's base
//! mass, its whole mass_ppm, and raises the other prices.

mod common;
mod mainnet;
mod program_so;

use {
    anchor_lang::prelude::Pubkey,
    common::*,
    side_market::{
        math, Market, MarketStatus, OpenMarketArgs, SideMarketError, FIRST_ENTRY_ID, NONE_ID,
    },
    solana_signer::Signer,
};

const USDC: u64 = 1_000_000;
/// The report's entry K: 5%.
const K_MASS: u32 = 50_000;
/// The report's floor-mass entry J.
const J_MASS: u32 = M_FLOOR;
/// How far a price (Q0.64) may move at a q_k < 0 scratch: the merge's rounding (weight_err on
/// two weights, LN_ERR, the new weight's own error and the cached weights') is a few parts in
/// 2^56 of FIELD's weight, so a price moves by at most a few times 2^8 ulps; 2^14 (2^-50) bounds
/// it with room.
const PRICE_ULPS: u128 = 1 << 14;

/// The report's market at fee 0, the bounty window open (buys until LOCK).
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

/// Opens the market with the entries `ks` (ids FIRST_ENTRY_ID..) at their masses, then J at the
/// floor. Returns the market, the K ids and J's id.
fn market(w: &mut World, n: u8, ks: &[u32]) -> (Mkt, Vec<u16>, u16) {
    let m = w.open(n, Some(args()));
    let mut id = FIRST_ENTRY_ID;
    let mut ids = Vec::new();
    for &mass in ks {
        w.add(&m, id, mass);
        ids.push(id);
        id += 1;
    }
    w.add(&m, id, J_MASS);
    w.open_bounty(&m);
    (m, ids, id)
}

fn trader(w: &mut World, m: &Mkt, amount: u64) -> Trader {
    let t = w.trader(amount);
    w.open_ticket(m, t.key.pubkey());
    t
}

fn outcome(market: &Market, id: u16) -> side_market::Outcome {
    market.outcomes[market.index_of(id).unwrap()]
}

/// Every live outcome's price, Q0.64 (w / sum), by id.
fn prices(market: &Market) -> Vec<(u16, u128)> {
    market
        .outcomes
        .iter()
        .filter(|o| o.is_live())
        .map(|o| {
            (
                o.id,
                math::frac_q64(o.w.min(market.sum), market.sum).unwrap().0,
            )
        })
        .collect()
}

/// A price in ppm.
fn ppm(w: &World, m: &Mkt, id: u16) -> u32 {
    let market = w.market(m);
    math::price_ppm(outcome(&market, id).w, market.sum).unwrap()
}

/// `no` buys `n` USDC-shares of NO on `k`, then the operator scratches it. Asserts the scratch at
/// q_k < 0: YES 0 and NO 1, F takes N_k, the liability does not rise, no other price moves past
/// PRICE_ULPS, the book's sum barely moves, and FIELD's mass grows by scratch_mass_ppm (less than
/// k's whole mass). Returns what FIELD's mass grew by.
fn short_scratch(w: &mut World, m: &Mkt, k: u16, no: &Trader, n: u64) -> u32 {
    expect_ok(w.trade(m, no, k, -((n * USDC) as i64)), "NO on K");
    let before = w.market(m);
    let o = outcome(&before, k);
    assert!(o.q < 0, "q_k {}", o.q);
    let field = before.field_index().unwrap();
    let gain = math::scratch_mass_ppm(o.mass_ppm, o.q, before.b).unwrap();
    assert!(gain < o.mass_ppm, "q_k {}: FIELD takes {gain}", o.q);
    let liability = w.assert_d6(m);
    let p0 = prices(&before);

    expect_ok(w.scratch(m, k), "scratch K");
    let after = w.market(m);
    let liability_after = w.assert_d6(m);
    assert!(liability_after <= liability, "the liability rose");
    let s = outcome(&after, k);
    assert_eq!((s.yes_value, s.no_value), (0, u64::MAX), "YES 0, NO 1");
    assert_eq!(after.scratch_owed, before.scratch_owed + o.no_shares);
    assert_eq!(
        after.outcomes[field].mass_ppm,
        before.outcomes[field].mass_ppm + gain,
        "FIELD's mass"
    );
    let live_mass: u64 = after
        .outcomes
        .iter()
        .filter(|o| o.is_live())
        .map(|o| u64::from(o.mass_ppm))
        .sum();
    assert!(live_mass <= 1_000_000, "the live masses sum to {live_mass}");
    // The sum and every other price (FIELD's included: it holds k's weight now) stay put.
    let ds = after.sum.abs_diff(before.sum);
    assert!(
        ds <= before.sum >> 50,
        "the sum moved {ds} of {}",
        before.sum
    );
    let p1 = prices(&after);
    for (id, p) in &p0 {
        if *id == k {
            continue;
        }
        let q = p1.iter().find(|(i, _)| i == id).expect("still live").1;
        if after.outcomes[field].id == *id {
            // FIELD's price is now its own and k's.
            let pk = p0.iter().find(|(i, _)| *i == k).unwrap().1;
            assert!(q.abs_diff(p + pk) <= PRICE_ULPS, "FIELD {p} + {pk} -> {q}");
        } else {
            assert!(q.abs_diff(*p) <= PRICE_ULPS, "outcome {id}: {p} -> {q}");
        }
    }
    gain
}

/// `buyer` buys YES on `id` in `step`-share buys until its price reaches 999,000 ppm: every buy
/// must land (none refused Insolvent), D6 checked after each. Returns how many buys it took.
fn drive(w: &mut World, m: &Mkt, buyer: &Trader, id: u16, step: u64, what: &str) -> usize {
    let mut n = 0;
    while ppm(w, m, id) < 999_000 {
        let sent = w.trade(m, buyer, id, (step * USDC) as i64);
        assert_ne!(
            sent.code,
            Some(u32::from(SideMarketError::Insolvent)),
            "{what}: buy {n} on {id} at {} ppm refused Insolvent",
            ppm(w, m, id)
        );
        expect_ok(sent, what);
        w.assert_d6(m);
        n += 1;
        assert!(n < 400, "{what}: {id} still under 0.999 after {n} buys");
    }
    n
}

/// Proposes `winner` at lock_ts, finalizes it, and settles every wallet in `ts`, D6 after each.
fn settle_all(w: &mut World, m: &Mkt, winner: u16, ts: &[&Trader]) {
    w.set_time(LOCK);
    expect_ok(w.propose(m, winner), "propose");
    w.set_time(LOCK + DISPUTE);
    expect_ok(w.as_operator(w.finalize_ix(m)), "finalize");
    assert_eq!(w.market(m).status, MarketStatus::Final);
    for t in ts {
        w.settle(m, t.key.pubkey());
        w.assert_d6(m);
    }
    assert_eq!(w.market(m).scratch_owed, 0, "F paid out");
}

#[test]
fn a_short_scratch_moves_no_other_price_and_gives_field_what_k_held() {
    for (n, no_shares) in [(10u8, 10u64), (11, 50), (12, 1_000)] {
        let mut w = World::new();
        let (m, ks, _) = market(&mut w, n, &[K_MASS]);
        let no = trader(&mut w, &m, 10_000 * USDC);
        let gain = short_scratch(&mut w, &m, ks[0], &no, no_shares);
        // 50,000 e^(-N / b), b = 9.436958 USDC: 17,328.6 at N = 10, 249.99996 at 50, 0 at 1,000.
        let b = w.market(&m).b;
        let want = (f64::from(K_MASS) * (-((no_shares * USDC) as f64) / b as f64).exp()).floor();
        assert!(
            (f64::from(gain) - want).abs() <= 1.0,
            "N {no_shares}: FIELD took {gain}, about {want}"
        );
    }
}

#[test]
fn after_short_scratches_a_buyer_drives_the_floor_entry_and_none_to_certainty() {
    // (entries scratched, their mass, N NO on each)
    let cases: [(usize, u32, u64); 8] = [
        (1, K_MASS, 10),
        (1, K_MASS, 50),
        (1, K_MASS, 1_000),
        (2, K_MASS, 50),
        (3, K_MASS, 10),
        (3, K_MASS, 50),
        (3, K_MASS, 1_000),
        (1, 600_000, 50),
    ];
    for (c, &(count, mass, no_shares)) in cases.iter().enumerate() {
        let n = 20 + c as u8;
        let mut w = World::new();
        let (m, ks, j) = market(&mut w, n, &vec![mass; count]);
        let no = trader(&mut w, &m, 100_000 * USDC);
        for &k in &ks {
            short_scratch(&mut w, &m, k, &no, no_shares);
        }
        let what = format!("{count} x {mass} ppm, N {no_shares}");
        for target in [j, NONE_ID] {
            let mut f = w.fork();
            let buyer = trader(&mut f, &m, 100_000 * USDC);
            let buys = drive(&mut f, &m, &buyer, target, 5, &what);
            assert!(buys > 0, "{what}: {target} needed no buy");
            settle_all(&mut f, &m, target, &[&buyer, &no]);
        }
    }
}

#[test]
fn after_a_short_scratch_a_holder_sells_all_its_none_yes_and_no_buys_on_none_land() {
    let mut w = World::new();
    let (m, ks, j) = market(&mut w, 40, &[K_MASS]);
    let no = trader(&mut w, &m, 10_000 * USDC);
    short_scratch(&mut w, &m, ks[0], &no, 50);
    // h holds 90 YES on J and 90 YES on NONE, bought in alternating 1-share buys.
    let h = trader(&mut w, &m, 10_000 * USDC);
    for _ in 0..90 {
        expect_ok(w.trade(&m, &h, j, USDC as i64), "h YES on J");
        expect_ok(w.trade(&m, &h, NONE_ID, USDC as i64), "h YES on NONE");
    }
    w.assert_d6(&m);

    // h sells all 90 NONE YES, one share a sale.
    let mut sells = w.fork();
    for s in 0..90 {
        let sent = sells.trade(&m, &h, NONE_ID, -(USDC as i64));
        assert_ne!(
            sent.code,
            Some(u32::from(SideMarketError::Insolvent)),
            "sale {s} refused Insolvent"
        );
        expect_ok(sent, "h sells NONE");
        sells.assert_d6(&m);
    }
    let t = sells.ticket(&m, &h.key.pubkey());
    assert!(
        t.legs
            .iter()
            .all(|l| l.outcome_id != NONE_ID || l.shares == 0),
        "h still holds NONE"
    );
    settle_all(&mut sells, &m, j, &[&h, &no]);

    // Another wallet buys 120 NO on NONE, one share a buy.
    let mut nos = w.fork();
    let x = trader(&mut nos, &m, 10_000 * USDC);
    for s in 0..120 {
        let sent = nos.trade(&m, &x, NONE_ID, -(USDC as i64));
        assert_ne!(
            sent.code,
            Some(u32::from(SideMarketError::Insolvent)),
            "NO buy {s} refused Insolvent"
        );
        expect_ok(sent, "x NO on NONE");
        nos.assert_d6(&m);
    }
    settle_all(&mut nos, &m, j, &[&h, &no, &x]);
}

#[test]
fn add_outcome_after_a_short_scratch_lists_an_entry_down_to_fields_floor() {
    for (n, no_shares) in [(50u8, 10u64), (51, 1_000)] {
        let mut w = World::new();
        let (m, ks, j) = market(&mut w, n, &[K_MASS]);
        let no = trader(&mut w, &m, 10_000 * USDC);
        short_scratch(&mut w, &m, ks[0], &no, no_shares);
        let market = w.market(&m);
        let field = market.field_index().unwrap();
        let field_mass = market.outcomes[field].mass_ppm;
        // FIELD's real weight is never below what its mass_ppm claims, past the rounding
        // theta_of_ppm itself takes (2^-50 relative here).
        let real = math::exp_q64(market.outcomes[field].theta).unwrap();
        let claim = (u128::from(field_mass) << 64) / 1_000_000;
        assert!(
            real + (claim >> 50) >= claim,
            "N {no_shares}: FIELD weighs {real}, its mass claims {claim}"
        );

        // A new entry takes FIELD down to exactly m_floor; then nothing more fits.
        let id = j + 1;
        w.add(&m, id, field_mass - M_FLOOR);
        let market = w.market(&m);
        assert_eq!(market.outcomes[field].mass_ppm, M_FLOOR);
        let ix = w.add_outcome_ix(&m, w.k.operator.pubkey(), id + 1, M_FLOOR);
        expect_code(
            &w.as_operator(ix),
            SideMarketError::MassBelowFloor,
            "past FIELD's floor",
        );
        // FIELD still weighs at least about m_floor, and the new entry trades.
        let real = math::exp_q64(market.outcomes[field].theta).unwrap();
        let floor = (u128::from(M_FLOOR) << 64) / 1_000_000;
        assert!(
            real + (floor >> 40) >= floor,
            "N {no_shares}: FIELD {real} < {floor}"
        );
        let pf = ppm(&w, &m, side_market::FIELD_ID);
        assert!(pf >= M_FLOOR - 10, "N {no_shares}: FIELD at {pf} ppm");
        let buyer = trader(&mut w, &m, 100_000 * USDC);
        drive(&mut w, &m, &buyer, j, 5, "J after the join");
        settle_all(&mut w, &m, j, &[&buyer, &no]);
    }
}

#[test]
fn a_scratch_at_q_zero_or_above_merges_the_base_mass_as_before() {
    for (n, yes, no_shares) in [(60u8, 20u64, 10u64), (61, 10, 10), (62, 0, 0)] {
        let mut w = World::new();
        let (m, ks, j) = market(&mut w, n, &[K_MASS]);
        let k = ks[0];
        let a = trader(&mut w, &m, 10_000 * USDC);
        let b = trader(&mut w, &m, 10_000 * USDC);
        if yes > 0 {
            expect_ok(w.trade(&m, &a, k, (yes * USDC) as i64), "YES on K");
        }
        if no_shares > 0 {
            expect_ok(w.trade(&m, &b, k, -((no_shares * USDC) as i64)), "NO on K");
        }
        let before = w.market(&m);
        let field = before.field_index().unwrap();
        let o = outcome(&before, k);
        assert!(o.q >= 0);
        // The rule before this change: ln(lo(e^theta_F) + lo(e^theta_k)) - LN_ERR, the whole mass.
        let lo = |t: i128| {
            let e = math::exp_q64(t).unwrap();
            e - math::weight_err(e)
        };
        let want =
            math::ln_q64(lo(before.outcomes[field].theta) + lo(o.theta)).unwrap() - math::LN_ERR;
        let (pn, pj) = (ppm(&w, &m, NONE_ID), ppm(&w, &m, j));
        let liability = w.assert_d6(&m);
        expect_ok(w.scratch(&m, k), "scratch K");
        let after = w.market(&m);
        assert!(w.assert_d6(&m) <= liability);
        assert_eq!(
            after.outcomes[field].theta, want,
            "q_k {}: FIELD's theta",
            o.q
        );
        assert_eq!(
            after.outcomes[field].mass_ppm,
            before.outcomes[field].mass_ppm + K_MASS,
            "q_k {}: FIELD's mass",
            o.q
        );
        // C fell by R >= 0: the other prices rose (or held, at q_k = 0).
        assert!(ppm(&w, &m, NONE_ID) >= pn && ppm(&w, &m, j) >= pj);
        if o.q > 0 {
            assert!(ppm(&w, &m, NONE_ID) > pn, "q_k {}: NONE did not rise", o.q);
        }
        settle_all(&mut w, &m, NONE_ID, &[&a, &b]);
    }
}
