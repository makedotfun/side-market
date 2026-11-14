//! Every instruction's happy path and refusals on the release build (LiteSVM under mainnet's
//! feature set): the Config, opening, windows, joins, tickets, YES and NO trades (several in one
//! transaction, the NO-close clamp), sells outside every window (between the bounty and race
//! windows, after a window ended early, from the deadline until sell_until; never while halted or
//! once proposed) and set_sell_until, halts, scratches, the disputed resolution, settlement, voids
//! at marks, fees and closing. D6 is checked from outside after every value-moving step.

mod common;
mod mainnet;
mod program_so;

use {
    anchor_lang::{error::ErrorCode, prelude::Pubkey, AnchorDeserialize, Discriminator},
    anchor_spl::{associated_token::get_associated_token_address, token},
    common::*,
    side_market::{
        math, MarketStatus, OutcomeStatus, SideMarketError, FIELD_ID, FIRST_ENTRY_ID,
        MAX_LIVE_ENTRIES, NONE_ID, VOID_GRACE,
    },
    solana_signer::Signer,
};

const E1: u16 = FIRST_ENTRY_ID;
const E2: u16 = FIRST_ENTRY_ID + 1;
const E3: u16 = FIRST_ENTRY_ID + 2;
const USDC: u64 = 1_000_000;

/// A market with three entries, the bounty window open.
fn market3(w: &mut World, n: u8) -> Mkt {
    let m = w.open(n, None);
    w.add(&m, E1, 200_000);
    w.add(&m, E2, 150_000);
    w.add(&m, E3, 100_000);
    w.open_bounty(&m);
    m
}

fn trader(w: &mut World, m: &Mkt, amount: u64) -> Trader {
    let t = w.trader(amount);
    w.open_ticket(m, t.key.pubkey());
    t
}

fn shares(w: &World, m: &Mkt, t: &Trader, id: u16) -> i64 {
    w.ticket(m, &t.key.pubkey())
        .legs
        .iter()
        .find(|l| l.outcome_id == id)
        .map_or(0, |l| l.shares)
}

#[test]
fn init_config_only_by_the_upgrade_authority_once() {
    let mut w = World::bare();
    let params = w.params();
    let stranger = w.k.stranger.insecure_clone();

    let ix = w.init_config_ix(&stranger.pubkey(), w.mint, params.clone());
    expect_code(
        &w.as_key(ix, &stranger),
        SideMarketError::NotUpgradeAuthority,
        "a stranger",
    );

    let upgrade = w.k.upgrade.insecure_clone();
    // A Token-2022 mint is refused by its owner.
    let t22 = Pubkey::new_unique();
    w.put_mint(t22, 6, TOKEN_2022_ID.parse().unwrap());
    let ix = w.init_config_ix(&upgrade.pubkey(), t22, params.clone());
    expect_code(
        &w.as_key(ix, &upgrade),
        anchor_code(ErrorCode::AccountOwnedByWrongProgram),
        "a Token-2022 mint",
    );
    // A 9-decimal mint.
    let nine = Pubkey::new_unique();
    w.put_mint(nine, 9, token::ID);
    let ix = w.init_config_ix(&upgrade.pubkey(), nine, params.clone());
    expect_code(
        &w.as_key(ix, &upgrade),
        SideMarketError::InvalidMint,
        "9 decimals",
    );
    // Out-of-bounds settings.
    for bad in [
        side_market::ConfigParams {
            operator: Pubkey::default(),
            ..params.clone()
        },
        side_market::ConfigParams {
            min_dispute_secs: 10,
            ..params.clone()
        },
        side_market::ConfigParams {
            max_fee_rate_ppm: 70_001,
            ..params.clone()
        },
    ] {
        let ix = w.init_config_ix(&upgrade.pubkey(), w.mint, bad);
        expect_code(
            &w.as_key(ix, &upgrade),
            SideMarketError::InvalidConfig,
            "bad params",
        );
    }
    // An immutable program has no upgrade authority: refused.
    w.set_upgrade_authority(&side_market::ID, None);
    let ix = w.init_config_ix(&upgrade.pubkey(), w.mint, params.clone());
    expect_code(
        &w.as_key(ix, &upgrade),
        SideMarketError::NotUpgradeAuthority,
        "immutable",
    );
    w.set_upgrade_authority(&side_market::ID, Some(upgrade.pubkey()));

    let ix = w.init_config_ix(&upgrade.pubkey(), w.mint, params.clone());
    expect_ok(w.as_key(ix, &upgrade), "init_config");
    let config: side_market::Config = w.anchor(&config_pda()).unwrap();
    assert_eq!(config.mint, w.mint);
    assert_eq!(config.operator, w.k.operator.pubkey());
    assert_eq!(config.min_dispute_secs, DISPUTE);

    let ix = w.init_config_ix(&upgrade.pubkey(), w.mint, params);
    expect_fail(&w.as_key(ix, &upgrade), "a second init_config");
}

#[test]
fn set_config_by_the_admin_alone() {
    let mut w = World::new();
    let mut params = w.params();
    params.operator = w.k.stranger.pubkey();
    let stranger = w.k.stranger.insecure_clone();
    let ix = w.set_config_ix(&stranger.pubkey(), params.clone());
    expect_code(
        &w.as_key(ix, &stranger),
        SideMarketError::UnauthorizedAdmin,
        "a stranger",
    );
    let admin = w.k.admin.insecure_clone();
    let bad = side_market::ConfigParams {
        max_fee_rate_ppm: 80_000,
        ..params.clone()
    };
    let ix = w.set_config_ix(&admin.pubkey(), bad);
    expect_code(
        &w.as_key(ix, &admin),
        SideMarketError::InvalidConfig,
        "fee cap",
    );
    let ix = w.set_config_ix(&admin.pubkey(), params);
    expect_ok(w.as_key(ix, &admin), "set_config");
    let config: side_market::Config = w.anchor(&config_pda()).unwrap();
    assert_eq!(config.operator, w.k.stranger.pubkey());
    assert_eq!(config.mint, w.mint);
}

#[test]
fn open_market_moves_the_house_deposit() {
    let mut w = World::new();
    let house_before = w.tokens(&w.house_token);
    let m = w.open(1, None);
    let market = w.market(&m);
    let (b, deposit) = math::liquidity_for(SEED, M_FLOOR).unwrap();
    assert_eq!(market.b, b);
    assert_eq!(market.deposit, deposit);
    assert_eq!(w.tokens(&m.vault), deposit);
    assert_eq!(house_before - w.tokens(&w.house_token), deposit);
    assert!(w.exists(&m.fees));
    assert_eq!(w.tokens(&m.fees), 0);
    assert_eq!(market.status, MarketStatus::Open);
    assert_eq!(market.void_after, LOCK + VOID_GRACE);
    assert_eq!(market.outcomes.len(), 2);
    assert_eq!(market.outcomes[0].id, NONE_ID);
    assert_eq!(market.outcomes[1].id, FIELD_ID);
    let p = w.prices(&m);
    assert!(
        (p[0].1 as i64 - NONE_MASS as i64).abs() <= 1,
        "NONE priced {}",
        p[0].1
    );
    w.assert_d6(&m);
    // The deposit covers the worst-case loss plus the cushion.
    let loss = math::max_loss(b, math::loss_per_b(M_FLOOR).unwrap()).unwrap();
    assert_eq!(deposit, loss + math::SEED_CUSHION);

    // Once per competition.
    let ix = w.open_market_ix(&m, w.args());
    let (op, house) = (w.k.operator.insecure_clone(), w.k.house.insecure_clone());
    expect_fail(&w.send(&[ix], &op, &[&house]), "a second open");
}

#[test]
fn open_market_refusals() {
    let mut w = World::new();
    let m = Mkt::of(cid(2));
    let (op, house, stranger) = (
        w.k.operator.insecure_clone(),
        w.k.house.insecure_clone(),
        w.k.stranger.insecure_clone(),
    );
    let ix = w.open_market_ix_with(
        &m,
        w.args(),
        stranger.pubkey(),
        house.pubkey(),
        w.house_token,
    );
    expect_code(
        &w.send(&[ix], &stranger, &[&house]),
        SideMarketError::UnauthorizedOperator,
        "operator",
    );
    let ix = w.open_market_ix_with(&m, w.args(), op.pubkey(), stranger.pubkey(), w.house_token);
    expect_code(
        &w.send(&[ix], &op, &[&stranger]),
        SideMarketError::UnauthorizedHouse,
        "house",
    );
    for (args, code) in [
        (
            side_market::OpenMarketArgs {
                fee_rate_ppm: MAX_FEE_RATE + 1,
                ..w.args()
            },
            SideMarketError::FeeRateTooHigh,
        ),
        (
            side_market::OpenMarketArgs {
                m_floor_ppm: 50,
                ..w.args()
            },
            SideMarketError::InvalidMarketParams,
        ),
        (
            side_market::OpenMarketArgs {
                none_mass_ppm: M_FLOOR - 1,
                ..w.args()
            },
            SideMarketError::MassBelowFloor,
        ),
        (
            side_market::OpenMarketArgs {
                none_mass_ppm: 1_000_000 - M_FLOOR + 1,
                ..w.args()
            },
            SideMarketError::MassBelowFloor,
        ),
        (
            side_market::OpenMarketArgs {
                lock_ts: 0,
                ..w.args()
            },
            SideMarketError::InvalidMarketParams,
        ),
        (
            side_market::OpenMarketArgs {
                seed: 100,
                ..w.args()
            },
            SideMarketError::InvalidMarketParams,
        ),
    ] {
        let ix = w.open_market_ix(&m, args);
        expect_code(&w.send(&[ix], &op, &[&house]), code, "bad args");
    }
    // A vault address someone sent lamports to first still opens.
    let rent0 = w.svm.minimum_balance_for_rent_exemption(0);
    w.svm.airdrop(&m.vault, rent0).unwrap();
    let ix = w.open_market_ix(&m, w.args());
    expect_ok(
        w.send(&[ix], &op, &[&house]),
        "open over a funded vault address",
    );
    assert_eq!(w.tokens(&m.vault), w.market(&m).deposit);
}

#[test]
fn windows_gate_trading() {
    let mut w = World::new();
    let m = w.open(3, None);
    w.add(&m, E1, 200_000);
    let t = trader(&mut w, &m, 1_000 * USDC);
    // Closed until set.
    expect_code(
        &w.trade(&m, &t, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "no window",
    );
    let op = w.k.operator.pubkey();
    for bad in [
        [NOW, LOCK + 1, 0, 0],
        [NOW + 5, NOW, 0, 0],
        [0, 0, LOCK + 5, LOCK],
    ] {
        let ix = w.set_windows_ix(&m, op, bad);
        expect_code(
            &w.as_operator(ix),
            SideMarketError::InvalidWindow,
            "bad window",
        );
    }
    let ix = w.set_windows_ix(&m, w.k.stranger.pubkey(), [NOW, LOCK, 0, 0]);
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger",
    );

    // The race window after the deadline.
    w.set_windows(&m, [NOW, NOW + DAY, LOCK + DAY, LOCK + 2 * DAY]);
    expect_ok(w.trade(&m, &t, E1, USDC as i64), "inside the bounty window");
    w.set_time(NOW + 2 * DAY);
    expect_code(
        &w.trade(&m, &t, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "between windows",
    );
    w.set_time(LOCK + DAY);
    expect_ok(
        w.trade(&m, &t, E1, -(USDC as i64) / 2),
        "inside the race window",
    );
    w.set_time(LOCK + 2 * DAY);
    expect_code(
        &w.trade(&m, &t, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "after the race",
    );
}

#[test]
fn add_outcome_splits_field() {
    let mut w = World::new();
    let m = w.open(4, None);
    w.add(&m, E1, 200_000);
    let market = w.market(&m);
    assert_eq!(market.outcomes[1].mass_ppm, 1_000_000 - NONE_MASS - 200_000);
    assert_eq!(market.live_entries, 1);
    let p = w.prices(&m);
    let e1 = p.iter().find(|(id, _)| *id == E1).unwrap().1;
    assert!((e1 as i64 - 200_000).abs() <= 1, "E1 priced {e1}");
    let op = w.k.operator.pubkey();
    for (id, mass, code) in [
        (E1, 10_000, SideMarketError::InvalidOutcomeId),
        (FIELD_ID, 10_000, SideMarketError::InvalidOutcomeId),
        (E2, M_FLOOR - 1, SideMarketError::MassBelowFloor),
        (E2, 750_000 - M_FLOOR + 1, SideMarketError::MassBelowFloor),
    ] {
        let ix = w.add_outcome_ix(&m, op, id, mass);
        expect_code(&w.as_operator(ix), code, "bad join");
    }
    let ix = w.add_outcome_ix(&m, w.k.stranger.pubkey(), E2, 10_000);
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger",
    );
    // Ids may skip but never go back.
    w.add(&m, E1 + 10, 10_000);
    let ix = w.add_outcome_ix(&m, op, E1 + 5, 10_000);
    expect_code(
        &w.as_operator(ix),
        SideMarketError::InvalidOutcomeId,
        "an id below the last",
    );
}

#[test]
fn at_most_64_live_entries() {
    let mut w = World::new();
    let m = w.open(5, None);
    for k in 0..MAX_LIVE_ENTRIES {
        w.add(&m, E1 + k, 10_000);
    }
    let ix = w.add_outcome_ix(&m, w.k.operator.pubkey(), E1 + MAX_LIVE_ENTRIES, 10_000);
    expect_code(
        &w.as_operator(ix),
        SideMarketError::TooManyEntries,
        "a 65th",
    );
    // A scratch frees a live slot.
    expect_ok(w.scratch(&m, E1), "scratch");
    w.add(&m, E1 + MAX_LIVE_ENTRIES, 10_000);
    assert_eq!(w.market(&m).live_entries, MAX_LIVE_ENTRIES);
}

#[test]
fn yes_and_no_trades_move_prices_and_money() {
    let mut w = World::new();
    let m = market3(&mut w, 6);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let p0 = w.prices(&m);
    let price = |p: &[(u16, u32)], id: u16| p.iter().find(|(i, _)| *i == id).unwrap().1;

    // YES on E1: its price rises, the vault takes the cost, the fees account the fee.
    let (vault0, fees0, a0) = (w.tokens(&m.vault), w.tokens(&m.fees), w.tokens(&a.token));
    expect_ok(w.trade(&m, &a, E1, 20 * USDC as i64), "buy YES");
    let p1 = w.prices(&m);
    assert!(price(&p1, E1) > price(&p0, E1));
    let paid = a0 - w.tokens(&a.token);
    let fee = w.tokens(&m.fees) - fees0;
    assert!(fee > 0);
    assert_eq!(w.tokens(&m.vault) - vault0, paid - fee);
    assert!(paid < 20 * USDC);
    assert_eq!(shares(&w, &m, &a, E1), 20 * USDC as i64);
    let leg = w.ticket(&m, &a.key.pubkey()).legs[0];
    assert_eq!(leg.cost_in, paid);
    w.assert_d6(&m);

    // NO on E2: its price falls, N grows.
    expect_ok(w.trade(&m, &a, E2, -(30 * USDC as i64)), "buy NO");
    assert!(price(&w.prices(&m), E2) < price(&p1, E2));
    assert_eq!(w.market(&m).no_total, 30 * USDC);
    assert_eq!(shares(&w, &m, &a, E2), -(30 * USDC as i64));
    w.assert_d6(&m);

    // NONE is tradeable too.
    expect_ok(w.trade(&m, &a, NONE_ID, 5 * USDC as i64), "buy YES on NONE");

    // Sell half of each: money comes back, never more than paid.
    let before = w.tokens(&a.token);
    expect_ok(w.trade(&m, &a, E1, -(10 * USDC as i64)), "sell YES");
    expect_ok(w.trade(&m, &a, E2, 15 * USDC as i64), "sell NO");
    assert!(w.tokens(&a.token) > before);
    assert_eq!(w.market(&m).no_total, 15 * USDC);
    w.assert_d6(&m);

    // Buying back and selling out leaves the trader behind (fees and rounding), never ahead.
    let start = w.tokens(&a.token);
    expect_ok(w.trade(&m, &a, E3, 7 * USDC as i64), "buy");
    expect_ok(w.trade(&m, &a, E3, -(7 * USDC as i64)), "sell");
    assert!(w.tokens(&a.token) < start);
}

#[test]
fn trade_refusals() {
    let mut w = World::new();
    let m = market3(&mut w, 7);
    let a = trader(&mut w, &m, 1_000 * USDC);
    // No ticket.
    let b = w.trader(1_000 * USDC);
    expect_code(
        &w.trade(&m, &b, E1, USDC as i64),
        anchor_code(ErrorCode::AccountNotInitialized),
        "no ticket",
    );
    expect_code(
        &w.trade(&m, &a, FIELD_ID, USDC as i64),
        SideMarketError::FieldNotTradeable,
        "FIELD",
    );
    expect_code(
        &w.trade(&m, &a, 99, USDC as i64),
        SideMarketError::UnknownOutcome,
        "unknown",
    );
    expect_code(&w.trade(&m, &a, E1, 0), SideMarketError::ZeroDelta, "zero");
    expect_ok(w.trade(&m, &a, E1, 5 * USDC as i64), "buy");
    expect_code(
        &w.trade(&m, &a, E1, -(6 * USDC as i64)),
        SideMarketError::CrossesZero,
        "flip",
    );
    // Limits: a buy paying more than its limit, a sale receiving less.
    let ix = w.trade_ix(&m, &a, E1, USDC as i64, 1_000);
    let k = a.key.insecure_clone();
    expect_code(
        &w.as_key(ix, &k),
        SideMarketError::LimitExceeded,
        "buy limit",
    );
    let ix = w.trade_ix(&m, &a, E1, -(USDC as i64), USDC);
    expect_code(
        &w.as_key(ix, &k),
        SideMarketError::LimitExceeded,
        "sell limit",
    );
    // A tight but sufficient limit passes.
    let ix = w.trade_ix(&m, &a, E1, USDC as i64, USDC);
    expect_ok(w.as_key(ix, &k), "buy within limit");
    // Not enough tokens.
    let poor = trader(&mut w, &m, 10);
    expect_fail(
        &w.trade(&m, &poor, E1, 10 * USDC as i64),
        "an underfunded buy",
    );

    // Revoked: buys refused, sells allowed; restored: buys again.
    let ix = w.revoke_ix(&m, w.k.operator.pubkey(), a.key.pubkey(), true);
    expect_ok(w.as_operator(ix), "revoke");
    let ix = w.revoke_ix(&m, w.k.stranger.pubkey(), a.key.pubkey(), false);
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger revokes",
    );
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TicketRevoked,
        "revoked buy",
    );
    expect_code(
        &w.trade(&m, &a, E2, -(USDC as i64)),
        SideMarketError::TicketRevoked,
        "revoked NO buy",
    );
    expect_ok(w.trade(&m, &a, E1, -(USDC as i64)), "revoked sell");
    let ix = w.revoke_ix(&m, w.k.operator.pubkey(), a.key.pubkey(), false);
    expect_ok(w.as_operator(ix), "restore");
    expect_ok(w.trade(&m, &a, E1, USDC as i64), "restored buy");

    // A second ticket for the same wallet: refused.
    let ix = w.open_ticket_ix(&m, w.k.operator.pubkey(), a.key.pubkey());
    expect_fail(&w.as_operator(ix), "a second ticket");
    let ix = w.open_ticket_ix(&m, w.k.stranger.pubkey(), Pubkey::new_unique());
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger opens a ticket",
    );
}

#[test]
fn several_trades_in_one_transaction() {
    let mut w = World::new();
    let m = market3(&mut w, 8);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let ixs = [
        cu_limit(1_400_000),
        w.trade_ix(&m, &a, E1, 3 * USDC as i64, u64::MAX),
        w.trade_ix(&m, &a, E2, -(4 * USDC as i64), u64::MAX),
        w.trade_ix(&m, &a, E1, -(USDC as i64), 0),
        w.trade_ix(&m, &a, NONE_ID, 2 * USDC as i64, u64::MAX),
    ];
    let k = a.key.insecure_clone();
    let sent = expect_ok(w.send(&ixs, &k, &[]), "four trades");
    assert_eq!(sent.events.len(), 4);
    assert_eq!(shares(&w, &m, &a, E1), 2 * USDC as i64);
    assert_eq!(shares(&w, &m, &a, E2), -(4 * USDC as i64));
    assert_eq!(shares(&w, &m, &a, NONE_ID), 2 * USDC as i64);
    assert_eq!(w.ticket(&m, &a.key.pubkey()).legs.len(), 3);
    w.assert_d6(&m);
}

#[test]
fn a_no_close_on_a_near_certain_outcome_is_clamped_to_zero() {
    let mut w = World::new();
    let m = market3(&mut w, 9);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let big = trader(&mut w, &m, 100_000_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, -3), "buy 3 NO base units");
    let b = w.market(&m).b as i64;
    expect_ok(w.trade(&m, &big, E1, 40 * b), "push E1 to certainty");
    let before = w.tokens(&a.token);
    let sent = expect_ok(w.trade(&m, &a, E1, 3), "sell the NO back");
    assert_eq!(w.tokens(&a.token), before, "the clamped sale pays nothing");
    assert_eq!(shares(&w, &m, &a, E1), 0);
    assert!(!sent.events.is_empty());
    w.assert_d6(&m);
}

#[test]
fn halt_and_resume() {
    let mut w = World::new();
    let m = market3(&mut w, 10);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    let s = w.k.stranger.insecure_clone();
    let ix = w.halt_ix(&m, s.pubkey());
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedHalt,
        "a stranger halts",
    );

    let ix = w.halt_ix(&m, w.k.operator.pubkey());
    expect_ok(w.as_operator(ix), "operator halt");
    let market = w.market(&m);
    assert_eq!(market.status, MarketStatus::Halted);
    for o in market.outcomes.iter() {
        assert!(
            o.yes_value > 0 && o.no_value > 0,
            "marks frozen for {}",
            o.id
        );
        assert!(u128::from(o.yes_value) + u128::from(o.no_value) <= 1 << 64);
    }
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::InvalidStatus,
        "halted",
    );
    let ix = w.halt_ix(&m, w.k.operator.pubkey());
    expect_code(
        &w.as_operator(ix),
        SideMarketError::InvalidStatus,
        "halt twice",
    );
    // Joins still work while halted (and refreeze the marks).
    w.add(&m, E3 + 1, 10_000);
    assert!(w.market(&m).outcomes.last().unwrap().yes_value > 0);
    let ix = w.resume_ix(&m, w.k.operator.pubkey());
    expect_ok(w.as_operator(ix), "operator resume");
    expect_ok(w.trade(&m, &a, E1, USDC as i64), "trading again");

    // The guardian's halt: the operator cannot lift it, the admin can.
    let g = w.k.guardian.insecure_clone();
    let ix = w.halt_ix(&m, g.pubkey());
    expect_ok(w.as_key(ix, &g), "guardian halt");
    assert!(w.market(&m).halted_by_guardian);
    let ix = w.resume_ix(&m, w.k.operator.pubkey());
    expect_code(
        &w.as_operator(ix),
        SideMarketError::UnauthorizedResume,
        "operator lifts a guardian halt",
    );
    let ix = w.resume_ix(&m, s.pubkey());
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedResume,
        "a stranger resumes",
    );
    let admin = w.k.admin.insecure_clone();
    let ix = w.resume_ix(&m, admin.pubkey());
    expect_ok(w.as_key(ix, &admin), "admin resume");
    let ix = w.resume_ix(&m, admin.pubkey());
    expect_code(
        &w.as_key(ix, &admin),
        SideMarketError::InvalidStatus,
        "resume an open market",
    );
}

#[test]
fn scratch_unwinds_at_the_amm_and_pays_fixed_values() {
    let mut w = World::new();
    let m = market3(&mut w, 11);
    let yes = trader(&mut w, &m, 1_000 * USDC);
    let no = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &yes, E1, 20 * USDC as i64), "YES on E1");
    expect_ok(w.trade(&m, &no, E1, -(5 * USDC as i64)), "NO on E1");
    expect_ok(w.trade(&m, &no, E2, -(8 * USDC as i64)), "NO on E2");
    expect_ok(w.trade(&m, &yes, E3, 4 * USDC as i64), "YES on E3");

    for id in [NONE_ID, FIELD_ID] {
        expect_code(
            &w.scratch(&m, id),
            SideMarketError::NotAnEntry,
            "NONE or FIELD",
        );
    }
    let ix = w.scratch_ix(&m, w.k.stranger.pubkey(), E1);
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger",
    );

    let field_before = w.market(&m).outcomes[1].mass_ppm;
    // E1: q > 0. E2: q < 0.
    expect_ok(w.scratch(&m, E1), "scratch E1");
    expect_ok(w.scratch(&m, E2), "scratch E2");
    let market = w.market(&m);
    let e1 = market.outcomes[market.index_of(E1).unwrap()];
    let e2 = market.outcomes[market.index_of(E2).unwrap()];
    assert_eq!(e1.status, OutcomeStatus::Scratched);
    assert_eq!(
        market.outcomes[1].mass_ppm,
        field_before + 200_000 + 150_000
    );
    assert_eq!(market.no_total, 0, "NO on scratched outcomes left N");
    assert!(market.scratch_owed > 0);
    assert!(e1.yes_value > 0 && e1.no_value > 0);
    assert!(e2.yes_value > 0 && e2.no_value > 0);
    assert_eq!(market.live_entries, 1);
    w.assert_d6(&m);
    expect_code(
        &w.scratch(&m, E1),
        SideMarketError::OutcomeNotLive,
        "scratch twice",
    );
    expect_code(
        &w.trade(&m, &yes, E1, -(USDC as i64)),
        SideMarketError::OutcomeNotLive,
        "trade a scratched one",
    );

    // Never after lock_ts.
    w.set_time(LOCK);
    expect_code(
        &w.scratch(&m, E3),
        SideMarketError::ScratchAfterLock,
        "after lock",
    );

    // E3 wins: the scratched legs are paid their values, E3's YES its shares.
    expect_ok(w.propose(&m, E3), "propose");
    w.set_time(LOCK + DISPUTE);
    expect_ok(w.as_operator(w.finalize_ix(&m)), "finalize");
    let owed = w.market(&m).scratch_owed;
    let got_yes = w.settle(&m, yes.key.pubkey());
    let got_no = w.settle(&m, no.key.pubkey());
    let want_yes = math::value_of(20 * USDC, e1.yes_value) + 4 * USDC;
    let want_no = math::value_of(5 * USDC, e1.no_value) + math::value_of(8 * USDC, e2.no_value);
    assert_eq!(got_yes, want_yes);
    assert_eq!(got_no, want_no);
    let dust = w.market(&m).scratch_owed;
    assert!(dust <= 4, "F left {dust} of {owed}");
    w.assert_d6(&m);
}

#[test]
fn a_scratch_with_no_net_position_pays_its_marks() {
    let mut w = World::new();
    let m = market3(&mut w, 12);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let b = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E2, 6 * USDC as i64), "YES");
    expect_ok(w.trade(&m, &b, E2, -(6 * USDC as i64)), "NO");
    assert_eq!(w.market(&m).outcomes[3].q, 0);
    expect_ok(w.scratch(&m, E2), "scratch at q = 0");
    let o = w.market(&m).outcomes[3];
    let sum = u128::from(o.yes_value) + u128::from(o.no_value);
    assert!(
        sum <= 1 << 64 && sum > (1 << 64) - 1_000_000,
        "marks {} + {}",
        o.yes_value,
        o.no_value
    );
    w.assert_d6(&m);
}

#[test]
fn propose_override_finalize_settle_close() {
    let mut w = World::new();
    let m = market3(&mut w, 13);
    let a = trader(&mut w, &m, 1_000 * USDC);
    let b = trader(&mut w, &m, 1_000 * USDC);
    let c = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 30 * USDC as i64), "a YES E1");
    expect_ok(w.trade(&m, &b, E2, 25 * USDC as i64), "b YES E2");
    expect_ok(w.trade(&m, &c, E1, -(10 * USDC as i64)), "c NO E1");
    expect_ok(w.trade(&m, &c, E3, -(12 * USDC as i64)), "c NO E3");
    let fees = w.tokens(&m.fees);
    assert!(fees > 0);

    // Not before trading ends.
    expect_code(
        &w.propose(&m, E1),
        SideMarketError::TradingNotEnded,
        "before lock",
    );
    w.set_time(LOCK);
    let s = w.k.stranger.insecure_clone();
    let ix = w.propose_ix(&m, s.pubkey(), E1, [7; 32], None);
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedResolver,
        "a stranger proposes",
    );
    expect_code(
        &w.propose(&m, 77),
        SideMarketError::InvalidWinner,
        "an unknown winner",
    );
    let ix = w.propose_ix(&m, w.k.resolver.pubkey(), E1, [0; 32], None);
    let r = w.k.resolver.insecure_clone();
    expect_code(
        &w.as_key(ix, &r),
        SideMarketError::InvalidResult,
        "a zero result",
    );
    expect_ok(w.propose(&m, E1), "propose E1");
    expect_code(
        &w.propose(&m, E1),
        SideMarketError::InvalidStatus,
        "propose twice",
    );
    // No settlement, trade or join while proposed.
    let ix = w.settle_ix(&m, s.pubkey(), a.key.pubkey());
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::InvalidStatus,
        "settle before final",
    );

    // The arbiter overrides to E2 during the dispute delay.
    let ix = w.override_ix(&m, s.pubkey(), E2, [8; 32]);
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedArbiter,
        "a stranger overrides",
    );
    let arb = w.k.arbiter.insecure_clone();
    let ix = w.override_ix(&m, arb.pubkey(), E2, [8; 32]);
    expect_ok(w.as_key(ix, &arb), "override");
    expect_code(
        &w.as_operator(w.finalize_ix(&m)),
        SideMarketError::DisputeOpen,
        "finalize early",
    );
    w.set_time(LOCK + DISPUTE);
    let ix = w.override_ix(&m, arb.pubkey(), E1, [8; 32]);
    expect_code(
        &w.as_key(ix, &arb),
        SideMarketError::DisputeOver,
        "override late",
    );
    expect_ok(w.as_key(w.finalize_ix(&m), &s), "finalize (anyone)");
    let market = w.market(&m);
    assert_eq!(market.status, MarketStatus::Final);
    assert_eq!(market.winner, E2);
    assert_eq!(market.result_sha256, [8; 32]);

    // close_market waits for every ticket.
    let ix = w.close_ix(&m, w.k.operator.pubkey());
    expect_code(
        &w.as_operator(ix),
        SideMarketError::TicketsOpen,
        "close with tickets",
    );

    let op_lamports = w.lamports(&w.k.operator.pubkey());
    assert_eq!(w.settle(&m, a.key.pubkey()), 0, "a held E1, the loser");
    assert_eq!(w.settle(&m, b.key.pubkey()), 25 * USDC, "b held E2");
    assert_eq!(
        w.settle(&m, c.key.pubkey()),
        22 * USDC,
        "c's NO on E1 and E3 both pay"
    );
    assert!(!w.exists(&ticket_pda(&m.market, &a.key.pubkey())));
    assert!(
        w.lamports(&w.k.operator.pubkey()) > op_lamports,
        "ticket rent back to the operator"
    );
    // a's payout was 0: no associated token account was created for it (the crank pays no rent
    // for a loser); b's was.
    assert!(!w.exists(&get_associated_token_address(&a.key.pubkey(), &w.mint)));
    assert!(w.exists(&get_associated_token_address(&b.key.pubkey(), &w.mint)));
    w.assert_d6(&m);
    let market = w.market(&m);
    assert_eq!(market.tickets, 0);
    assert_eq!(market.no_total, 0);

    // Close: surplus and fees to the house; the house lost at most its bound.
    let house0 = w.tokens(&w.house_token);
    let surplus = w.tokens(&m.vault);
    let ix = w.close_ix(&m, s.pubkey());
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger closes",
    );
    expect_ok(
        w.as_operator(w.close_ix(&m, w.k.operator.pubkey())),
        "close",
    );
    assert_eq!(w.tokens(&w.house_token) - house0, surplus + fees);
    assert!(!w.exists(&m.market) && !w.exists(&m.vault) && !w.exists(&m.fees));
    let loss = market.deposit.saturating_sub(surplus);
    let bound = math::max_loss(market.b, math::loss_per_b(M_FLOOR).unwrap()).unwrap();
    assert!(loss <= bound, "house lost {loss} > {bound}");
}

#[test]
fn field_and_none_can_win() {
    for (n, winner) in [(14u8, FIELD_ID), (15u8, NONE_ID)] {
        let mut w = World::new();
        let m = market3(&mut w, n);
        let a = trader(&mut w, &m, 1_000 * USDC);
        expect_ok(w.trade(&m, &a, E1, -(10 * USDC as i64)), "NO E1");
        expect_ok(w.trade(&m, &a, NONE_ID, 3 * USDC as i64), "YES NONE");
        w.set_time(LOCK);
        expect_ok(w.propose(&m, winner), "propose");
        w.set_time(LOCK + DISPUTE);
        expect_ok(w.as_operator(w.finalize_ix(&m)), "finalize");
        let got = w.settle(&m, a.key.pubkey());
        let want = 10 * USDC + if winner == NONE_ID { 3 * USDC } else { 0 };
        assert_eq!(got, want);
        w.assert_d6(&m);
    }
}

#[test]
fn void_pays_frozen_marks_and_stays_solvent() {
    let mut w = World::new();
    let m = market3(&mut w, 16);
    let traders: Vec<Trader> = (0..4).map(|_| trader(&mut w, &m, 10_000 * USDC)).collect();
    let moves = [
        (0, E1, 200),
        (1, E1, -150),
        (2, E2, 300),
        (3, E3, -400),
        (0, NONE_ID, 50),
        (1, E2, -80),
    ];
    for (i, id, d) in moves {
        expect_ok(w.trade(&m, &traders[i], id, d * USDC as i64), "trade");
    }
    expect_ok(w.scratch(&m, E3), "scratch E3");
    let ix = w.halt_ix(&m, w.k.operator.pubkey());
    expect_ok(w.as_operator(ix), "halt");

    // Anyone may void only after void_after; the arbiter any time before final.
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(w.void_ix(&m, s.pubkey()), &s),
        SideMarketError::VoidNotYet,
        "early",
    );
    let marks: Vec<(u16, u64, u64)> = w
        .market(&m)
        .outcomes
        .iter()
        .map(|o| (o.id, o.yes_value, o.no_value))
        .collect();
    w.set_time(LOCK + VOID_GRACE);
    expect_ok(
        w.as_key(w.void_ix(&m, s.pubkey()), &s),
        "void after void_after",
    );
    let market = w.market(&m);
    assert_eq!(market.status, MarketStatus::Void);
    for (o, (id, yes, no)) in market.outcomes.iter().zip(marks) {
        assert_eq!(o.id, id);
        assert_eq!(
            (o.yes_value, o.no_value),
            (yes, no),
            "void pays the halt's marks"
        );
    }
    w.assert_d6(&m);

    let mut paid = 0;
    for t in &traders {
        let ticket = w.ticket(&m, &t.key.pubkey());
        let want: u64 = ticket
            .legs
            .iter()
            .map(|l| market.leg_payout(l.outcome_id, l.shares).unwrap())
            .sum();
        let got = w.settle(&m, t.key.pubkey());
        assert_eq!(got, want);
        paid += got;
        w.assert_d6(&m);
    }
    assert!(paid > 0);
    expect_ok(
        w.as_operator(w.close_ix(&m, w.k.operator.pubkey())),
        "close",
    );
}

#[test]
fn the_arbiter_voids_a_proposed_market_and_anyone_never_does() {
    let mut w = World::new();
    let m = market3(&mut w, 17);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    w.set_time(LOCK + VOID_GRACE);
    expect_ok(w.propose(&m, E1), "propose");
    let s = w.k.stranger.insecure_clone();
    expect_code(
        &w.as_key(w.void_ix(&m, s.pubkey()), &s),
        SideMarketError::InvalidStatus,
        "anyone on a proposal",
    );
    let arb = w.k.arbiter.insecure_clone();
    expect_ok(w.as_key(w.void_ix(&m, arb.pubkey()), &arb), "arbiter void");
    let got = w.settle(&m, a.key.pubkey());
    assert!(got > 0 && got < 10 * USDC);
    expect_code(
        &w.as_key(w.void_ix(&m, arb.pubkey()), &arb),
        SideMarketError::InvalidStatus,
        "void twice",
    );
}

#[test]
fn sweep_fees_to_the_house() {
    let mut w = World::new();
    let m = market3(&mut w, 18);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 50 * USDC as i64), "buy");
    let fees = w.tokens(&m.fees);
    assert!(fees > 0);
    let s = w.k.stranger.insecure_clone();
    let ix = w.sweep_ix(&m, s.pubkey(), w.house_token);
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger",
    );
    // Only a house token account.
    let ix = w.sweep_ix(&m, w.k.operator.pubkey(), a.token);
    expect_code(
        &w.as_operator(ix),
        anchor_code(ErrorCode::ConstraintTokenOwner),
        "not the house's",
    );
    let house0 = w.tokens(&w.house_token);
    let ix = w.sweep_ix(&m, w.k.operator.pubkey(), w.house_token);
    let sent = expect_ok(w.as_operator(ix), "sweep");
    assert_eq!(w.tokens(&w.house_token) - house0, fees);
    assert_eq!(w.tokens(&m.fees), 0);
    // The event is FeesSwept.
    let body = &sent.events[0];
    assert_eq!(&body[..8], side_market::FeesSwept::DISCRIMINATOR);
    let ev = side_market::FeesSwept::try_from_slice(&body[8..]).unwrap();
    assert_eq!(ev.amount, fees);
    w.assert_d6(&m);
}

// ---------------------------------------------------------------------- sells outside windows

#[test]
fn a_position_sells_between_the_bounty_and_the_race_window() {
    let mut w = World::new();
    let m = w.open(19, None);
    w.add(&m, E1, 200_000);
    w.add(&m, E2, 150_000);
    // The bounty window, then (after the deadline) the race window.
    w.set_windows(&m, [NOW, NOW + DAY, LOCK + DAY, LOCK + 2 * DAY]);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(
        w.trade(&m, &a, E1, 10 * USDC as i64),
        "YES in the bounty window",
    );
    expect_ok(
        w.trade(&m, &a, E2, -(6 * USDC as i64)),
        "NO in the bounty window",
    );

    // Between the windows: every buy is refused, every sell lands.
    w.set_time(NOW + 2 * DAY);
    for (id, delta, what) in [
        (E1, USDC as i64, "a YES buy between the windows"),
        (E2, -(USDC as i64), "a NO buy between the windows"),
        (
            NONE_ID,
            USDC as i64,
            "a first buy on a new leg between the windows",
        ),
    ] {
        expect_code(
            &w.trade(&m, &a, id, delta),
            SideMarketError::TradingClosed,
            what,
        );
    }
    let before = w.tokens(&a.token);
    let sent = expect_ok(
        w.trade(&m, &a, E1, -(4 * USDC as i64)),
        "a YES sell between the windows",
    );
    assert!(w.tokens(&a.token) > before, "the sale paid the wallet");
    assert_eq!(shares(&w, &m, &a, E1), 6 * USDC as i64);
    assert_eq!(&sent.events[0][..8], side_market::Traded::DISCRIMINATOR);
    expect_ok(
        w.trade(&m, &a, E2, 2 * USDC as i64),
        "a NO sell between the windows",
    );
    assert_eq!(shares(&w, &m, &a, E2), -(4 * USDC as i64));
    w.assert_d6(&m);
    // A sell still never crosses zero.
    expect_code(
        &w.trade(&m, &a, E1, -(7 * USDC as i64)),
        SideMarketError::CrossesZero,
        "a flip between the windows",
    );
    // A revoked ticket sells too, and no operator signs a sale.
    let ix = w.revoke_ix(&m, w.k.operator.pubkey(), a.key.pubkey(), true);
    expect_ok(w.as_operator(ix), "revoke");
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a revoked ticket's sell between the windows",
    );

    // After the deadline, before the race window: still sells only.
    w.set_time(LOCK + DAY / 2);
    let ix = w.revoke_ix(&m, w.k.operator.pubkey(), a.key.pubkey(), false);
    expect_ok(w.as_operator(ix), "restore");
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "a buy after the deadline",
    );
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a sell after the deadline",
    );
    // Inside the race window buys land again.
    w.set_time(LOCK + DAY);
    expect_ok(w.trade(&m, &a, E1, USDC as i64), "a buy in the race window");
    assert_eq!(shares(&w, &m, &a, E1), 5 * USDC as i64);
    w.assert_d6(&m);
}

#[test]
fn a_window_ended_early_by_set_windows_still_lets_a_holder_sell() {
    let mut w = World::new();
    let m = market3(&mut w, 20);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "YES");
    expect_ok(w.trade(&m, &a, E2, -(5 * USDC as i64)), "NO");
    // The admin's toggle goes off a day in: the operator closes every window early.
    w.set_time(NOW + DAY);
    w.set_windows(&m, [0, 0, 0, 0]);
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "a buy once the window was closed",
    );
    expect_ok(
        w.trade(&m, &a, E1, -(5 * USDC as i64)),
        "a YES sell once the window was closed",
    );
    expect_ok(
        w.trade(&m, &a, E2, 5 * USDC as i64),
        "the whole NO position sold once the window was closed",
    );
    assert_eq!(shares(&w, &m, &a, E2), 0);
    // Flat again, a trade on E2 is a buy: refused.
    expect_code(
        &w.trade(&m, &a, E2, -(USDC as i64)),
        SideMarketError::TradingClosed,
        "a NO buy on a closed leg",
    );
    // A window that ended (it lies in the past) does the same.
    w.set_windows(&m, [NOW, NOW + DAY / 2, 0, 0]);
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "a buy after a window that ended",
    );
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a sell after a window that ended",
    );
    assert_eq!(shares(&w, &m, &a, E1), 4 * USDC as i64);
    w.assert_d6(&m);
}

#[test]
fn a_bounty_only_market_sells_from_the_deadline_until_sell_until() {
    let mut w = World::new();
    // The bounty window [NOW, LOCK) and no race window.
    let m = market3(&mut w, 21);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "YES E1");
    expect_ok(w.trade(&m, &a, E3, -(4 * USDC as i64)), "NO E3");
    assert_eq!(
        w.market(&m).sell_until,
        0,
        "open_market leaves sell_until unset"
    );

    // From the deadline: sells only.
    w.set_time(LOCK);
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "a buy at the deadline",
    );
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a sell at the deadline",
    );

    // The result becomes public at `at`: sells land until then.
    let at = LOCK + 3 * DAY;
    let sent = w.set_sell_until(&m, at);
    let body = &sent.events[0];
    assert_eq!(&body[..8], side_market::SellUntilSet::DISCRIMINATOR);
    let ev = side_market::SellUntilSet::try_from_slice(&body[8..]).unwrap();
    assert_eq!((ev.market, ev.sell_until), (m.market, at));
    assert_eq!(w.market(&m).sell_until, at);
    w.set_time(at - 1);
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a sell one second before sell_until",
    );
    w.set_time(at);
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::TradingClosed,
        "a YES sell at sell_until",
    );
    expect_code(
        &w.trade(&m, &a, E3, USDC as i64),
        SideMarketError::TradingClosed,
        "a NO sell at sell_until",
    );
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "a buy at sell_until",
    );
    // 0 clears it: sells land until the market is proposed.
    w.set_sell_until(&m, 0);
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a sell with sell_until cleared",
    );
    w.assert_d6(&m);

    // Once proposed nothing trades, then once final.
    expect_ok(w.propose(&m, E1), "propose");
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::InvalidStatus,
        "a YES sell once proposed",
    );
    expect_code(
        &w.trade(&m, &a, E3, USDC as i64),
        SideMarketError::InvalidStatus,
        "a NO sell once proposed",
    );
    w.set_time(at + DISPUTE);
    expect_ok(w.as_operator(w.finalize_ix(&m)), "finalize");
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::InvalidStatus,
        "a sell once final",
    );
    // a kept 7 YES of E1, the winner, and 4 NO of E3, a loser.
    assert_eq!(w.settle(&m, a.key.pubkey()), 11 * USDC);
    w.assert_d6(&m);
}

#[test]
fn a_halted_market_refuses_sells_inside_and_outside_windows() {
    let mut w = World::new();
    let m = market3(&mut w, 22);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    expect_ok(w.as_operator(w.halt_ix(&m, w.k.operator.pubkey())), "halt");
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::InvalidStatus,
        "a sell while halted, inside the window",
    );
    w.set_time(LOCK + DAY);
    expect_code(
        &w.trade(&m, &a, E1, -(USDC as i64)),
        SideMarketError::InvalidStatus,
        "a sell while halted, outside every window",
    );
    expect_ok(
        w.as_operator(w.resume_ix(&m, w.k.operator.pubkey())),
        "resume",
    );
    expect_ok(
        w.trade(&m, &a, E1, -(USDC as i64)),
        "a sell once resumed, outside every window",
    );
    expect_code(
        &w.trade(&m, &a, E1, USDC as i64),
        SideMarketError::TradingClosed,
        "a buy once resumed, outside every window",
    );
    w.assert_d6(&m);
}

#[test]
fn set_sell_until_by_the_operator_from_lock_ts_while_open_or_halted() {
    let mut w = World::new();
    let m = market3(&mut w, 23);
    let a = trader(&mut w, &m, 1_000 * USDC);
    expect_ok(w.trade(&m, &a, E1, 10 * USDC as i64), "buy");
    let s = w.k.stranger.insecure_clone();
    let ix = w.set_sell_until_ix(&m, s.pubkey(), LOCK + DAY);
    expect_code(
        &w.as_key(ix, &s),
        SideMarketError::UnauthorizedOperator,
        "a stranger",
    );
    let op = w.k.operator.pubkey();
    for bad in [LOCK - 1, NOW, 1, -1, i64::MIN] {
        let ix = w.set_sell_until_ix(&m, op, bad);
        expect_code(
            &w.as_operator(ix),
            SideMarketError::InvalidWindow,
            "a sell_until before lock_ts",
        );
    }
    assert_eq!(w.market(&m).sell_until, 0);
    w.set_sell_until(&m, LOCK);
    assert_eq!(w.market(&m).sell_until, LOCK);
    w.set_sell_until(&m, i64::MAX);
    w.set_sell_until(&m, 0);
    assert_eq!(w.market(&m).sell_until, 0);
    // While halted: allowed.
    expect_ok(w.as_operator(w.halt_ix(&m, op)), "halt");
    w.set_sell_until(&m, LOCK + DAY);
    expect_ok(w.as_operator(w.resume_ix(&m, op)), "resume");
    assert_eq!(w.market(&m).sell_until, LOCK + DAY);

    // Proposed, Void, Final: refused.
    w.set_time(LOCK);
    expect_ok(w.propose(&m, E1), "propose");
    let ix = w.set_sell_until_ix(&m, op, LOCK + 2 * DAY);
    expect_code(
        &w.as_operator(ix),
        SideMarketError::InvalidStatus,
        "proposed",
    );
    let mut v = w.fork();
    let arb = v.k.arbiter.insecure_clone();
    expect_ok(v.as_key(v.void_ix(&m, arb.pubkey()), &arb), "arbiter void");
    let ix = v.set_sell_until_ix(&m, op, LOCK + 2 * DAY);
    expect_code(&v.as_operator(ix), SideMarketError::InvalidStatus, "void");
    w.set_time(LOCK + DISPUTE);
    expect_ok(w.as_operator(w.finalize_ix(&m)), "finalize");
    let ix = w.set_sell_until_ix(&m, op, LOCK + 2 * DAY);
    expect_code(&w.as_operator(ix), SideMarketError::InvalidStatus, "final");
    assert_eq!(w.market(&m).sell_until, LOCK + DAY);
}
