//! Compute units of a trade at 8, 16 and 66 outcomes (6, 14 and 64 live entries, NONE and FIELD),
//! measured on the release build and printed to stderr past libtest's capture, with ceilings
//! asserted: a trade that reweighs its own outcome, its first trade on a new leg (the ticket's
//! realloc), a sale (inside a window and outside every window), and the worst case, a trade that rebases the whole book. Also the largest
//! book the program holds (MAX_OUTCOMES, scratched slots included) with a ticket holding a leg on
//! every tradeable outcome, which must stay inside the 32 KiB heap.

mod common;
mod mainnet;
mod program_so;

use {
    common::*,
    side_market::{FIRST_ENTRY_ID, MAX_LIVE_ENTRIES, MAX_OUTCOMES, NONE_ID},
    solana_signer::Signer,
    std::io::Write,
};

const USDC: u64 = 1_000_000;
/// The plan's target for a trade at 66 outcomes: above it the client sets a ComputeBudget limit.
const TARGET: u64 = 200_000;
/// A transaction's most compute units: above it the live-entry maximum would have to fall.
const MAX_TX: u64 = 1_400_000;

struct Measured {
    outcomes: usize,
    buy: u64,
    new_leg: u64,
    sell: u64,
    /// A sale outside every window (after the deadline).
    sell_outside: u64,
    rebase: u64,
    halt: u64,
    void: u64,
    settle: u64,
}

fn measure(n: u8, entries: u16) -> Measured {
    let mut w = World::new();
    let m = w.open(n, None);
    let mass = (600_000 / u32::from(entries)).min(100_000);
    for k in 0..entries {
        w.add(&m, FIRST_ENTRY_ID + k, mass);
    }
    w.open_bounty(&m);
    let t = w.trader(10_000_000 * USDC);
    w.open_ticket(&m, t.key.pubkey());
    let last = FIRST_ENTRY_ID + entries - 1;

    let new_leg = expect_ok(w.trade(&m, &t, last, 5 * USDC as i64), "first buy").cu;
    let buy = expect_ok(w.trade(&m, &t, last, 5 * USDC as i64), "buy").cu;
    let sell = expect_ok(w.trade(&m, &t, last, -(3 * USDC as i64)), "sell").cu;
    let mut f = w.fork();
    f.set_time(LOCK + DAY);
    let sell_outside = expect_ok(
        f.trade(&m, &t, last, -(USDC as i64)),
        "a sell outside every window",
    )
    .cu;
    // 30 b of YES on NONE takes its exponent past base + 16: the whole book is rebased.
    let b = w.market(&m).b as i64;
    let base0 = w.market(&m).base;
    let rebase = expect_ok(w.trade(&m, &t, NONE_ID, 30 * b), "rebasing buy").cu;
    assert_ne!(w.market(&m).base, base0, "the big buy rebased the book");
    w.assert_d6(&m);
    // halt and void freeze every live outcome's marks; neither sets a ComputeBudget limit.
    let halt = expect_ok(w.as_operator(w.halt_ix(&m, w.k.operator.pubkey())), "halt").cu;
    let arb = w.k.arbiter.insecure_clone();
    let void = expect_ok(w.as_key(w.void_ix(&m, arb.pubkey()), &arb), "void").cu;
    let s = w.k.stranger.insecure_clone();
    let ix = w.settle_ix(&m, s.pubkey(), t.key.pubkey());
    let settle = expect_ok(w.as_key(ix, &s), "settle").cu;
    Measured {
        outcomes: w.market(&m).outcomes.len(),
        buy,
        new_leg,
        sell,
        sell_outside,
        rebase,
        halt,
        void,
        settle,
    }
}

#[test]
fn trade_compute_units_at_8_16_and_66_outcomes() {
    let mut rows = vec![];
    for (n, entries) in [(1u8, 6u16), (2, 14), (3, MAX_LIVE_ENTRIES)] {
        rows.push(measure(n, entries));
    }
    let mut err = std::io::stderr();
    for r in &rows {
        let _ = writeln!(
            err,
            "side_market CU at {} outcomes: buy {} | first buy (new leg) {} | sell {} | sell outside every window {} | rebasing buy {} | halt {} | void {} | settle {}",
            r.outcomes, r.buy, r.new_leg, r.sell, r.sell_outside, r.rebase, r.halt, r.void, r.settle
        );
    }
    assert_eq!(
        rows.iter().map(|r| r.outcomes).collect::<Vec<_>>(),
        [8, 16, 66]
    );
    let at66 = &rows[2];
    assert!(
        at66.buy <= TARGET,
        "a trade at 66 outcomes takes {} CU",
        at66.buy
    );
    assert!(
        at66.sell <= TARGET,
        "a sale at 66 outcomes takes {} CU",
        at66.sell
    );
    assert!(
        at66.sell_outside <= TARGET,
        "a sale outside every window at 66 outcomes takes {} CU",
        at66.sell_outside
    );
    assert!(
        at66.new_leg <= TARGET,
        "a first buy at 66 outcomes takes {} CU",
        at66.new_leg
    );
    assert!(
        at66.rebase <= MAX_TX / 2,
        "a rebasing trade at 66 outcomes takes {} CU",
        at66.rebase
    );
    for r in &rows {
        assert!(r.buy <= r.rebase);
        assert!(r.halt <= TARGET && r.void <= TARGET && r.settle <= TARGET);
    }
}

#[test]
fn the_largest_book_and_ticket_fit_the_heap() {
    let mut w = World::new();
    let m = w.open(4, None);
    let t = w.trader(10_000_000 * USDC);
    w.open_ticket(&m, t.key.pubkey());
    w.open_bounty(&m);
    // Fill every slot: 64 live entries plus scratched ones up to MAX_OUTCOMES, a leg on each.
    let mut id = FIRST_ENTRY_ID;
    let slots = usize::from(MAX_OUTCOMES) - 2;
    let scratched = slots - usize::from(MAX_LIVE_ENTRIES);
    for _ in 0..scratched {
        w.add(&m, id, 10_000);
        expect_ok(
            w.trade(&m, &t, id, USDC as i64),
            "a leg on a soon-scratched entry",
        );
        expect_ok(w.scratch(&m, id), "scratch");
        id += 1;
    }
    for _ in 0..MAX_LIVE_ENTRIES {
        w.add(&m, id, 10_000);
        expect_ok(w.trade(&m, &t, id, USDC as i64), "a leg");
        id += 1;
    }
    expect_ok(w.trade(&m, &t, NONE_ID, USDC as i64), "a leg on NONE");
    let market = w.market(&m);
    assert_eq!(market.outcomes.len(), usize::from(MAX_OUTCOMES));
    assert_eq!(
        w.ticket(&m, &t.key.pubkey()).legs.len(),
        usize::from(MAX_OUTCOMES) - 1
    );
    let ix = w.add_outcome_ix(&m, w.k.operator.pubkey(), id, 10_000);
    expect_fail(&w.as_operator(ix), "an outcome past MAX_OUTCOMES");
    let b = market.b as i64;
    let sent = expect_ok(
        w.trade(&m, &t, id - 1, 30 * b),
        "a rebasing buy on the largest book",
    );
    let _ = writeln!(
        std::io::stderr(),
        "side_market rebasing buy at {} outcomes ({} live) with {} legs: {} CU",
        market.outcomes.len(),
        market.book_len(),
        usize::from(MAX_OUTCOMES) - 1,
        sent.cu
    );
    w.assert_d6(&m);
    // Settle it all: the void path reads every leg too.
    let arb = w.k.arbiter.insecure_clone();
    expect_ok(w.as_key(w.void_ix(&m, arb.pubkey()), &arb), "void");
    w.settle(&m, t.key.pubkey());
    w.assert_d6(&m);
}
