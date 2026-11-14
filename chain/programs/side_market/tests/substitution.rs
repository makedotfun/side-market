//! Account substitution (the Raydium V3 loss): every mint, vault, fees account, ticket, Config and
//! payee an instruction takes is re-derived and owner-checked, so passing any other account in its
//! place is refused, and the honest instruction still lands afterwards.

mod common;
mod mainnet;
mod program_so;

use {
    anchor_lang::{error::ErrorCode, prelude::Pubkey, InstructionData, ToAccountMetas},
    anchor_spl::token,
    common::*,
    side_market::{instruction as ixs, SideMarketError, FIRST_ENTRY_ID},
    solana_signer::Signer,
};

const E1: u16 = FIRST_ENTRY_ID;
const USDC: u64 = 1_000_000;

struct Two {
    w: World,
    a: Mkt,
    b: Mkt,
    t: Trader,
}

/// Two open markets, each with an entry and the bounty window open; one trader with a ticket in
/// each and a YES position in `a`.
fn two() -> Two {
    let mut w = World::new();
    let a = w.open(1, None);
    let b = w.open(2, None);
    for m in [&a, &b] {
        w.add(m, E1, 200_000);
        w.open_bounty(m);
    }
    let t = w.trader(1_000 * USDC);
    w.open_ticket(&a, t.key.pubkey());
    w.open_ticket(&b, t.key.pubkey());
    expect_ok(w.trade(&a, &t, E1, 10 * USDC as i64), "a buy in a");
    expect_ok(w.trade(&b, &t, E1, 10 * USDC as i64), "a buy in b");
    Two { w, a, b, t }
}

/// A trade on `a` with its accounts edited by `edit`, signed by the trader.
fn trade_with(
    x: &mut Two,
    delta: i64,
    edit: impl FnOnce(&mut side_market::accounts::Trade),
) -> Sent {
    let mut accounts =
        x.w.trade_accounts(&x.a, x.t.key.pubkey(), x.t.key.pubkey(), x.t.token);
    edit(&mut accounts);
    let ix = anchor_lang::solana_program::instruction::Instruction::new_with_bytes(
        side_market::ID,
        &ixs::Trade {
            outcome_id: E1,
            delta,
            limit: if delta > 0 { u64::MAX } else { 0 },
        }
        .data(),
        accounts.to_account_metas(None),
    );
    let k = x.t.key.insecure_clone();
    x.w.send(&[cu_limit(1_400_000), ix], &k, &[])
}

#[test]
fn trade_refuses_every_substituted_account() {
    let mut x = two();
    let seeds = anchor_code(ErrorCode::ConstraintSeeds);
    let (bv, bf, bm) = (x.b.vault, x.b.fees, x.b.market);

    // Another market's vault or fees account, or the fees account as the vault: by seeds.
    for (delta, what) in [(USDC as i64, "buy"), (-(USDC as i64), "sale")] {
        expect_code(&trade_with(&mut x, delta, |a| a.vault = bv), seeds, what);
        expect_code(&trade_with(&mut x, delta, |a| a.fees = bf), seeds, what);
        // The fees account named twice: Anchor's duplicate-mutable check refuses it first.
        let af = x.a.fees;
        expect_code(
            &trade_with(&mut x, delta, |a| a.vault = af),
            anchor_code(ErrorCode::ConstraintDuplicateMutableAccount),
            what,
        );
    }
    // A look-alike vault: a token account of the mint whose authority is the market.
    let fake = Pubkey::new_unique();
    let (mint, am) = (x.w.mint, x.a.market);
    x.w.put_token_account(fake, mint, am, 1_000_000 * USDC);
    expect_code(
        &trade_with(&mut x, -(USDC as i64), |a| a.vault = fake),
        seeds,
        "a look-alike vault",
    );
    // Another market (its vault and ticket stay a's): by seeds.
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.market = bm),
        seeds,
        "another market",
    );
    // b's ticket on a's market.
    let bt = ticket_pda(&bm, &x.t.key.pubkey());
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.ticket = bt),
        seeds,
        "another market's ticket",
    );
    // Another wallet's ticket.
    let other = x.w.trader(1_000 * USDC);
    let am2 = x.a;
    x.w.open_ticket(&am2, other.key.pubkey());
    let ot = ticket_pda(&x.a.market, &other.key.pubkey());
    expect_code(
        &trade_with(&mut x, -(USDC as i64), |a| a.ticket = ot),
        seeds,
        "another wallet's ticket",
    );

    // Another mint (classic, 6 dp).
    let mint2 = Pubkey::new_unique();
    x.w.put_mint(mint2, 6, token::ID);
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.mint = mint2),
        SideMarketError::InvalidMint,
        "another mint",
    );
    // A wallet token account of another mint, of another owner, or owned by Token-2022.
    let wrong_mint = Pubkey::new_unique();
    let tk = x.t.key.pubkey();
    x.w.put_token_account(wrong_mint, mint2, tk, 1_000 * USDC);
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.wallet_token = wrong_mint),
        anchor_code(ErrorCode::ConstraintTokenMint),
        "a token account of another mint",
    );
    let others = other.token;
    expect_code(
        &trade_with(&mut x, -(USDC as i64), |a| a.wallet_token = others),
        anchor_code(ErrorCode::ConstraintTokenOwner),
        "a sale into another owner's account",
    );
    let t22 = Pubkey::new_unique();
    x.w.put_token_account_owned(t22, mint, tk, 1_000 * USDC, TOKEN_2022_ID.parse().unwrap());
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.wallet_token = t22),
        anchor_code(ErrorCode::AccountOwnedByWrongProgram),
        "a Token-2022 account",
    );
    // The Token-2022 program in place of Token.
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| {
            a.token_program = TOKEN_2022_ID.parse().unwrap()
        }),
        anchor_code(ErrorCode::InvalidProgramId),
        "the Token-2022 program",
    );
    // A Config look-alike: the real Config's bytes, owned by the program, at another address.
    let mut cfg = x.w.svm.get_account(&config_pda()).unwrap();
    let fake_cfg = Pubkey::new_unique();
    x.w.svm.set_account(fake_cfg, cfg.clone()).unwrap();
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.config = fake_cfg),
        seeds,
        "a Config look-alike",
    );
    cfg.owner = Pubkey::new_unique();
    x.w.svm.set_account(fake_cfg, cfg).unwrap();
    expect_code(
        &trade_with(&mut x, USDC as i64, |a| a.config = fake_cfg),
        anchor_code(ErrorCode::AccountOwnedByWrongProgram),
        "a Config of another owner",
    );

    // The honest trade still lands, and D6 holds in both markets.
    expect_ok(
        trade_with(&mut x, -(USDC as i64), |_| {}),
        "the honest sale",
    );
    x.w.assert_d6(&x.a);
    x.w.assert_d6(&x.b);
}

#[test]
fn a_signer_cannot_trade_another_wallets_ticket() {
    let mut x = two();
    let thief = x.w.trader(1_000 * USDC);
    // The thief signs as wallet but names the victim's ticket and its own token account.
    let victim_ticket = ticket_pda(&x.a.market, &x.t.key.pubkey());
    let mut accounts =
        x.w.trade_accounts(&x.a, thief.key.pubkey(), thief.key.pubkey(), thief.token);
    accounts.ticket = victim_ticket;
    let ix = anchor_lang::solana_program::instruction::Instruction::new_with_bytes(
        side_market::ID,
        &ixs::Trade {
            outcome_id: E1,
            delta: -(5 * USDC as i64),
            limit: 0,
        }
        .data(),
        accounts.to_account_metas(None),
    );
    let k = thief.key.insecure_clone();
    expect_code(
        &x.w.send(&[ix], &k, &[]),
        anchor_code(ErrorCode::ConstraintSeeds),
        "a stolen sale",
    );
}

#[test]
fn value_moving_instructions_refuse_another_markets_vault() {
    let mut x = two();
    let seeds = anchor_code(ErrorCode::ConstraintSeeds);
    let bv = x.b.vault;
    // scratch, void, finalize.
    let mut ix = x.w.scratch_ix(&x.a, x.w.k.operator.pubkey(), E1);
    ix.accounts[3].pubkey = bv;
    expect_code(&x.w.as_operator(ix), seeds, "scratch with b's vault");
    x.w.set_time(LOCK);
    expect_ok(x.w.propose(&x.a, E1), "propose");
    x.w.set_time(LOCK + DISPUTE);
    let mut ix = x.w.finalize_ix(&x.a);
    ix.accounts[1].pubkey = bv;
    expect_code(&x.w.as_operator(ix), seeds, "finalize with b's vault");
    let arb = x.w.k.arbiter.insecure_clone();
    let mut ix = x.w.void_ix(&x.b, arb.pubkey());
    ix.accounts[3].pubkey = x.a.vault;
    expect_code(&x.w.as_key(ix, &arb), seeds, "void with a's vault");
    expect_ok(x.w.as_operator(x.w.finalize_ix(&x.a)), "finalize");

    // settle_ticket: another market's vault, another wallet, another rent payee.
    let s = x.w.k.stranger.insecure_clone();
    let wallet = x.t.key.pubkey();
    let mut acc = x.w.settle_accounts(&x.a, s.pubkey(), wallet);
    acc.vault = bv;
    let ix = ix_of(acc, ixs::SettleTicket {});
    expect_code(&x.w.as_key(ix, &s), seeds, "settle from b's vault");
    let mut acc = x.w.settle_accounts(&x.a, s.pubkey(), wallet);
    acc.ticket_payer = s.pubkey();
    let ix = ix_of(acc, ixs::SettleTicket {});
    expect_code(
        &x.w.as_key(ix, &s),
        SideMarketError::PayerMismatch,
        "rent to the caller",
    );
    let mut acc = x.w.settle_accounts(&x.a, s.pubkey(), wallet);
    acc.wallet = s.pubkey();
    acc.wallet_token =
        anchor_spl::associated_token::get_associated_token_address(&s.pubkey(), &x.w.mint);
    let ix = ix_of(acc, ixs::SettleTicket {});
    expect_code(
        &x.w.as_key(ix, &s),
        SideMarketError::TicketMismatch,
        "pay the caller",
    );
    let paid = x.w.settle(&x.a, wallet);
    assert_eq!(paid, 10 * USDC);

    // close_market: another rent payee, a non-house token account.
    let mut acc = x.w.close_accounts(&x.a, x.w.k.operator.pubkey());
    acc.rent_to = s.pubkey();
    let ix = ix_of(acc, ixs::CloseMarket {});
    expect_code(
        &x.w.as_operator(ix),
        SideMarketError::PayerMismatch,
        "close rent to a stranger",
    );
    let mut acc = x.w.close_accounts(&x.a, x.w.k.operator.pubkey());
    acc.house_token = x.t.token;
    let ix = ix_of(acc, ixs::CloseMarket {});
    expect_code(
        &x.w.as_operator(ix),
        anchor_code(ErrorCode::ConstraintTokenOwner),
        "surplus to a trader",
    );
    let mut acc = x.w.close_accounts(&x.a, x.w.k.operator.pubkey());
    acc.vault = bv;
    let ix = ix_of(acc, ixs::CloseMarket {});
    expect_code(&x.w.as_operator(ix), seeds, "close b's vault");
    expect_ok(
        x.w.as_operator(x.w.close_ix(&x.a, x.w.k.operator.pubkey())),
        "close",
    );
    // b is untouched.
    x.w.assert_d6(&x.b);
    assert!(x.w.tokens(&x.b.vault) > 0);
}

#[test]
fn open_market_refuses_a_vault_off_its_pda() {
    let mut w = World::new();
    let m = Mkt::of(cid(9));
    let mut ix = w.open_market_ix(&m, w.args());
    // vault is the 7th account (operator, house, config, mint, house_token, market, vault).
    ix.accounts[6].pubkey = Pubkey::new_unique();
    let (op, house) = (w.k.operator.insecure_clone(), w.k.house.insecure_clone());
    expect_code(
        &w.send(&[ix], &op, &[&house]),
        anchor_code(ErrorCode::ConstraintSeeds),
        "a vault off its PDA",
    );
    // The house's token account must be the house's.
    let t = w.trader(1_000 * USDC);
    let ix = w.open_market_ix_with(&m, w.args(), op.pubkey(), house.pubkey(), t.token);
    expect_code(
        &w.send(&[ix], &op, &[&house]),
        anchor_code(ErrorCode::ConstraintTokenOwner),
        "a deposit from a trader",
    );
}

fn ix_of(
    accounts: impl ToAccountMetas,
    data: impl InstructionData,
) -> anchor_lang::solana_program::instruction::Instruction {
    ix(accounts, data)
}
