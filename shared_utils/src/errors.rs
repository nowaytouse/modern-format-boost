use thiserror::Error;

#[derive(Error, Debug)]
pub enum VidQualityError {
    #[error("Video format not supported: {0}")]
    UnsupportedFormat(String),

    #[error("Failed to read video: {0}")]
    VideoReadError(String),

    #[error("FFprobe failed: {0}")]
    FFprobeError(String),

    #[error("FFmpeg failed: {0}")]
    FFmpegError(String),

    #[error("Conversion failed: {0}")]
    ConversionError(String),

    #[error("External tool not found: {0}")]
    ToolNotFound(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    // Allow converting other errors to string for general failures
    #[error("General error: {0}")]
    GeneralError(String),
}

pub type Result<T> = std::result::Result<T, VidQualityError>;
