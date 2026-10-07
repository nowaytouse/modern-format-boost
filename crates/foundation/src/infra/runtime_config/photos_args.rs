//! Shared Photos overrides for direct CLI and GUI launcher requests.
use super::{LoadedConfig, PhotosBackend};
use clap::{Args, ValueEnum};

#[derive(Args, Debug, Default, Clone)]
pub struct PhotosArgs {
    #[arg(long, global = true, value_enum)]
    photos_backend: Option<PhotosBackend>,
    /// Top-level Photos folder name (one component).
    #[arg(long, global = true)]
    photos_import_root: Option<String>,
    /// Base Photos album name (one component).
    #[arg(long, global = true)]
    photos_album_name: Option<String>,
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    preserve_folder_structure: Option<bool>,
    #[arg(long, global = true)]
    photos_native_batch_size: Option<usize>,
    #[arg(long, global = true)]
    photos_import_batch_size: Option<usize>,
    /// Independent verification window and query cap.
    #[arg(long, global = true)]
    photos_verification_batch_size: Option<usize>,
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    photos_adaptive_batching: Option<bool>,
    #[arg(long, global = true)]
    photos_native_min_batch_size: Option<usize>,
    #[arg(long, global = true)]
    photos_native_max_batch_size: Option<usize>,
    #[arg(long, global = true)]
    photos_target_batch_seconds: Option<u64>,
}

impl PhotosArgs {
    pub fn apply_to(&self, loaded: &mut LoadedConfig) {
        macro_rules! apply {
            ($($field:ident => $key:ident),* $(,)?) => {$(
                if let Some(value) = &self.$field {
                    loaded.config.photos.$key = *value;
                    loaded.sources.insert(concat!("photos.", stringify!($key)).into(), "CLI".into());
                }
            )*};
        }
        apply!(photos_backend => backend, preserve_folder_structure => preserve_folder_structure,
            photos_native_batch_size => native_batch_size, photos_import_batch_size => import_batch_size,
            photos_verification_batch_size => verification_batch_size, photos_adaptive_batching => adaptive_batching,
            photos_native_min_batch_size => native_min_batch_size, photos_native_max_batch_size => native_max_batch_size,
            photos_target_batch_seconds => target_batch_seconds);
        for (value, target, key) in [
            (
                &self.photos_import_root,
                &mut loaded.config.photos.import_root,
                "photos.import_root",
            ),
            (
                &self.photos_album_name,
                &mut loaded.config.photos.album_name,
                "photos.album_name",
            ),
        ] {
            if let Some(value) = value {
                *target = Some(value.clone());
                loaded.sources.insert(key.into(), "CLI".into());
            }
        }
    }

    #[must_use]
    pub fn cli_arguments(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(value) = self
            .photos_backend
            .and_then(|value| value.to_possible_value())
        {
            args.extend(["--photos-backend".into(), value.get_name().into()]);
        }
        for (flag, value) in [
            ("--photos-import-root", self.photos_import_root.clone()),
            ("--photos-album-name", self.photos_album_name.clone()),
            (
                "--photos-native-batch-size",
                self.photos_native_batch_size.map(|v| v.to_string()),
            ),
            (
                "--photos-import-batch-size",
                self.photos_import_batch_size.map(|v| v.to_string()),
            ),
            (
                "--photos-verification-batch-size",
                self.photos_verification_batch_size.map(|v| v.to_string()),
            ),
            (
                "--photos-native-min-batch-size",
                self.photos_native_min_batch_size.map(|v| v.to_string()),
            ),
            (
                "--photos-native-max-batch-size",
                self.photos_native_max_batch_size.map(|v| v.to_string()),
            ),
            (
                "--photos-target-batch-seconds",
                self.photos_target_batch_seconds.map(|v| v.to_string()),
            ),
        ] {
            if let Some(value) = value {
                args.extend([flag.into(), value]);
            }
        }
        for (flag, value) in [
            (
                "--preserve-folder-structure",
                self.preserve_folder_structure,
            ),
            ("--photos-adaptive-batching", self.photos_adaptive_batching),
        ] {
            if let Some(value) = value {
                args.push(format!("{flag}={value}"));
            }
        }
        args
    }
}
