//! The golden vectors (tests/vectors.json, written by scripts/side-market/vectors.ts from
//! server/side-market/math.ts) replayed through src/math.rs: every output to the bit, so math.rs and
//! its TypeScript mirror agree on every vector. tests/unit/side-market-math.test.ts replays the same
//! file through math.ts and holds every real value (server/market/lmsr.ts's oracle) inside the bounds.

use {
    serde_json::Value,
    side_market::math::{
        cost_delta_lo, fee_of, join, liquidity_for, ln_q64, loss_per_b, marks, open_book,
        price_ppm, scratch, shares_for_budget, theta_of_ppm, trade, trade_flows, weight_err, Term,
        Weighted,
    },
};

const VECTORS: &str = include_str!("vectors.json");

fn big(v: &Value) -> i128 {
    v.as_str()
        .unwrap_or_else(|| panic!("{v} is not a string"))
        .parse()
        .unwrap_or_else(|e| panic!("{v}: {e}"))
}

fn ubig(v: &Value) -> u128 {
    v.as_str()
        .unwrap_or_else(|| panic!("{v} is not a string"))
        .parse()
        .unwrap_or_else(|e| panic!("{v}: {e}"))
}

fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("{v} is not a number"))
}

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("tests/vectors.json is JSON")
}

#[test]
fn exp_ln_theta_liquidity_and_fees() {
    let v = vectors();
    for e in v["exp"].as_array().unwrap() {
        let w = side_market::math::exp_q64(big(&e["d"])).unwrap();
        assert_eq!(w, ubig(&e["w"]), "exp {}", e["d"]);
        assert_eq!(weight_err(w), ubig(&e["err"]));
    }
    for e in v["ln"].as_array().unwrap() {
        assert_eq!(
            ln_q64(ubig(&e["z"])).unwrap(),
            big(&e["ln"]),
            "ln {}",
            e["z"]
        );
    }
    for e in v["theta"].as_array().unwrap() {
        assert_eq!(
            theta_of_ppm(num(&e["massPpm"]) as u32).unwrap(),
            big(&e["theta"])
        );
    }
    for e in v["liquidity"].as_array().unwrap() {
        let m = num(&e["mFloorPpm"]) as u32;
        let (b, deposit) = liquidity_for(ubig(&e["seed"]) as u64, m).unwrap();
        assert_eq!(u128::from(b), ubig(&e["b"]));
        assert_eq!(u128::from(deposit), ubig(&e["deposit"]));
        assert_eq!(loss_per_b(m).unwrap(), big(&e["lossPerB"]));
    }
    for e in v["fees"].as_array().unwrap() {
        let fee = fee_of(
            num(&e["feeRatePpm"]) as u32,
            ubig(&e["deltaAbs"]) as u64,
            ubig(&e["dcAbs"]),
        )
        .unwrap();
        assert_eq!(u128::from(fee), ubig(&e["fee"]));
    }
}

#[test]
fn scenarios_replay_to_the_bit() {
    let v = vectors();
    let scenarios = v["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 4);
    for sc in scenarios {
        let name = sc["name"].as_str().unwrap();
        let none = num(&sc["nonePpm"]) as u32;
        let (b, deposit) =
            liquidity_for(ubig(&sc["seed"]) as u64, num(&sc["mFloorPpm"]) as u32).unwrap();
        assert_eq!(u128::from(b), ubig(&sc["b"]), "{name}");
        assert_eq!(u128::from(deposit), ubig(&sc["deposit"]), "{name}");
        let mut terms = vec![
            Term::new(theta_of_ppm(none).unwrap()),
            Term::new(theta_of_ppm(1_000_000 - none).unwrap()),
        ];
        let (mut base, mut sum) = (0i128, 0u128);
        for (k, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let e = &st["expect"];
            let at = format!("{name} step {k} ({})", st["op"]);
            let index = st["index"].as_u64().map(|i| i as usize);
            match st["op"].as_str().unwrap() {
                "open" => {
                    (base, sum) = open_book(&mut terms, b).unwrap();
                    let thetas: Vec<i128> =
                        e["thetas"].as_array().unwrap().iter().map(big).collect();
                    assert_eq!(
                        terms.iter().map(|t| t.theta).collect::<Vec<_>>(),
                        thetas,
                        "{at}"
                    );
                }
                "join" => {
                    terms.push(Term::new(0));
                    let n = terms.len() - 1;
                    (base, sum) =
                        join(&mut terms, 1, n, num(&st["massPpm"]) as u32, base, sum, b).unwrap();
                    assert_eq!(terms[n].theta, big(&e["theta"]), "{at}");
                    assert_eq!(terms[1].theta, big(&e["fieldTheta"]), "{at}");
                }
                "trade" => {
                    let i = index.unwrap();
                    let delta = big(&st["delta"]) as i64;
                    let n = side_market::math::book_len(&terms);
                    let (b1, s1, dc) = trade(&mut terms, i, delta, base, sum, b).unwrap();
                    assert_eq!(dc, big(&e["dc"]), "{at}");
                    assert_eq!(
                        cost_delta_lo(b, base, sum, b1, s1, n).unwrap(),
                        big(&e["dcLo"]),
                        "{at}"
                    );
                    let f = trade_flows(
                        big(&st["position"]) as i64,
                        delta,
                        dc,
                        num(&st["feeRatePpm"]) as u32,
                    )
                    .unwrap();
                    let fe = &e["flows"];
                    assert_eq!(f.buy, fe["buy"].as_bool().unwrap(), "{at}");
                    assert_eq!(i128::from(f.position), big(&fe["position"]), "{at}");
                    assert_eq!(i128::from(f.dn), big(&fe["dn"]), "{at}");
                    assert_eq!(f.cash, big(&fe["cash"]), "{at}");
                    assert_eq!(u128::from(f.pay), ubig(&fe["pay"]), "{at}");
                    assert_eq!(u128::from(f.receive), ubig(&fe["receive"]), "{at}");
                    assert_eq!(u128::from(f.fee), ubig(&fe["fee"]), "{at}");
                    (base, sum) = (b1, s1);
                }
                "marks" => {
                    let t = &terms[index.unwrap()];
                    let n = side_market::math::book_len(&terms);
                    let (yes, no) = marks(t.w(), sum, n).unwrap();
                    assert_eq!(u128::from(yes), ubig(&e["yes"]), "{at}");
                    assert_eq!(u128::from(no), ubig(&e["no"]), "{at}");
                    assert_eq!(
                        u64::from(price_ppm(t.w(), sum).unwrap()),
                        num(&e["pricePpm"]),
                        "{at}"
                    );
                }
                "shares" => {
                    let s = shares_for_budget(
                        &terms,
                        index.unwrap(),
                        st["yes"].as_bool().unwrap(),
                        big(&st["position"]) as i64,
                        ubig(&st["budget"]) as u64,
                        num(&st["feeRatePpm"]) as u32,
                        base,
                        sum,
                        b,
                    )
                    .unwrap();
                    assert_eq!(u128::from(s), ubig(&e["shares"]), "{at}");
                }
                "scratch" => {
                    let s = scratch(&mut terms, index.unwrap(), 1, base, sum, b).unwrap();
                    assert_eq!(s.base, big(&e["base"]), "{at}");
                    assert_eq!(s.sum, ubig(&e["sum"]), "{at}");
                    assert_eq!(s.r_lo, big(&e["rLo"]), "{at}");
                    assert_eq!(s.r_hi, big(&e["rHi"]), "{at}");
                    assert_eq!(u128::from(s.yes), ubig(&e["yes"]), "{at}");
                    assert_eq!(u128::from(s.no), ubig(&e["no"]), "{at}");
                    assert_eq!(s.field_theta, big(&e["fieldTheta"]), "{at}");
                    (base, sum) = (s.base, s.sum);
                }
                other => panic!("{at}: unknown op {other}"),
            }
            if !e["base"].is_null() {
                assert_eq!(base, big(&e["base"]), "{at} base");
                assert_eq!(sum, ubig(&e["sum"]), "{at} sum");
            }
        }
        let fin = &sc["final"];
        assert_eq!(base, big(&fin["base"]), "{name} final base");
        assert_eq!(sum, ubig(&fin["sum"]), "{name} final sum");
        let want = fin["terms"].as_array().unwrap();
        assert_eq!(terms.len(), want.len(), "{name}");
        for (t, w) in terms.iter().zip(want) {
            assert_eq!(t.theta, big(&w["theta"]), "{name}");
            assert_eq!(i128::from(t.q), big(&w["q"]), "{name}");
            assert_eq!(t.w, ubig(&w["w"]), "{name}");
            assert_eq!(t.in_book, w["inBook"].as_bool().unwrap(), "{name}");
        }
    }
}
