//! Property tests on the release build: random sequences of YES and NO buys and sales by several
//! wallets, joins, scratches (of hedged books too: a large gross YES and NO behind a small net q,
//! each scratch paying both sides from one value), halts and resumes, and warps of the clock
//! forward through a bounty window, the gap after it, a race window, the stretch before the
//! deadline and the stretch after it. D6 is checked after every step from the vault's real
//! balance against a liability the test recomputes from the market's raw fields (common::
//! liability_of), and no step is ever refused as Insolvent. The trading rule is checked on every
//! trade: a buy outside every window is refused TradingClosed, a sell on an Open market always
//! lands (inside a window or not), anything on a halted market is refused InvalidStatus.
//!
//! Every case opens with one buy per wallet inside the bounty window and closes, after its random
//! steps, with sells outside every window (past the deadline): a holder that no random step
//! touches sells half its NONE position, and every wallet sells half of each live leg. So every
//! case lands at least TRADERS + 1 buys and at least one sell outside every window (asserted), and
//! prints its counts of landed buys, sells, sells outside any window, joins and scratches.
//!
//! Then every ending is played on a fork of the same world: each live outcome winning, FIELD
//! winning, and a void at the marks. In every one every ticket settles, D6 holds after each
//! settlement, and the house's realised loss (its deposit less the surplus close_market returns)
//! is at most ceil(b L), its bound b ln(1/m_floor) (rounding always falls the house's way, so no
//! rounding allowance is needed).

mod common;
mod mainnet;
mod program_so;

use {
    common::*,
    proptest::prelude::*,
    side_market::{
        math, MarketStatus, OpenMarketArgs, SideMarketError, FIELD_ID, FIRST_ENTRY_ID, NONE_ID,
    },
    solana_signer::Signer,
    std::{
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    },
};

const USDC: u64 = 1_000_000;
/// The wallets the random steps trade with; one more, the holder, only opens and closes.
const TRADERS: usize = 4;

/// The property market's windows: the bounty window, a gap, the race window, then the run to the
/// deadline (LOCK = NOW + 10 days) with no window open.
const BOUNTY: [i64; 2] = [NOW, NOW + 3 * DAY];
const RACE: [i64; 2] = [NOW + 5 * DAY, NOW + 6 * DAY];
/// Where a Warp moves the clock (never back): inside the bounty window, the gap, inside the race
/// window, after the race before the deadline, after the deadline. All before void_after.
const STOPS: [i64; 5] = [
    NOW + DAY,
    NOW + 4 * DAY,
    NOW + 5 * DAY + DAY / 2,
    NOW + 8 * DAY,
    LOCK + DAY,
];

fn in_window(t: i64) -> bool {
    (BOUNTY[0] <= t && t < BOUNTY[1]) || (RACE[0] <= t && t < RACE[1])
}

#[derive(Clone, Debug)]
enum Op {
    /// A trade by trader `who` on the `pick`-th tradeable outcome: a buy of YES or NO, or a sale
    /// of what it holds, sized `frac` / 65535 of 3 b (at least 1 base unit).
    Trade {
        who: u8,
        pick: u8,
        yes: bool,
        buy: bool,
        frac: u16,
    },
    Join {
        mass: u32,
    },
    Scratch {
        pick: u8,
    },
    /// A hedged book, then its scratch (the review's F1): on the `pick`-th entry one trader buys
    /// YES sized `frac` / 65535 of 2 b, another buys NO down to a net q of `net`, and the entry
    /// is scratched. Both sides are paid from one value v.
    HedgedScratch {
        pick: u8,
        frac: u16,
        net: i16,
    },
    Halt {
        guardian: bool,
    },
    Resume,
    /// The clock moves forward to STOPS[stop % 5] (when that is later than now).
    Warp {
        stop: u8,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0u8..TRADERS as u8, any::<u8>(), any::<bool>(), prop::bool::weighted(0.7), any::<u16>())
            .prop_map(|(who, pick, yes, buy, frac)| Op::Trade { who, pick, yes, buy, frac }),
        1 => (5_000u32..80_000).prop_map(|mass| Op::Join { mass }),
        1 => any::<u8>().prop_map(|pick| Op::Scratch { pick }),
        1 => (
            any::<u8>(),
            any::<u16>(),
            prop_oneof![Just(0i16), -3i16..=3, -1_000i16..1_000, any::<i16>()],
        )
            .prop_map(|(pick, frac, net)| Op::HedgedScratch { pick, frac, net }),
        1 => any::<bool>().prop_map(|guardian| Op::Halt { guardian }),
        1 => Just(Op::Resume),
        1 => any::<u8>().prop_map(|stop| Op::Warp { stop }),
    ]
}

#[derive(Clone, Debug)]
struct Case {
    seed_usdc: u64,
    m_floor: u32,
    fee: u32,
    joins: Vec<u32>,
    ops: Vec<Op>,
}

fn case() -> impl Strategy<Value = Case> {
    (
        1u64..200,
        prop::sample::select(vec![500u32, 5_000, 20_000]),
        prop::sample::select(vec![0u32, 10_000, 70_000]),
        prop::collection::vec(20_000u32..150_000, 1..5),
        prop::collection::vec(op(), 5..40),
    )
        .prop_map(|(seed_usdc, m_floor, fee, joins, ops)| Case {
            seed_usdc,
            m_floor,
            fee,
            joins,
            ops,
        })
}

/// What landed in one case.
#[derive(Default, Debug)]
struct Counts {
    buys: usize,
    sells: usize,
    /// Sells that landed with no window open.
    sells_outside: usize,
    joins: usize,
    scratches: usize,
}

impl Counts {
    /// Counts a trade of `delta` on a position `p` that landed at `t`.
    fn landed(&mut self, p: i64, delta: i64, t: i64) {
        if is_buy(p, delta) {
            self.buys += 1;
        } else {
            self.sells += 1;
            if !in_window(t) {
                self.sells_outside += 1;
            }
        }
    }
}

/// A trade of `delta` on a position `p` is a buy unless it moves the position toward zero.
fn is_buy(p: i64, delta: i64) -> bool {
    !((p > 0 && delta < 0) || (p < 0 && delta > 0))
}

fn tradeable(w: &World, m: &Mkt) -> Vec<u16> {
    w.market(m)
        .outcomes
        .iter()
        .filter(|o| o.is_live() && o.id != FIELD_ID)
        .map(|o| o.id)
        .collect()
}

fn position(w: &World, m: &Mkt, t: &Trader, id: u16) -> i64 {
    w.ticket(m, &t.key.pubkey())
        .legs
        .iter()
        .find(|l| l.outcome_id == id)
        .map_or(0, |l| l.shares)
}

#[track_caller]
fn never_insolvent(sent: &Sent, what: &str) {
    assert_ne!(
        sent.code,
        Some(u32::from(SideMarketError::Insolvent)),
        "{what} was refused as Insolvent\n{}",
        sent.logs
    );
}

/// A scratched outcome's values come from one v: yes + no = 2^64 (less the u64 cap's one ulp at
/// v = 0 or 1).
#[track_caller]
fn one_value(w: &World, m: &Mkt, id: u16, what: &str) {
    let market = w.market(m);
    let o = market.outcomes[market.index_of(id).unwrap()];
    let sum = u128::from(o.yes_value) + u128::from(o.no_value);
    assert!(
        sum <= 1 << 64 && sum >= (1 << 64) - 1,
        "{what}: yes {} + no {}",
        o.yes_value,
        o.no_value
    );
}

/// Scratches `id` (refusals allowed, never Insolvent); a scratch that lands pays from one value.
fn scratch_checked(w: &mut World, m: &Mkt, id: u16, what: &str, counts: &mut Counts) {
    let sent = w.scratch(m, id);
    never_insolvent(&sent, what);
    assert!(sent.ok || sent.code.is_some(), "{what}: {}", sent.error);
    if sent.ok {
        counts.scratches += 1;
        one_value(w, m, id, what);
    }
}

/// A trade whose timing rule is checked: on an Open market a sell always lands and a buy outside
/// every window is refused TradingClosed; on a Halted one anything is refused InvalidStatus.
fn trade_checked(
    w: &mut World,
    m: &Mkt,
    t: &Trader,
    id: u16,
    delta: i64,
    what: &str,
    counts: &mut Counts,
) -> bool {
    let p = position(w, m, t, id);
    let at = w.time();
    let status = w.market(m).status;
    let sent = w.trade(m, t, id, delta);
    never_insolvent(&sent, what);
    let buy = is_buy(p, delta);
    match status {
        MarketStatus::Open if !buy => {
            assert!(
                sent.ok,
                "{what}: a sell on an Open market at {at} (window open: {}) was refused: {}\n{}",
                in_window(at),
                sent.error,
                sent.logs
            );
        }
        MarketStatus::Open if !in_window(at) => {
            expect_code(&sent, SideMarketError::TradingClosed, what);
        }
        MarketStatus::Open => {}
        _ => expect_code(&sent, SideMarketError::InvalidStatus, what),
    }
    if sent.ok {
        counts.landed(p, delta, at);
    }
    sent.ok
}

/// Plays one ending on a fork: `winner` Some(id) proposes and finalizes it, None voids at the
/// marks. Every ticket settles, then the market closes; returns the house's realised loss.
fn ending(w: &World, m: &Mkt, traders: &[Trader], winner: Option<u16>) -> (u64, u64) {
    let mut f = w.fork();
    let market = f.market(m);
    match winner {
        Some(id) => {
            let at = f
                .time()
                .max(LOCK)
                .max(market.bounty_until)
                .max(market.race_until);
            f.set_time(at);
            // propose waits out a guardian's halt: the admin lifts it first.
            if market.status == MarketStatus::Halted && market.halted_by_guardian {
                let admin = f.k.admin.insecure_clone();
                expect_ok(f.as_key(f.resume_ix(m, admin.pubkey()), &admin), "resume");
            }
            expect_ok(f.propose(m, id), "propose");
            let t = f.time();
            f.set_time(t + DISPUTE);
            expect_ok(f.as_operator(f.finalize_ix(m)), "finalize");
        }
        None => {
            let arb = f.k.arbiter.insecure_clone();
            expect_ok(f.as_key(f.void_ix(m, arb.pubkey()), &arb), "void");
        }
    }
    f.assert_d6(m);
    for t in traders {
        f.settle(m, t.key.pubkey());
        f.assert_d6(m);
    }
    let surplus = f.tokens(&m.vault);
    expect_ok(f.as_operator(f.close_ix(m, f.k.operator.pubkey())), "close");
    let loss = market.deposit.saturating_sub(surplus);
    let bound = math::max_loss(market.b, math::loss_per_b(market.m_floor_ppm).unwrap()).unwrap();
    (loss, bound)
}

static CASE: AtomicUsize = AtomicUsize::new(0);

fn run(c: &Case) {
    let mut w = World::new();
    let mut counts = Counts::default();
    let args = OpenMarketArgs {
        seed: c.seed_usdc * USDC,
        m_floor_ppm: c.m_floor,
        fee_rate_ppm: c.fee,
        ..w.args()
    };
    let m = w.open(1, Some(args));
    let mut next = FIRST_ENTRY_ID;
    for &mass in &c.joins {
        let ix = w.add_outcome_ix(&m, w.k.operator.pubkey(), next, mass);
        if w.as_operator(ix).ok {
            next += 1;
            counts.joins += 1;
        }
    }
    w.set_windows(&m, [BOUNTY[0], BOUNTY[1], RACE[0], RACE[1]]);
    // traders[..TRADERS] take the random steps; traders[TRADERS], the holder, only opens and
    // closes.
    let traders: Vec<Trader> = (0..=TRADERS)
        .map(|_| w.trader(1_000_000_000 * USDC))
        .collect();
    for t in &traders {
        w.open_ticket(&m, t.key.pubkey());
    }
    let b = w.market(&m).b;
    let opening = (b / 4).max(1) as i64;

    // The opening: one buy per wallet inside the bounty window, the holder's on NONE.
    for (i, t) in traders.iter().enumerate() {
        let ids = tradeable(&w, &m);
        let id = if i == TRADERS {
            NONE_ID
        } else {
            ids[i % ids.len()]
        };
        let what = format!("opening buy {i}");
        assert!(
            trade_checked(&mut w, &m, t, id, opening, &what, &mut counts),
            "{what} was refused"
        );
        w.assert_d6(&m);
    }

    let steps = &traders[..TRADERS];
    for (k, o) in c.ops.iter().enumerate() {
        let what = format!("op {k} {o:?}");
        match o {
            Op::Trade {
                who,
                pick,
                yes,
                buy,
                frac,
            } => {
                let ids = tradeable(&w, &m);
                let id = ids[usize::from(*pick) % ids.len()];
                let t = &steps[usize::from(*who)];
                let p = position(&w, &m, t, id);
                let size = (1 + u128::from(b) * 3 * u128::from(*frac) / 65_535) as i64;
                let delta = if *buy && (p == 0 || (p > 0) == *yes) {
                    if *yes {
                        size
                    } else {
                        -size
                    }
                } else if p != 0 {
                    -p.signum() * size.min(p.abs())
                } else {
                    continue;
                };
                trade_checked(&mut w, &m, t, id, delta, &what, &mut counts);
            }
            Op::Join { mass } => {
                let ix = w.add_outcome_ix(&m, w.k.operator.pubkey(), next, *mass);
                if w.as_operator(ix).ok {
                    next += 1;
                    counts.joins += 1;
                }
            }
            Op::Scratch { pick } => {
                let entries: Vec<u16> = tradeable(&w, &m)
                    .into_iter()
                    .filter(|id| *id >= FIRST_ENTRY_ID)
                    .collect();
                if let Some(id) = entries.get(usize::from(*pick) % entries.len().max(1)) {
                    scratch_checked(&mut w, &m, *id, &what, &mut counts);
                }
            }
            Op::HedgedScratch { pick, frac, net } => {
                let entries: Vec<u16> = tradeable(&w, &m)
                    .into_iter()
                    .filter(|id| *id >= FIRST_ENTRY_ID)
                    .collect();
                let Some(&id) = entries.get(usize::from(*pick) % entries.len().max(1)) else {
                    continue;
                };
                // A YES buyer and a different NO buyer, each flat or already on its side.
                let yes_by = steps.iter().position(|t| position(&w, &m, t, id) >= 0);
                let no_by = steps
                    .iter()
                    .enumerate()
                    .position(|(i, t)| Some(i) != yes_by && position(&w, &m, t, id) <= 0);
                if let (Some(y), Some(n)) = (yes_by, no_by) {
                    let size = (1 + u128::from(b) * 2 * u128::from(*frac) / 65_535) as i64;
                    trade_checked(&mut w, &m, &steps[y], id, size, &what, &mut counts);
                    let market = w.market(&m);
                    let q = market.outcomes[market.index_of(id).unwrap()].q;
                    let no = q - i64::from(*net);
                    if no > 0 {
                        trade_checked(&mut w, &m, &steps[n], id, -no, &what, &mut counts);
                    }
                }
                w.assert_d6(&m);
                scratch_checked(&mut w, &m, id, &what, &mut counts);
            }
            Op::Halt { guardian } => {
                let key = if *guardian {
                    w.k.guardian.insecure_clone()
                } else {
                    w.k.operator.insecure_clone()
                };
                let _ = w.as_key(w.halt_ix(&m, key.pubkey()), &key);
            }
            Op::Resume => {
                let admin = w.k.admin.insecure_clone();
                let _ = w.as_key(w.resume_ix(&m, admin.pubkey()), &admin);
            }
            Op::Warp { stop } => {
                let to = STOPS[usize::from(*stop) % STOPS.len()];
                if to > w.time() {
                    w.set_time(to);
                }
            }
        }
        w.assert_d6(&m);
    }

    // The closing: past the deadline, outside every window, before void_after. A halted market
    // is resumed for it and halted again by the same key after, so the endings still start from
    // a halt when the random steps left one.
    let market = w.market(&m);
    let halted_by = (market.status == MarketStatus::Halted).then_some(market.halted_by_guardian);
    if halted_by.is_some() {
        let admin = w.k.admin.insecure_clone();
        expect_ok(w.as_key(w.resume_ix(&m, admin.pubkey()), &admin), "resume");
    }
    let at = w.time().max(LOCK + DAY);
    w.set_time(at);
    assert!(!in_window(at) && at < w.market(&m).void_after);
    for (i, t) in traders.iter().enumerate() {
        for id in tradeable(&w, &m) {
            let p = position(&w, &m, t, id);
            if p == 0 {
                continue;
            }
            let delta = -p.signum() * (p.abs() / 2).max(1);
            let what = format!("closing sell by {i} on {id}");
            assert!(
                trade_checked(&mut w, &m, t, id, delta, &what, &mut counts),
                "{what} was refused"
            );
            w.assert_d6(&m);
        }
    }
    if let Some(guardian) = halted_by {
        let key = if guardian {
            w.k.guardian.insecure_clone()
        } else {
            w.k.operator.insecure_clone()
        };
        expect_ok(w.as_key(w.halt_ix(&m, key.pubkey()), &key), "re-halt");
    }

    // The floor: the opening's TRADERS + 1 buys always land, and the holder's NONE position (no
    // random step touches it, and NONE is never scratched) is always there to sell at the
    // closing, outside every window.
    assert!(
        counts.buys > TRADERS && counts.sells_outside >= 1,
        "the floor: {counts:?}"
    );
    let n = CASE.fetch_add(1, Ordering::Relaxed);
    let _ = writeln!(
        std::io::stderr(),
        "side_market property case {n}: {} ops, landed buys {}, sells {} (outside any window {}), joins {}, scratches {}",
        c.ops.len(),
        counts.buys,
        counts.sells,
        counts.sells_outside,
        counts.joins,
        counts.scratches
    );

    // Every world: each live outcome, FIELD, and a void.
    let mut worlds: Vec<Option<u16>> = tradeable(&w, &m).into_iter().map(Some).collect();
    worlds.push(Some(FIELD_ID));
    worlds.push(None);
    assert!(worlds.contains(&Some(NONE_ID)));
    for winner in worlds {
        let (loss, bound) = ending(&w, &m, &traders, winner);
        assert!(
            loss <= bound,
            "world {winner:?}: the house lost {loss} > {bound}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn d6_holds_and_the_house_loss_is_bounded_in_every_world(c in case()) {
        run(&c);
    }
}

/// One adversarial sequence: buy every tradeable outcome up hard, then everything back; the
/// house's loss in each world stays within its bound.
#[test]
fn pushing_every_outcome_to_certainty() {
    let mut w = World::new();
    let m = w.open(2, None);
    for k in 0..5 {
        w.add(&m, FIRST_ENTRY_ID + k, 100_000);
    }
    w.open_bounty(&m);
    let traders: Vec<Trader> = (0..3).map(|_| w.trader(1_000_000_000 * USDC)).collect();
    for t in &traders {
        w.open_ticket(&m, t.key.pubkey());
    }
    let b = w.market(&m).b as i64;
    for (i, id) in tradeable(&w, &m).into_iter().enumerate() {
        let t = &traders[i % 3];
        expect_ok(w.trade(&m, t, id, 12 * b), "push YES");
        w.assert_d6(&m);
        let u = &traders[(i + 1) % 3];
        expect_ok(w.trade(&m, u, id, -(4 * b)), "lean NO");
        w.assert_d6(&m);
    }
    let mut worlds: Vec<Option<u16>> = tradeable(&w, &m).into_iter().map(Some).collect();
    worlds.push(Some(FIELD_ID));
    worlds.push(None);
    for winner in worlds {
        let (loss, bound) = ending(&w, &m, &traders, winner);
        assert!(
            loss <= bound,
            "world {winner:?}: the house lost {loss} > {bound}"
        );
    }
}
