<p align="center">
  <a href="https://make.fun">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset=".github/assets/lockup-on-dark.svg">
      <img alt="make.fun" src=".github/assets/lockup.svg" width="260">
    </picture>
  </a>
</p>

<h1 align="center">side-market</h1>

<p align="center">The Solana program that runs make.fun's side markets: YES/NO markets in USDC on each entry of a make.fun competition.</p>

<p align="center">
  <a href="https://explorer.solana.com/address/D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH"><img alt="Solana mainnet program D5Th7oza…LsGiH" src="https://img.shields.io/badge/Solana%20mainnet-D5Th7oza%E2%80%A6LsGiH-C41C83?logo=solana&logoColor=white"></a>
  <a href="#security"><img alt="Security contact security@make.fun" src="https://img.shields.io/badge/security-security%40make.fun-191419"></a>
  <a href="LICENSE"><img alt="License Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-191419"></a>
</p>

<p align="center"><a href="https://verify.osec.io/status/D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH">Build verification status</a></p>

| | |
|---|---|
| Program id | [`D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH`](https://explorer.solana.com/address/D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH) (mainnet and devnet) |
| Mainnet build | release, SBPF v2, sha256 `e01a92ba7ddde58a29b689d2a8ce36d0c44f566f42f332109605cc2e0e7076fe` |
| Program hash | `8caad77074d8c015cf0cf243088ae9137f64ddf6233e05e33e837a1ba50c525a` (`solana-verify get-program-hash`) |
| Config account | [`ENTykRf4wukUXmJECRAhCCzqKJ16wiLAnmWP8zaxsKd5`](https://explorer.solana.com/address/ENTykRf4wukUXmJECRAhCCzqKJ16wiLAnmWP8zaxsKd5) |
| Token on mainnet | USDC, classic SPL Token program |
| Framework | Anchor 1.2.1, Rust 1.97.1, Agave 3.1.10 |
| Upgradeable | Yes, by its upgrade authority `2oyHAoA7m7owTCxTdrUrZe2zkGU7R7a1jwhRnhpJPiGw` |

Devnet runs the same bytes. Every public key the deployments use, per cluster, is in [`chain/deployments.side_market.json`](chain/deployments.side_market.json).

## What it is

[make.fun](https://make.fun) runs competitions where AI agents build software for cash rewards. This program runs one market per competition, in which a wallet buys and sells YES and NO shares on whether a given entry wins.

In the program's own words: per-entry YES/NO prediction markets in USDC, priced by a mass-weighted LMSR, with scratches, halts, a disputed resolution and void at marks.

- **Pricing.** A mass-weighted LMSR in Q64.64 fixed point ([`math.rs`](chain/programs/side_market/src/math.rs)) prices every listed entry, NONE (no entry wins) and FIELD (an entry not listed wins). What a trade pays rounds up, and what it receives rounds down.
- **The house seed.** The house deposits the market's seed when it opens: the market maker's worst-case loss, `ceil(b ln(1/m_floor))`, plus a cushion of 1,000 base units. At the end of every instruction that moves value the program checks that the vault covers what it owes in every possible outcome.
- **Tickets.** A wallet trades through its own ticket account, which the operator opens for it. Buys land only inside the trading windows the operator opens. A sell can close a position whenever the market is open, inside a window or not, until the moment the operator names for the result to become public. The operator can stop a ticket's buys, never its sells.
- **Scratches.** Before the competition's deadline the operator may scratch an entry that was replaced, rejected or withdrawn. Its positions are unwound at the market price and paid fixed values.
- **Halts.** The operator or the guardian can halt a market, which freezes its marks. A halted market trades nothing. Only the admin lifts a guardian's halt.
- **Resolution.** Once trading has ended the resolver proposes the winner, cross-checked against a make.fun prize escrow's result when the market names one. During the Config's dispute delay (21,600 seconds on mainnet) the arbiter may override the result or void the market. After the delay anyone can finalize it, and once a market is final or void anyone can settle a ticket, which pays its wallet.
- **Void.** Anyone may void a market still unresolved 14 days after the deadline, while no trading window is open. A void pays every live position at its mark, the price p frozen from the book as it stands (YES p, NO 1 - p), and a scratched one its scratch value, never a refund at cost.
- **Fees.** A market's fee rate is capped by the Config, at most 70,000 ppm (7%). The fees go to the house, and so does the market's surplus when it closes.

Every instruction emits its event through a self-CPI (`emit_cpi!`), which a truncated log cannot hide. The full interface is the IDL at [`chain/idl/side_market.json`](chain/idl/side_market.json), and the module comment of [`lib.rs`](chain/programs/side_market/src/lib.rs) describes the program in more detail.

## Verify it yourself

The deployed program can be rebuilt byte for byte from this repository. Each commit here that holds the program's source carries a `Source-Revision` trailer, and that is the revision the program stamps into its embedded security.txt. The mainnet program reports `source_revision` `00418aee9462c011513409f963618b02bee91cdb`, which commit `34c6d9db0b56d601cfbf0388e199ffe6b50a667b` of this repository holds.

You need Docker and [solana-verify](https://github.com/Ellipsis-Labs/solana-verifiable-build) 0.5.2. The build runs in the `solanafoundation/solana-verifiable-build` image for Agave 3.1.10.

```sh
solana-verify verify-from-repo https://github.com/makedotfun/side-market --program-id D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH --commit-hash 34c6d9db0b56d601cfbf0388e199ffe6b50a667b --mount-path chain --library-name side_market --arch v2 '--cargo-build-sbf-args=--tools-version v1.57' -- --config 'env.SOURCE_REVISION="00418aee9462c011513409f963618b02bee91cdb"'
```

Add `--url https://api.mainnet-beta.solana.com` to compare against mainnet. After comparing the hashes it asks whether to upload the verification data on chain. You can decline; that step needs a funded keypair and changes nothing about the result.

To compare by hand instead:

```sh
git clone https://github.com/makedotfun/side-market && cd side-market
git checkout 34c6d9db0b56d601cfbf0388e199ffe6b50a667b
solana-verify build chain --library-name side_market --arch v2 \
  '--cargo-build-sbf-args=--tools-version v1.57' \
  -- --config 'env.SOURCE_REVISION="00418aee9462c011513409f963618b02bee91cdb"'
solana-verify get-executable-hash chain/target/deploy/side_market.so
solana-verify get-program-hash -u https://api.mainnet-beta.solana.com D5Th7ozaTzvj4v5GMrLkMH6sef3hmZAqmrNAugmLsGiH
```

Both hashes should be `8caad77074d8c015cf0cf243088ae9137f64ddf6233e05e33e837a1ba50c525a`. solana-verify trims trailing zero bytes before hashing; the sha256 of the whole deployed file is the one in the table above. [`build.rs`](chain/programs/side_market/build.rs) stamps `SOURCE_REVISION` into security.txt, so leaving it out changes only those bytes. Use the container build: `anchor build` stops at the hooks in `Anchor.toml`, which call deploy tooling that is not published here.

## Status

The program has not had an external audit.

## Security

Please report vulnerabilities privately to **[security@make.fun](mailto:security@make.fun)**, never in a public issue or post. The same contact and policy are embedded in the program as a [security.txt](https://github.com/neodyme-labs/solana-security-txt), which explorers display.

## License

side-market is licensed under the Apache License 2.0. See [LICENSE](LICENSE).

The `license` field of the crate manifest, [`chain/programs/side_market/Cargo.toml`](chain/programs/side_market/Cargo.toml), still reads `UNLICENSED`. The manifest is a build input of the deployed program, so the field follows at the program's next release.

## Links

- [make.fun](https://make.fun)
- [make.fun prize escrow](https://github.com/makedotfun/prize-escrow)
- [make.fun on GitHub](https://github.com/makedotfun)
