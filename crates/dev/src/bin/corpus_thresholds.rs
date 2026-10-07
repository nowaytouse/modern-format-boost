//! Corpus maturity threshold inspector.
//!
//! Port of `crates/dev/scripts/mfb_corpus_thresholds.py`.
//! Library SSOT lives in `dev::infra::corpus_thresholds`.

use anyhow::{Result, bail};
use clap::Parser;
use dev::infra::corpus_thresholds::{
    ENV_DISABLE_STRICT_CORPUS, loop_corpus_is_mature, loop_corpus_samples_shortfall,
    min_loop_samples_per_class, min_loop_samples_total, min_quality_samples_per_class,
    min_quality_samples_total, quality_corpus_is_mature, quality_corpus_samples_shortfall,
    strict_corpus_enabled,
};

#[derive(Parser, Debug)]
#[command(
    name = "corpus_thresholds",
    about = "Print resolved corpus maturity thresholds (mirrors mfb_corpus_thresholds.py)"
)]
struct Args {
    /// Check whether a loop corpus is mature: TOTAL,QUALITY_CLASS,VIDEO_CLASS
    #[arg(long, value_name = "TOTAL,QUALITY,VIDEO")]
    check_loop: Option<String>,

    /// Check whether a quality corpus is mature: HIGH,LOW
    #[arg(long, value_name = "HIGH,LOW")]
    check_quality: Option<String>,

    /// Exit non-zero when any corpus is immature (useful in CI gates)
    #[arg(long)]
    assert_mature: bool,
}

fn parse_u64_triple(s: &str, context: &str) -> Result<(u64, u64, u64)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        bail!("{context}: expected 3 comma-separated integers, got {s:?}");
    }
    let a = parts[0]
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("{context}[0]: {e}"))?;
    let b = parts[1]
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("{context}[1]: {e}"))?;
    let c = parts[2]
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("{context}[2]: {e}"))?;
    Ok((a, b, c))
}

fn parse_u64_pair(s: &str, context: &str) -> Result<(u64, u64)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        bail!("{context}: expected 2 comma-separated integers, got {s:?}");
    }
    let a = parts[0]
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("{context}[0]: {e}"))?;
    let b = parts[1]
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("{context}[1]: {e}"))?;
    Ok((a, b))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let strict = strict_corpus_enabled();

    println!("=== Corpus Maturity Thresholds ===");
    println!(
        "  strict_corpus: {} ({}={})",
        strict,
        ENV_DISABLE_STRICT_CORPUS,
        std::env::var(ENV_DISABLE_STRICT_CORPUS).unwrap_or_else(|_| "<unset>".to_string())
    );
    println!();
    println!("  Loop intent:");
    println!("    min_total            = {}", min_loop_samples_total());
    println!(
        "    min_per_class        = {}",
        min_loop_samples_per_class()
    );
    println!();
    println!("  Static image quality:");
    println!("    min_total            = {}", min_quality_samples_total());
    println!(
        "    min_per_class        = {}",
        min_quality_samples_per_class()
    );
    println!();

    let mut any_immature = false;

    if let Some(spec) = &args.check_loop {
        let (total, quality_class, video_class) = parse_u64_triple(spec, "--check-loop")?;
        let mature = loop_corpus_is_mature(total, quality_class, video_class);
        let shortfall = loop_corpus_samples_shortfall(total, quality_class, video_class);
        println!(
            "  Loop corpus check (total={total}, quality={quality_class}, video={video_class}):"
        );
        println!(
            "    mature: {}  shortfall: {}",
            if mature { "YES" } else { "NO" },
            shortfall
        );
        if !mature {
            any_immature = true;
        }
    }

    if let Some(spec) = &args.check_quality {
        let (high, low) = parse_u64_pair(spec, "--check-quality")?;
        let mature = quality_corpus_is_mature(high, low);
        let shortfall = quality_corpus_samples_shortfall(high, low);
        println!("  Quality corpus check (high={high}, low={low}):");
        println!(
            "    mature: {}  shortfall: {}",
            if mature { "YES" } else { "NO" },
            shortfall
        );
        if !mature {
            any_immature = true;
        }
    }

    if args.assert_mature && any_immature {
        bail!("corpus is immature — see thresholds above");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shortfall_zero_when_mature() {
        let sf = loop_corpus_samples_shortfall(300, 100, 100);
        assert_eq!(sf, 0);
    }

    #[test]
    fn test_quality_shortfall_non_zero() {
        let sf = quality_corpus_samples_shortfall(5, 5);
        assert!(sf > 0);
    }

    #[test]
    fn test_parse_u64_triple_valid() {
        let (a, b, c) = parse_u64_triple("10,20,30", "test").unwrap();
        assert_eq!((a, b, c), (10, 20, 30));
    }

    #[test]
    fn test_parse_u64_pair_valid() {
        let (a, b) = parse_u64_pair("42,99", "test").unwrap();
        assert_eq!((a, b), (42, 99));
    }

    #[test]
    fn test_parse_u64_triple_wrong_count() {
        assert!(parse_u64_triple("1,2", "test").is_err());
    }
}
