//! Shared cache-policy flags for processors and explicit cache maintenance.

use super::{LoadedConfig, record_source};

#[derive(Debug, Clone, Default, clap::Args)]
pub struct CacheArgs {
    /// Maximum serialized directory-snapshot payload bytes, not total database file size.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=i64::MAX.unsigned_abs()))]
    cache_max_bytes: Option<u64>,
    /// Directory-snapshot idle lifetime in seconds; hits refresh the last-use time.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=i64::MAX.unsigned_abs()))]
    cache_ttl_seconds: Option<u64>,
}

impl CacheArgs {
    #[must_use]
    pub fn cli_arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        for (flag, value) in [
            ("--cache-max-bytes", self.cache_max_bytes),
            ("--cache-ttl-seconds", self.cache_ttl_seconds),
        ] {
            if let Some(value) = value {
                arguments.extend([flag.to_owned(), value.to_string()]);
            }
        }
        arguments
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cache_max_bytes.is_none() && self.cache_ttl_seconds.is_none()
    }

    pub fn apply_to(&self, loaded: &mut LoadedConfig) {
        for (value, target, key) in [
            (
                self.cache_max_bytes,
                &mut loaded.config.cache.path_tree_max_bytes,
                "cache.path_tree_max_bytes",
            ),
            (
                self.cache_ttl_seconds,
                &mut loaded.config.cache.path_tree_ttl_seconds,
                "cache.path_tree_ttl_seconds",
            ),
        ] {
            if let Some(value) = value {
                *target = value;
                record_source(&mut loaded.sources, &mut loaded.source_chain, key, "CLI");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        cache: CacheArgs,
    }

    #[test]
    fn cache_flags_preserve_omitted_fields_and_track_explicit_overrides() {
        let mut loaded = crate::runtime_config::load(None, true).unwrap();
        let default = Cli::try_parse_from(["test"]).unwrap();
        assert!(default.cache.is_empty());
        assert!(default.cache.cli_arguments().is_empty());
        default.cache.apply_to(&mut loaded);
        assert_eq!(loaded.sources["cache.path_tree_max_bytes"], "default");
        let cli = Cli::try_parse_from(["test", "--cache-max-bytes", "268435456"]).unwrap();
        assert!(!cli.cache.is_empty());
        assert_eq!(
            cli.cache.cli_arguments(),
            ["--cache-max-bytes", "268435456"]
        );
        cli.cache.apply_to(&mut loaded);
        assert_eq!(loaded.config.cache.path_tree_max_bytes, 268_435_456);
        assert_eq!(
            loaded.source_chain["cache.path_tree_max_bytes"],
            ["default", "CLI"]
        );
        assert_eq!(loaded.sources["cache.path_tree_ttl_seconds"], "default");
    }

    #[test]
    fn cache_flags_reject_zero_negative_overflow_and_accept_full_storage_range() {
        for flag in ["--cache-max-bytes", "--cache-ttl-seconds"] {
            for value in ["0", "-1", "9223372036854775808", "1.5", "unknown"] {
                assert!(Cli::try_parse_from(["test", flag, value]).is_err());
            }
            for value in ["1", "9223372036854775807"] {
                let cli = Cli::try_parse_from(["test", flag, value]).unwrap();
                let mut loaded = crate::runtime_config::load(None, true).unwrap();
                cli.cache.apply_to(&mut loaded);
                loaded.config.validate().unwrap();
            }
        }
    }
}
