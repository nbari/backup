//! Shared clap value parsers for backup command arguments.

use clap::builder::ValueParser;
use std::{fs, path::PathBuf};

/// Accept a backup name of ASCII alphanumerics and underscores (but not a lone underscore).
#[must_use]
pub fn is_alphanumeric() -> ValueParser {
    ValueParser::from(move |s: &str| -> std::result::Result<String, String> {
        if s == "_" {
            return Err("The name cannot be just an underscore".to_string());
        }

        if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Ok(s.to_string());
        }

        Err("Only alphanumeric characters and underscore are allowed".to_string())
    })
}

/// Accept a path that exists and is a file.
#[must_use]
pub fn is_file() -> ValueParser {
    ValueParser::from(move |s: &str| -> std::result::Result<PathBuf, String> {
        if let Ok(metadata) = fs::metadata(s)
            && metadata.is_file()
        {
            return Ok(PathBuf::from(s));
        }

        Err(format!("Invalid file path or file does not exist: '{s}'"))
    })
}

/// Accept a path that exists and is a directory.
#[must_use]
pub fn is_dir() -> ValueParser {
    ValueParser::from(move |s: &str| -> std::result::Result<PathBuf, String> {
        if let Ok(metadata) = fs::metadata(s)
            && metadata.is_dir()
        {
            return Ok(PathBuf::from(s));
        }

        Err(format!(
            "Invalid directory path or directory does not exist: '{s}'"
        ))
    })
}
