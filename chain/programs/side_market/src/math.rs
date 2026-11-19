//! The market maker's arithmetic (Design D2-D5, D7, D11, D13): a mass-weighted LMSR in Q64.64
//! fixed point, written here with no crate beyond core. server/side-market/math.ts is its
//! bit-exact TypeScript mirror (every function below has a camelCase twin there), and
//! tests/vectors.json, written by scripts/side-market/vectors.ts, pins both to the same outputs
//! and to server/market/lmsr.ts (a 10^-36 BigInt LMSR) as the oracle: lo <= oracle <= hi.
//!
//! The model. Each outcome j in the book has a theta_j (ln of its mass, stored as Q64.64) and a
//! net quantity q_j (YES minus NO shares, base units). With b the liquidity,
//!
//!   x_j = theta_j + floor(q_j * 2^64 / b) / 2^64        (x_of: the model's exponent, exact Q64.64)
//!   C(q) = b ln sum_j e^(x_j)                           (the cost function)
//!   p_j  = e^(x_j) / sum_k e^(x_k)                       (the price)
//!
//! The market caches w_j = exp_q64(x_j - base) (u128, 64 fractional bits) against a reference
//! `base` and keeps sum = sum_j w_j exactly as an integer. A trade on i recomputes w_i alone; the
//! whole book is rebased (base = max_j x_j, every weight recomputed) when the traded exponent
//! rises above base + REBASE_SPAN (w would pass e^16) or the sum falls below SUM_FLOOR (every
//! exponent below about base - 16, where the cached weights lose relative precision).
//!
//! Every cached weight carries a bound: |w - e^(x - base) * 2^64| <= weight_err(w) =
//! (w >> 56) + 2 (exp_q64's own error is under 2^-57 relative plus one ulp; the tests measure it
//! against lmsr.ts). So the true sum lies in [sum - sum_err, sum + sum_err], sum_err = (sum >> 56)
//! + 2n, and ln_q64 is within LN_ERR ulps. A trade's cost difference C(q') - C(q) =
//! b ((base' - base) + ln sum' - ln sum) is bracketed by cost_delta_lo and cost_delta_hi, and the
//! program charges the high bound rounded up plus one base unit: pays round up, receipts round
//! down, so the vault never holds less than the true LMSR says. The worst-case house loss stays
//! b ln(1/m_floor), which the house deposit S = ceil(b L) + 1,000 covers (liquidity_for).
//!
//! Exponentials: x = k ln2 + r with |r| <= ln2/2, then 16 Taylor terms with precomputed
//! 2^64/j! (no division), then a shift by k. Logarithms: z = 2^k y with y in [1/sqrt2, sqrt2),
//! ln y = 2 atanh(s), s = (y - 1)/(y + 1), 12 terms with precomputed 2^64/(2j+1).
//!
//! Integer semantics the mirror reproduces: `>>` on a signed value floors (arithmetic shift, as
//! BigInt's `>>`), `/` truncates toward zero (as BigInt's `/`), and every bound is checked, never
//! wrapped: a value out of range is a MathError, which the program returns as an error.

/// 1.0 in Q64.64.
pub const ONE: u128 = 1 << 64;
const ONE_I: i128 = 1 << 64;

/// round(ln 2 * 2^64).
pub const LN2: i128 = 12_786_308_645_202_655_660;
/// floor(sqrt 2 * 2^64): ln_q64 reduces its argument to [SQRT2 / 2, SQRT2).
pub const SQRT2: u128 = 26_087_635_650_665_564_424;

/// The Taylor terms exp_q64 sums: floor(2^64 / j!) for j = 1..=16.
pub const INV_FACT: [i128; 16] = [
    18_446_744_073_709_551_616,
    9_223_372_036_854_775_808,
    3_074_457_345_618_258_602,
    768_614_336_404_564_650,
    153_722_867_280_912_930,
    25_620_477_880_152_155,
    3_660_068_268_593_165,
    457_508_533_574_145,
    50_834_281_508_238,
    5_083_428_150_823,
    462_129_831_893,
    38_510_819_324,
    2_962_370_717,
    211_597_908,
    14_106_527,
    881_657,
];

/// The atanh terms ln_q64 sums: floor(2^64 / (2j + 1)) for j = 0..12.
pub const INV_ODD: [i128; 12] = [
    18_446_744_073_709_551_616,
    6_148_914_691_236_517_205,
    3_689_348_814_741_910_323,
    2_635_249_153_387_078_802,
    2_049_638_230_412_172_401,
    1_676_976_733_973_595_601,
    1_418_980_313_362_273_201,
    1_229_782_938_247_303_441,
    1_085_102_592_571_150_095,
    970_881_267_037_344_821,
    878_416_384_462_359_600,
    802_032_351_030_850_070,
];

/// exp_q64 refuses an argument above 40 (e^40 * 2^64 < 2^122): no book path passes one above
/// REBASE_SPAN, so this is a guard, not a limit anyone meets.
pub const EXP_MAX_ARG: i128 = 40 << 64;
/// exp_q64 is 0 below -46: e^-46 * 2^64 is under 2^-2 ulp.
pub const EXP_MIN_ARG: i128 = -(46 << 64);
/// A cached weight's exponent x - base stays at or below 16: past it the book is rebased.
pub const REBASE_SPAN: i128 = 16 << 64;
/// The book is rebased when its sum falls below 2^41 (about e^-15.9 in Q64.64).
pub const SUM_FLOOR: u128 = 1 << 41;
/// weight_err's relative part: w >> 56 (2^-56 relative, past exp_q64's measured 2^-57).
pub const W_REL_SHIFT: u32 = 56;
/// weight_err's absolute part, in ulps (2^-64).
pub const W_ABS_ERR: u128 = 2;
/// ln_q64's error bound, in ulps: its measured error is under 100.
pub const LN_ERR: i128 = 256;
/// 1,000,000: masses, prices, floors and fee rates are integer ppm.
pub const PPM: u64 = 1_000_000;
/// The smallest liquidity b, in base units: x_of stays far inside i128 for every i64 quantity.
pub const MIN_B: u64 = 1_000;
/// The largest liquidity b, in base units (2^48): b times any cost exponent stays inside i128.
pub const MAX_B: u64 = 1 << 48;
/// What the house deposits beyond its worst-case loss ceil(b L): D2's 1,000 base units.
pub const SEED_CUSHION: u64 = 1_000;

/// What a computation refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathError {
    /// A value left the range the fixed point is proved for.
    Overflow,
    /// An argument outside the function's domain (ln of 0, an empty book, a mass out of range).
    Domain,
    /// A trade of 0 shares.
    ZeroDelta,
    /// A trade that would take a position across zero (a flip is two trades).
    CrossesZero,
    /// A buy that would cost less than one base unit.
    BuyTooSmall,
}

pub type MathResult<T> = core::result::Result<T, MathError>;

fn ov<T>(v: Option<T>) -> MathResult<T> {
    v.ok_or(MathError::Overflow)
}

// ------------------------------------------------------------------------------ exp and ln

/// e^(d / 2^64) * 2^64 for a Q64.64 `d` <= EXP_MAX_ARG; 0 below EXP_MIN_ARG. d = k ln2 + r
/// (k = d / LN2 rounded half away from zero), e^r by 16 Taylor terms, then a shift by k (a right
/// shift floors). Within (w >> 57) + 1 of the real value.
pub fn exp_q64(d: i128) -> MathResult<u128> {
    if d > EXP_MAX_ARG {
        return Err(MathError::Overflow);
    }
    if d < EXP_MIN_ARG {
        return Ok(0);
    }
    let half = LN2 / 2;
    let k = if d >= 0 {
        (d + half) / LN2
    } else {
        -((-d + half) / LN2)
    };
    let r = d - k * LN2;
    let mut sum: i128 = ONE_I;
    let mut pow: i128 = ONE_I;
    for inv in INV_FACT {
        pow = (pow * r) >> 64;
        sum += (pow * inv) >> 64;
    }
    // e^r is in [0.70, 1.42], so sum is positive and below 2^65.
    let sum = sum as u128;
    if k >= 0 {
        Ok(sum << (k as u32))
    } else if -k >= 128 {
        Ok(0)
    } else {
        Ok(sum >> ((-k) as u32))
    }
}

/// ln(z / 2^64) as Q64.64 for z > 0: z = 2^k y with y in [SQRT2 / 2, SQRT2), ln y = 2 atanh(s),
/// s = (y - 1) / (y + 1), 12 terms. Within LN_ERR of the real value.
pub fn ln_q64(z: u128) -> MathResult<i128> {
    if z == 0 {
        return Err(MathError::Domain);
    }
    let bits = 128 - z.leading_zeros() as i32;
    let mut k = bits - 65;
    let mut y = shift_down(z, k);
    if y >= SQRT2 {
        k += 1;
        y = shift_down(z, k);
    }
    let y = y as i128;
    let s = ((y - ONE_I) << 64) / (y + ONE_I);
    let s2 = (s * s) >> 64;
    let mut pow = s;
    let mut acc: i128 = 0;
    for inv in INV_ODD {
        acc += (pow * inv) >> 64;
        pow = (pow * s2) >> 64;
    }
    Ok(2 * acc + (k as i128) * LN2)
}

/// z * 2^-k (floor) for k >= 0, z * 2^-k exactly for k < 0. Only called with |k| < 128 and a
/// result below 2^66.
fn shift_down(z: u128, k: i32) -> u128 {
    if k >= 0 {
        z >> (k as u32)
    } else {
        z << ((-k) as u32)
    }
}

/// The bound on a cached weight's error, in ulps: (w >> 56) + 2.
pub fn weight_err(w: u128) -> u128 {
    (w >> W_REL_SHIFT) + W_ABS_ERR
}

/// The bound on a sum of `n` cached weights' error: (sum >> 56) + 2n (the sum of the floors is at
/// most the floor of the sum).
pub fn sum_err(sum: u128, n: u32) -> u128 {
    (sum >> W_REL_SHIFT) + W_ABS_ERR * u128::from(n)
}

// ------------------------------------------------------------------------------ masses and b

/// theta for a mass of `mass_ppm` (1 to 1,000,000): ln(floor(mass * 2^64 / 1e6)) - LN_ERR, at or
/// below the real ln(mass / 1e6), so a book's sum of e^theta never exceeds its masses' sum.
pub fn theta_of_ppm(mass_ppm: u32) -> MathResult<i128> {
    if mass_ppm == 0 || u64::from(mass_ppm) > PPM {
        return Err(MathError::Domain);
    }
    let z = (u128::from(mass_ppm) << 64) / u128::from(PPM);
    Ok(ln_q64(z)? - LN_ERR)
}

/// L, the house's loss per unit of b: LN_ERR - ln(floor(m_floor * 2^64 / 1e6)), at or above the
/// real ln(1/m_floor) (Q64.64, positive for m_floor below 1).
pub fn loss_per_b(m_floor_ppm: u32) -> MathResult<i128> {
    if m_floor_ppm == 0 || u64::from(m_floor_ppm) >= PPM {
        return Err(MathError::Domain);
    }
    let z = (u128::from(m_floor_ppm) << 64) / u128::from(PPM);
    Ok(LN_ERR - ln_q64(z)?)
}

/// D2: b = floor(seed / L) and the house deposit S = ceil(b L) + SEED_CUSHION, L = loss_per_b.
/// b must land in [MIN_B, MAX_B]. Returns (b, S).
pub fn liquidity_for(seed: u64, m_floor_ppm: u32) -> MathResult<(u64, u64)> {
    let l = loss_per_b(m_floor_ppm)?;
    if l <= 0 {
        return Err(MathError::Domain);
    }
    let b = (u128::from(seed) << 64) / (l as u128);
    if b < u128::from(MIN_B) || b > u128::from(MAX_B) {
        return Err(MathError::Domain);
    }
    let b = b as u64;
    let deposit = ov(max_loss(b, l)?.checked_add(SEED_CUSHION))?;
    Ok((b, deposit))
}

/// ceil(b L / 2^64): the most the house can lose at liquidity b (L = loss_per_b).
pub fn max_loss(b: u64, l: i128) -> MathResult<u64> {
    let v = ov(i128::from(b).checked_mul(l))?;
    let loss = ceil_shr64(v);
    u64::try_from(loss).map_err(|_| MathError::Overflow)
}

/// The model's exponent x = theta + floor(q * 2^64 / b), exact Q64.64.
pub fn x_of(theta: i128, q: i64, b: u64) -> MathResult<i128> {
    if b == 0 {
        return Err(MathError::Domain);
    }
    let scaled = (i128::from(q) << 64).div_euclid(i128::from(b));
    ov(theta.checked_add(scaled))
}

/// ceil(v / 2^64) for any i128 v.
pub fn ceil_shr64(v: i128) -> i128 {
    -((-v) >> 64)
}

// ------------------------------------------------------------------------------ the book

/// One outcome as the book sees it: its theta, its net quantity, its cached weight, and whether
/// it is in the book (a scratched outcome is not: its weight is 0 and nothing prices it). The
/// program's state::Outcome implements it; Term is the plain one the tests and vectors use.
pub trait Weighted {
    fn theta(&self) -> i128;
    fn set_theta(&mut self, theta: i128);
    fn q(&self) -> i64;
    fn set_q(&mut self, q: i64);
    fn w(&self) -> u128;
    fn set_w(&mut self, w: u128);
    fn in_book(&self) -> bool;
    /// Takes it out of the book for good (a scratch).
    fn leave_book(&mut self);
}

/// The plain Weighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Term {
    pub theta: i128,
    pub q: i64,
    pub w: u128,
    pub in_book: bool,
}

impl Term {
    pub fn new(theta: i128) -> Self {
        Term {
            theta,
            q: 0,
            w: 0,
            in_book: true,
        }
    }
}

impl Weighted for Term {
    fn theta(&self) -> i128 {
        self.theta
    }
    fn set_theta(&mut self, theta: i128) {
        self.theta = theta;
    }
    fn q(&self) -> i64 {
        self.q
    }
    fn set_q(&mut self, q: i64) {
        self.q = q;
    }
    fn w(&self) -> u128 {
        self.w
    }
    fn set_w(&mut self, w: u128) {
        self.w = w;
    }
    fn in_book(&self) -> bool {
        self.in_book
    }
    fn leave_book(&mut self) {
        self.in_book = false;
    }
}

/// How many terms are in the book.
pub fn book_len<T: Weighted>(terms: &[T]) -> u32 {
    terms.iter().filter(|t| t.in_book()).count() as u32
}

/// A full rebase: base = max x over the book, every weight exp_q64(x - base) (0 out of the
/// book). Returns (base, sum); the sum is at least ONE (the max term is exactly ONE).
pub fn rebase<T: Weighted>(terms: &mut [T], b: u64) -> MathResult<(i128, u128)> {
    let mut xs: Vec<i128> = Vec::with_capacity(terms.len());
    let mut base: Option<i128> = None;
    for t in terms.iter() {
        if t.in_book() {
            let x = x_of(t.theta(), t.q(), b)?;
            base = Some(base.map_or(x, |m: i128| m.max(x)));
            xs.push(x);
        } else {
            xs.push(0);
        }
    }
    let base = base.ok_or(MathError::Domain)?;
    let mut sum: u128 = 0;
    for (t, x) in terms.iter_mut().zip(xs) {
        if t.in_book() {
            let w = exp_q64(ov(x.checked_sub(base))?)?;
            t.set_w(w);
            sum = ov(sum.checked_add(w))?;
        } else {
            t.set_w(0);
        }
    }
    Ok((base, sum))
}

/// The book after the terms at `changed` moved (their theta, q or place in the book; their cached
/// w still the old one): each is reweighed against `base`, unless one would pass base +
/// REBASE_SPAN or the new sum falls below SUM_FLOOR, when the whole book is rebased. Returns the
/// new (base, sum).
pub fn refresh<T: Weighted>(
    terms: &mut [T],
    changed: &[usize],
    base: i128,
    sum: u128,
    b: u64,
) -> MathResult<(i128, u128)> {
    let mut s = sum;
    for &i in changed {
        let t = terms.get_mut(i).ok_or(MathError::Domain)?;
        s = ov(s.checked_sub(t.w()))?;
        if t.in_book() {
            let d = ov(x_of(t.theta(), t.q(), b)?.checked_sub(base))?;
            if d > REBASE_SPAN {
                return rebase(terms, b);
            }
            let w = exp_q64(d)?;
            t.set_w(w);
            s = ov(s.checked_add(w))?;
        } else {
            t.set_w(0);
        }
    }
    if s < SUM_FLOOR {
        return rebase(terms, b);
    }
    Ok((base, s))
}

/// An upper bound on C(q1) - C(q0) in Q64.64 base units (unrounded), from two frames (base0,
/// sum0) and (base1, sum1) of an n-term book: b ((base1 - base0) + ln(sum1 + err) + LN_ERR -
/// ln(sum0 - err) + LN_ERR). The bound is a few times b * LN_ERR ulps above the truth (about
/// 2^-30 base units at b = 2^23).
pub fn cost_delta_hi_q64(
    b: u64,
    base0: i128,
    sum0: u128,
    base1: i128,
    sum1: u128,
    n: u32,
) -> MathResult<i128> {
    let s0 = ov(sum0.checked_sub(sum_err(sum0, n)))?;
    let s1 = ov(sum1.checked_add(sum_err(sum1, n)))?;
    if s0 == 0 {
        return Err(MathError::Domain);
    }
    let dl = ov(base1.checked_sub(base0))?;
    let dl = ov(dl.checked_add(ln_q64(s1)? - ln_q64(s0)? + 2 * LN_ERR))?;
    ov(i128::from(b).checked_mul(dl))
}

/// A lower bound on C(q1) - C(q0) in Q64.64 base units (unrounded): the mirror of
/// cost_delta_hi_q64.
pub fn cost_delta_lo_q64(
    b: u64,
    base0: i128,
    sum0: u128,
    base1: i128,
    sum1: u128,
    n: u32,
) -> MathResult<i128> {
    let s0 = ov(sum0.checked_add(sum_err(sum0, n)))?;
    let s1 = ov(sum1.checked_sub(sum_err(sum1, n)))?;
    if s1 == 0 {
        return Err(MathError::Domain);
    }
    let dl = ov(base1.checked_sub(base0))?;
    let dl = ov(dl.checked_add(ln_q64(s1)? - ln_q64(s0)? - 2 * LN_ERR))?;
    ov(i128::from(b).checked_mul(dl))
}

/// An upper bound on C(q1) - C(q0), in whole base units: cost_delta_hi_q64 rounded up, plus 1.
pub fn cost_delta_hi(
    b: u64,
    base0: i128,
    sum0: u128,
    base1: i128,
    sum1: u128,
    n: u32,
) -> MathResult<i128> {
    let v = cost_delta_hi_q64(b, base0, sum0, base1, sum1, n)?;
    ov(ceil_shr64(v).checked_add(1))
}

/// A lower bound on C(q1) - C(q0), in whole base units: cost_delta_lo_q64 rounded down, minus 1.
pub fn cost_delta_lo(
    b: u64,
    base0: i128,
    sum0: u128,
    base1: i128,
    sum1: u128,
    n: u32,
) -> MathResult<i128> {
    let v = cost_delta_lo_q64(b, base0, sum0, base1, sum1, n)?;
    ov((v >> 64).checked_sub(1))
}

/// A new book's frame: every term's theta set, q 0. Returns (base, sum).
pub fn open_book<T: Weighted>(terms: &mut [T], b: u64) -> MathResult<(i128, u128)> {
    rebase(terms, b)
}

/// D3 join: the new term at `new_index` (in the book, q 0, w 0) takes `mass_ppm` from FIELD (at
/// `field`). Its theta is theta_of_ppm(mass_ppm); FIELD's becomes ln(lo(e^theta_F) -
/// hi(e^theta_new)) - LN_ERR, so the two together never weigh more than FIELD did and C never
/// rises. The masses' ppm bookkeeping (and its floors) is the caller's. Returns (base, sum).
pub fn join<T: Weighted>(
    terms: &mut [T],
    field: usize,
    new_index: usize,
    mass_ppm: u32,
    base: i128,
    sum: u128,
    b: u64,
) -> MathResult<(i128, u128)> {
    let theta_new = theta_of_ppm(mass_ppm)?;
    let theta_field = terms.get(field).ok_or(MathError::Domain)?.theta();
    let ef = exp_q64(theta_field)?;
    let en = exp_q64(theta_new)?;
    let lo_field = ef.saturating_sub(weight_err(ef));
    let hi_new = ov(en.checked_add(weight_err(en)))?;
    let rest = lo_field.checked_sub(hi_new).filter(|r| *r > 0);
    let rest = rest.ok_or(MathError::Domain)?;
    let theta_field = ln_q64(rest)? - LN_ERR;
    terms[field].set_theta(theta_field);
    let t = terms.get_mut(new_index).ok_or(MathError::Domain)?;
    t.set_theta(theta_new);
    refresh(terms, &[field, new_index], base, sum, b)
}

/// A trade: q_i += delta. Returns (base', sum', dc), dc an upper bound on C(q') - C(q) in base
/// units (cost_delta_hi). The terms hold the new state.
pub fn trade<T: Weighted>(
    terms: &mut [T],
    i: usize,
    delta: i64,
    base: i128,
    sum: u128,
    b: u64,
) -> MathResult<(i128, u128, i128)> {
    let n = book_len(terms);
    let t = terms.get_mut(i).ok_or(MathError::Domain)?;
    if !t.in_book() {
        return Err(MathError::Domain);
    }
    let q = ov(t.q().checked_add(delta))?;
    t.set_q(q);
    let (base1, sum1) = refresh(terms, &[i], base, sum, b)?;
    let dc = cost_delta_hi(b, base, sum, base1, sum1, n)?;
    Ok((base1, sum1, dc))
}

/// What a trade moves, once its cost difference dc is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flows {
    /// |position| grows: a buy.
    pub buy: bool,
    /// The trader's position after it.
    pub position: i64,
    /// The change in the trader's NO shares on this outcome (and in the market's N).
    pub dn: i64,
    /// dc + dn: what the trader pays (positive) or is owed (negative), before the fee.
    pub cash: i128,
    /// A buy's payment into the vault (cash, at least 1); 0 on a sell.
    pub pay: u64,
    /// A sell's receipt out of the vault, before the fee (max(0, -cash): a positive cash is
    /// clamped to 0); 0 on a buy.
    pub receive: u64,
    /// The fee, to the fees account: on a buy paid beside `pay`, on a sell out of `receive`.
    pub fee: u64,
}

/// NO shares of a signed position: max(0, -s). i64::MIN (2^63 NO shares, past i64) is an
/// Overflow, never a panic.
pub fn no_part(s: i64) -> MathResult<i64> {
    if s < 0 {
        ov(s.checked_neg())
    } else {
        Ok(0)
    }
}

/// D4/D5: the flows of a trade of `delta` on a position `position`, with dc its cost difference
/// (cost_delta_hi) and `fee_rate_ppm` the market's fee. Refuses a zero delta, a position that
/// would cross zero and a buy below one base unit.
pub fn trade_flows(position: i64, delta: i64, dc: i128, fee_rate_ppm: u32) -> MathResult<Flows> {
    if delta == 0 {
        return Err(MathError::ZeroDelta);
    }
    let after = ov(position.checked_add(delta))?;
    if (position > 0 && after < 0) || (position < 0 && after > 0) {
        return Err(MathError::CrossesZero);
    }
    let buy = after.unsigned_abs() > position.unsigned_abs();
    // Both parts are in [0, i64::MAX], so their difference fits.
    let dn = no_part(after)? - no_part(position)?;
    let cash = ov(dc.checked_add(i128::from(dn)))?;
    let fee = fee_of(fee_rate_ppm, delta.unsigned_abs(), dc.unsigned_abs())?;
    if buy {
        if cash < 1 {
            return Err(MathError::BuyTooSmall);
        }
        let pay = u64::try_from(cash).map_err(|_| MathError::Overflow)?;
        Ok(Flows {
            buy,
            position: after,
            dn,
            cash,
            pay,
            receive: 0,
            fee,
        })
    } else {
        let receive = if cash < 0 {
            u64::try_from(-cash).map_err(|_| MathError::Overflow)?
        } else {
            0
        };
        Ok(Flows {
            buy,
            position: after,
            dn,
            cash,
            pay: 0,
            receive,
            fee: fee.min(receive),
        })
    }
}

/// D5: ceil(rate * ceil(a (d - a) / d) / 1e6) with d = |delta| and a = min(|dc|, d): rate *
/// |delta| * pbar (1 - pbar), pbar = |dc| / |delta|, in two ceilings.
pub fn fee_of(fee_rate_ppm: u32, delta_abs: u64, dc_abs: u128) -> MathResult<u64> {
    if fee_rate_ppm == 0 || delta_abs == 0 {
        return Ok(0);
    }
    let d = u128::from(delta_abs);
    let a = dc_abs.min(d);
    let t = ov(a.checked_mul(d - a))?.div_ceil(d);
    let f = ov(t.checked_mul(u128::from(fee_rate_ppm)))?.div_ceil(u128::from(PPM));
    u64::try_from(f).map_err(|_| MathError::Overflow)
}

// ------------------------------------------------------------------------------ fractions

/// floor(num * 2^64 / den) for num <= den, den in (0, 2^96): exact, by one u128 division when
/// den < 2^64 and otherwise by long division in two 32-bit digits (num * 2^32 and the first
/// remainder * 2^32 both stay below 2^128). Returns (quotient, whether a remainder was left).
pub fn frac_q64(num: u128, den: u128) -> MathResult<(u128, bool)> {
    if den == 0 || num > den || den >= 1 << 96 {
        return Err(MathError::Domain);
    }
    if num == den {
        return Ok((ONE, false));
    }
    if den < ONE {
        let scaled = num << 64;
        return Ok((scaled / den, scaled % den != 0));
    }
    let a = num << 32;
    let (q_hi, r) = (a / den, a % den);
    let b = r << 32;
    let (q_lo, r) = (b / den, b % den);
    Ok(((q_hi << 32) | q_lo, r != 0))
}

/// ceil(num * 2^64 / den), num <= den.
pub fn frac_q64_ceil(num: u128, den: u128) -> MathResult<u128> {
    let (q, rem) = frac_q64(num, den)?;
    Ok(if rem { q + 1 } else { q })
}

/// The price in ppm, floor(w * 1e6 / sum), for display: the quotes and the indexer.
pub fn price_ppm(w: u128, sum: u128) -> MathResult<u32> {
    if sum == 0 {
        return Err(MathError::Domain);
    }
    let p = ov(w.checked_mul(u128::from(PPM)))? / sum;
    u32::try_from(p.min(u128::from(PPM))).map_err(|_| MathError::Overflow)
}

/// D11 marks for a weight w of an n-term book with sum `sum`, as Q0.64 values a share is paid:
/// (yes, no) with yes <= p * 2^64 and no <= (1 - p) * 2^64 for the true price p, from the weight's
/// and the sum's error bounds.
pub fn marks(w: u128, sum: u128, n: u32) -> MathResult<(u64, u64)> {
    let err = sum_err(sum, n);
    let s_hi = ov(sum.checked_add(err))?;
    let s_lo = sum.checked_sub(err).filter(|s| *s > 0);
    let s_lo = s_lo.ok_or(MathError::Domain)?;
    let lo = w.saturating_sub(weight_err(w));
    let hi = ov(w.checked_add(weight_err(w)))?.min(s_lo);
    let (yes, _) = frac_q64(lo, s_hi)?;
    let p_hi = frac_q64_ceil(hi, s_lo)?;
    Ok((value_q64(yes), value_q64(ONE - p_hi)))
}

/// A Q0.64 value in [0, ONE] as the u64 value_of reads: ONE is u64::MAX (VALUE_ONE), and a value
/// below ONE is held to u64::MAX - 1, so it is never read as ONE.
pub fn value_q64(v: u128) -> u64 {
    if v >= ONE {
        VALUE_ONE
    } else {
        u64::try_from(v).unwrap_or(u64::MAX).min(VALUE_ONE - 1)
    }
}

/// The u64 Q0.64 value that stands for ONE: value_of pays every share 1 base unit at it.
pub const VALUE_ONE: u64 = u64::MAX;

/// What `shares` are paid at a Q0.64 value: floor(shares * value / 2^64), and `shares` itself at
/// VALUE_ONE (a u64 cannot hold 2^64, so u64::MAX stands for ONE).
pub fn value_of(shares: u64, value: u64) -> u64 {
    if value == VALUE_ONE {
        return shares;
    }
    ((u128::from(shares) * u128::from(value)) >> 64) as u64
}

/// D7: what a YES share on a scratched entry is paid: nothing (the entry cannot win).
pub const SCRATCH_YES: u64 = 0;
/// D7: what a NO share on a scratched entry is paid: 1 base unit (VALUE_ONE).
pub const SCRATCH_NO: u64 = VALUE_ONE;

/// What a scratch did to the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scratch {
    pub base: i128,
    pub sum: u128,
    /// The scratched outcome's YES and NO values (Q0.64): SCRATCH_YES and SCRATCH_NO.
    pub yes: u64,
    pub no: u64,
    /// FIELD's new theta.
    pub field_theta: i128,
}

/// D7: a scratched entry loses. Its YES shares are paid SCRATCH_YES (0) and its NO shares
/// SCRATCH_NO (1 each), fixed by side and share count alone: no trade, on this outcome or
/// another, before or after, moves what a position on it is paid. A ticket holding Y YES and N
/// NO shares on it is owed N, so the scratch owes N_k, exactly what it takes off N, and the
/// max-q term loses k: D6's liability never rises. The term at k leaves the book and FIELD (at
/// `field`, never traded, so its exponent is its theta) absorbs it by the sign of k's q:
///
/// - q_k >= 0: k's base mass. FIELD's theta becomes ln(lo(e^theta_F) + lo(e^theta_k)) - LN_ERR:
///   the book's cost falls by R >= 0, which the house keeps, and the other prices rise.
/// - q_k < 0: k's current weight. FIELD's theta becomes ln(lo(e^theta_F) + lo(e^x_k)) - LN_ERR
///   with x_k = x_of(theta_k, q_k, b), the exponent the book prices k at: the same weight the
///   book held for the two, so the sum, C and every other price stay put (within the rounding
///   that keeps C from rising). Paying k's NO holders 1 each while its base mass merged would
///   raise C by |R| and spend that much of the house's guarantee, and D6 would then refuse later
///   buys on a low-mass outcome.
///
/// Either way FIELD never weighs more than it and k did together. Both are computed from the
/// absolute exponents (the frame base = 0), not the cached weights: x_F and x_k are at most 0,
/// so neither can overflow, and FIELD's weight (at least about m_floor) keeps its full precision
/// however far the book's base has run. scratch_mass_ppm is what FIELD's mass_ppm takes. k's q is
/// left as it was (it is out of the book, so nothing prices it).
pub fn scratch<T: Weighted>(
    terms: &mut [T],
    k: usize,
    field: usize,
    base: i128,
    sum: u128,
    b: u64,
) -> MathResult<Scratch> {
    if k == field {
        return Err(MathError::Domain);
    }
    let x_k = {
        let t = terms.get(k).ok_or(MathError::Domain)?;
        if !t.in_book() {
            return Err(MathError::Domain);
        }
        if t.q() < 0 {
            x_of(t.theta(), t.q(), b)?
        } else {
            t.theta()
        }
    };
    let theta_f = terms.get(field).ok_or(MathError::Domain)?.theta();
    let ef = exp_q64(theta_f)?;
    let ek = exp_q64(x_k)?;
    let merged = ov(ef
        .saturating_sub(weight_err(ef))
        .checked_add(ek.saturating_sub(weight_err(ek))))?;
    let field_theta = ln_q64(merged)? - LN_ERR;
    terms[field].set_theta(field_theta);
    terms[k].leave_book();
    let (base, sum) = refresh(terms, &[k, field], base, sum, b)?;
    Ok(Scratch {
        base,
        sum,
        yes: SCRATCH_YES,
        no: SCRATCH_NO,
        field_theta,
    })
}

/// What a scratch of an entry with `mass_ppm` and net quantity `q` adds to FIELD's mass_ppm (the
/// mass scratch merged): `mass_ppm` itself when q >= 0, and when q < 0 floor(mass_ppm *
/// lo(e^(q/b))), lo(e) = e - weight_err(e) for e = exp_q64(floor(q * 2^64 / b)), rounded down so
/// FIELD's mass never claims more than the weight it took (k's theta is itself at or below its
/// mass). The live masses then sum to at most 1,000,000.
pub fn scratch_mass_ppm(mass_ppm: u32, q: i64, b: u64) -> MathResult<u32> {
    if u64::from(mass_ppm) > PPM {
        return Err(MathError::Domain);
    }
    if q >= 0 {
        return Ok(mass_ppm);
    }
    let e = exp_q64(x_of(0, q, b)?)?;
    let lo = e.saturating_sub(weight_err(e));
    // lo < 2^64 and mass_ppm <= 1e6 < 2^20: the product fits, and the result is at most mass_ppm.
    let m = (u128::from(mass_ppm) * lo) >> 64;
    u32::try_from(m).map_err(|_| MathError::Overflow)
}

// ------------------------------------------------------------------------------ USDC to shares

/// The most shares a buy of `yes` (YES) or NO on the term at i, from `position` (same side or 0),
/// can take for at most `budget` base units, fee included. The closed form shares = b ln(1 +
/// (e^(c/b) - 1) / p) (p the side's price, 1 - p for NO; computed as b (ln(e^(c/b) - 1 + p) -
/// ln p)) less a margin of 2^-16 and 1 is the start; each probe is priced by trade and
/// trade_flows themselves, galloping out from it and then bisecting, so the answer is exact.
#[allow(clippy::too_many_arguments)]
pub fn shares_for_budget<T: Weighted + Clone>(
    terms: &[T],
    i: usize,
    yes: bool,
    position: i64,
    budget: u64,
    fee_rate_ppm: u32,
    base: i128,
    sum: u128,
    b: u64,
) -> MathResult<u64> {
    if (yes && position < 0) || (!yes && position > 0) {
        return Err(MathError::CrossesZero);
    }
    let t = terms.get(i).ok_or(MathError::Domain)?;
    if !t.in_book() {
        return Err(MathError::Domain);
    }
    let cap = (i64::MAX as u64) - position.unsigned_abs();
    let total = |s: u64| -> MathResult<Option<u64>> {
        if s == 0 {
            return Ok(Some(0));
        }
        let delta = if yes { s as i64 } else { -(s as i64) };
        let mut probe = terms.to_vec();
        let (_, _, dc) = match trade(&mut probe, i, delta, base, sum, b) {
            Ok(v) => v,
            Err(MathError::Overflow) => return Ok(None),
            Err(e) => return Err(e),
        };
        match trade_flows(position, delta, dc, fee_rate_ppm) {
            Ok(f) => Ok(f.pay.checked_add(f.fee)),
            Err(MathError::BuyTooSmall) => Ok(Some(1)),
            Err(MathError::Overflow) => Ok(None),
            Err(e) => Err(e),
        }
    };
    let fits = |s: u64| -> MathResult<bool> { Ok(total(s)?.is_some_and(|c| c <= budget)) };

    let (p_yes, _) = frac_q64(t.w().min(sum), sum)?;
    let p = if yes { p_yes } else { ONE - p_yes }.max(1);
    let c = ((u128::from(budget) << 64) / u128::from(b)).min(39 << 64) as i128;
    let e = exp_q64(c)?;
    let a = e - ONE + p;
    let est = (i128::from(b) * (ln_q64(a)? - ln_q64(p)?)) >> 64;
    let est = u64::try_from(est.max(0)).unwrap_or(u64::MAX).min(cap);
    let start = est.saturating_sub((est >> 16) + 1);

    let (mut lo, mut hi);
    if fits(start)? {
        lo = start;
        let mut step: u64 = 1;
        loop {
            let next = lo.saturating_add(step).min(cap);
            if next == lo || !fits(next)? {
                hi = next.max(lo + 1);
                break;
            }
            lo = next;
            step = step.saturating_mul(2);
        }
    } else {
        hi = start;
        let mut step: u64 = 1;
        loop {
            if hi <= step {
                lo = 0;
                break;
            }
            let next = hi - step;
            if fits(next)? {
                lo = next;
                break;
            }
            hi = next;
            step = step.saturating_mul(2);
        }
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fits(mid)? {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_and_ln_round_trip_at_fixed_points() {
        assert_eq!(exp_q64(0).unwrap(), ONE);
        assert_eq!(ln_q64(ONE).unwrap(), 0);
        // e^(ln 2) = 2 within the bound.
        let two = exp_q64(LN2).unwrap();
        assert!(two.abs_diff(2 * ONE) <= weight_err(two));
        // ln(2) = LN2 within LN_ERR.
        assert!((ln_q64(2 * ONE).unwrap() - LN2).abs() <= LN_ERR);
        assert_eq!(exp_q64(EXP_MIN_ARG - 1).unwrap(), 0);
        assert_eq!(exp_q64(EXP_MAX_ARG + 1), Err(MathError::Overflow));
        assert_eq!(ln_q64(0), Err(MathError::Domain));
    }

    #[test]
    fn theta_is_never_above_the_mass() {
        for m in [1u32, 100, 5_000, 333_333, 999_999, 1_000_000] {
            let theta = theta_of_ppm(m).unwrap();
            let e = exp_q64(theta).unwrap();
            let mass = (u128::from(m) << 64) / u128::from(PPM);
            assert!(e <= mass + weight_err(e), "mass {m}");
        }
    }

    #[test]
    fn liquidity_matches_the_design_default() {
        // 50 USDC at m_floor 0.005: b = floor(50e6 / ln 200) = 9,436,859 or one below it.
        let (b, s) = liquidity_for(50_000_000, 5_000).unwrap();
        assert!((9_436_000..=9_437_000).contains(&b), "b {b}");
        assert!(
            s <= 50_000_000 + SEED_CUSHION + 1 && s > 50_000_000 - 20,
            "S {s}"
        );
    }

    #[test]
    fn a_buy_and_its_sale_never_pay_the_trader() {
        let (b, _) = liquidity_for(50_000_000, 5_000).unwrap();
        let mut terms = vec![
            Term::new(theta_of_ppm(100_000).unwrap()),
            Term::new(theta_of_ppm(900_000).unwrap()),
        ];
        let (base, sum) = open_book(&mut terms, b).unwrap();
        let (b1, s1, dc_buy) = trade(&mut terms, 0, 5_000_000, base, sum, b).unwrap();
        let (_, _, dc_sell) = trade(&mut terms, 0, -5_000_000, b1, s1, b).unwrap();
        let buy = trade_flows(0, 5_000_000, dc_buy, 0).unwrap();
        let sell = trade_flows(5_000_000, -5_000_000, dc_sell, 0).unwrap();
        assert!(sell.receive <= buy.pay);
        assert!(buy.pay - sell.receive <= 4);
    }

    #[test]
    fn no_part_of_i64_min_is_an_overflow_not_a_panic() {
        assert_eq!(no_part(i64::MIN), Err(MathError::Overflow));
        assert_eq!(no_part(i64::MIN + 1), Ok(i64::MAX));
        assert_eq!(no_part(-5), Ok(5));
        assert_eq!(no_part(0), Ok(0));
        assert_eq!(no_part(7), Ok(0));
        // A NO buy to i64::MIN, and a sale from it, are refused the same way.
        assert_eq!(trade_flows(0, i64::MIN, -1, 0), Err(MathError::Overflow));
        assert_eq!(trade_flows(i64::MIN, 1, 0, 0), Err(MathError::Overflow));
    }

    /// The reviewer's book (F1): b at the design default, NONE 0.1, an entry at 0.5 split from
    /// FIELD, then Y = 100 USDC of YES and N = Y - q_net of NO on it. Returns the book, its frame,
    /// b, Y and N.
    fn hedged(q_net: i64) -> (Vec<Term>, i128, u128, u64, i64, i64) {
        let (b, _) = liquidity_for(50_000_000, 5_000).unwrap();
        let mut terms = vec![
            Term::new(theta_of_ppm(100_000).unwrap()),
            Term::new(theta_of_ppm(900_000).unwrap()),
        ];
        let (base, sum) = open_book(&mut terms, b).unwrap();
        terms.push(Term::new(0));
        let (base, sum) = join(&mut terms, 1, 2, 500_000, base, sum, b).unwrap();
        let y: i64 = 100_000_000;
        let n = y - q_net;
        let (base, sum, _) = trade(&mut terms, 2, y, base, sum, b).unwrap();
        let (base, sum, _) = trade(&mut terms, 2, -n, base, sum, b).unwrap();
        assert_eq!(terms[2].q, q_net);
        (terms, base, sum, b, y, n)
    }

    #[test]
    fn a_scratch_pays_yes_nothing_and_no_one_whatever_the_book() {
        for q in [
            0i64, 1, -1, 3, -3, 100, -100, 1_000_000, -1_000_000, 50_000_000,
        ] {
            let (mut terms, base, sum, b, y, n) = hedged(q);
            let (theta_f, theta_k) = (terms[1].theta, terms[2].theta);
            let before = terms.clone();
            let s = scratch(&mut terms, 2, 1, base, sum, b).unwrap();
            assert_eq!((s.yes, s.no), (SCRATCH_YES, SCRATCH_NO), "q {q}");
            assert_eq!(value_of(y as u64, s.yes), 0, "q {q}: YES paid");
            assert_eq!(value_of(n as u64, s.no), n as u64, "q {q}: NO paid 1 each");
            // k left the book with its q kept; the cached sum is exact.
            assert!(!terms[2].in_book && terms[2].w == 0 && terms[2].q == q);
            assert_eq!(s.sum, terms.iter().map(|t| t.w).sum::<u128>(), "q {q}: sum");
            assert_eq!(terms[1].theta, s.field_theta);
            // FIELD never weighs more than it and k did together: k's base mass when q >= 0, its
            // current weight e^(x_k) when q < 0.
            let x_k = if q < 0 {
                x_of(theta_k, q, b).unwrap()
            } else {
                theta_k
            };
            let (ef, ek) = (exp_q64(theta_f).unwrap(), exp_q64(x_k).unwrap());
            let merged = exp_q64(s.field_theta).unwrap();
            assert!(merged <= ef + ek, "q {q}: FIELD {merged} > {ef} + {ek}");
            assert!(merged + (1 << 20) >= ef + ek, "q {q}: FIELD lost mass");
            if q < 0 {
                // No other price moves: NONE's weight against the sum, before and after.
                let p0 = frac_q64(before[0].w, sum).unwrap().0;
                let p1 = frac_q64(terms[0].w, s.sum).unwrap().0;
                assert!(p0.abs_diff(p1) <= 1 << 12, "q {q}: NONE's price moved");
            }
            // The book trades on after it.
            let (_, s2, _) = trade(&mut terms, 0, 1_000_000, s.base, s.sum, b).unwrap();
            assert_eq!(s2, terms.iter().map(|t| t.w).sum::<u128>());
        }
    }

    #[test]
    fn a_scratch_gives_field_the_mass_it_merged() {
        let (b, _) = liquidity_for(50_000_000, 5_000).unwrap();
        for q in [0i64, 1, 50_000_000] {
            assert_eq!(scratch_mass_ppm(50_000, q, b), Ok(50_000));
        }
        // floor(mass e^(q/b)), never above the real value.
        let one_b = -(b as i64);
        let got = scratch_mass_ppm(50_000, one_b, b).unwrap();
        assert_eq!(got, 18_393, "50,000 e^-1 = 18,393.97");
        assert_eq!(scratch_mass_ppm(5_000, -1, b), Ok(4_999));
        assert_eq!(scratch_mass_ppm(1_000_000, -(b as i64) * 60, b), Ok(0));
        assert_eq!(scratch_mass_ppm(1_000_001, 0, b), Err(MathError::Domain));
        assert_eq!(scratch_mass_ppm(1_000_000, i64::MIN, MIN_B), Ok(0));
    }

    #[test]
    fn a_scratch_refuses_field_and_an_outcome_out_of_the_book() {
        let (mut terms, base, sum, b, _, _) = hedged(5);
        assert_eq!(
            scratch(&mut terms, 1, 1, base, sum, b),
            Err(MathError::Domain)
        );
        let s = scratch(&mut terms, 2, 1, base, sum, b).unwrap();
        assert_eq!(
            scratch(&mut terms, 2, 1, s.base, s.sum, b),
            Err(MathError::Domain)
        );
        assert_eq!(
            scratch(&mut terms, 9, 1, s.base, s.sum, b),
            Err(MathError::Domain)
        );
    }

    #[test]
    fn value_one_pays_every_share_and_no_other_value_reads_as_one() {
        assert_eq!(value_of(70_000_000, VALUE_ONE), 70_000_000);
        assert_eq!(value_of(u64::MAX, VALUE_ONE), u64::MAX);
        assert_eq!(value_of(1, VALUE_ONE), 1);
        assert_eq!(value_of(0, VALUE_ONE), 0);
        assert_eq!(value_of(70_000_000, SCRATCH_YES), 0);
        assert_eq!(value_of(4, (ONE / 4) as u64), 1);
        assert_eq!(value_of(3, VALUE_ONE - 1), 2);
        assert_eq!(value_q64(ONE), VALUE_ONE);
        assert_eq!(value_q64(ONE + 5), VALUE_ONE);
        assert_eq!(value_q64(ONE - 1), VALUE_ONE - 1);
        assert_eq!(value_q64(ONE / 4), (ONE / 4) as u64);
        assert_eq!(value_q64(0), 0);
        // A mark is never read as ONE unless the bound is ONE itself.
        let (b, _) = liquidity_for(50_000_000, 5_000).unwrap();
        let mut terms = vec![
            Term::new(theta_of_ppm(100_000).unwrap()),
            Term::new(theta_of_ppm(900_000).unwrap()),
        ];
        let (_, sum) = open_book(&mut terms, b).unwrap();
        let (yes, no) = marks(terms[0].w, sum, 2).unwrap();
        assert!(yes < VALUE_ONE && no < VALUE_ONE);
        assert!(u128::from(yes) + u128::from(no) <= ONE);
    }
}
