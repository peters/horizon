//! Minimal GGUF metadata reader for speech models.
//!
//! Reads only the key/value header of a transcribe.cpp GGUF — never the
//! tensor data — so the settings UI can offer the model's real language
//! list and translation targets without loading gigabytes of weights.
//! Pure std, tolerant of unknown keys, and bounded against malformed files.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// Speech-relevant metadata extracted from a GGUF header.
///
/// Capability fields are `Option` so "the KV is absent" (family default
/// applies) stays distinguishable from an explicit `false`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpeechModelInfo {
    /// BCP-47-ish language codes the model declares (`general.languages`).
    pub languages: Vec<String>,
    /// Whether the model supports the translate task, when declared.
    pub supports_translate: Option<bool>,
    /// Whether the model supports source-language auto-detection, when
    /// declared (`stt.capability.lang_detect`).
    pub supports_lang_detect: Option<bool>,
    /// Declared translation target languages (usually `["en"]`).
    pub translate_targets: Vec<String>,
    /// Declared source→target translation pairs (`stt.translation.pairs`,
    /// entries shaped `src>tgt`). Empty means "not restricted".
    pub translate_pairs: Vec<(String, String)>,
}

impl SpeechModelInfo {
    /// Translation targets valid for a given source language, honoring the
    /// model's declared pair restrictions when present. `auto` (or empty)
    /// returns the union of all targets.
    #[must_use]
    pub fn targets_for_source(&self, source: &str) -> Vec<String> {
        if self.translate_pairs.is_empty() {
            return self.translate_targets.clone();
        }
        let source = source.trim();
        let unrestricted = source.is_empty() || source == "auto";
        let mut targets: Vec<String> = Vec::new();
        for (from, to) in &self.translate_pairs {
            if (unrestricted || from == source) && !targets.contains(to) {
                targets.push(to.clone());
            }
        }
        targets
    }
}

const GGUF_MAGIC: [u8; 4] = *b"GGUF";
/// Caps against malformed headers: no real model comes close.
const MAX_KV_COUNT: u64 = 100_000;
const MAX_STRING_LEN: u64 = 16 * 1024 * 1024;
const MAX_ARRAY_LEN: u64 = 1_000_000;
/// Cumulative allocation budget for all strings this parser materializes.
/// A malformed file must not be able to demand per-item-cap × item-count
/// memory, even when parsed by a background metadata worker.
const MAX_TOTAL_STRING_BYTES: u64 = 8 * 1024 * 1024;

const TYPE_UINT8: u32 = 0;
const TYPE_INT8: u32 = 1;
const TYPE_UINT16: u32 = 2;
const TYPE_INT16: u32 = 3;
const TYPE_UINT32: u32 = 4;
const TYPE_INT32: u32 = 5;
const TYPE_FLOAT32: u32 = 6;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_UINT64: u32 = 10;
const TYPE_INT64: u32 = 11;
const TYPE_FLOAT64: u32 = 12;

/// Read speech metadata from a GGUF file. Returns `None` for anything that
/// is not a well-formed GGUF v2/v3 header — callers fall back to free-form
/// input in that case.
#[must_use]
pub fn read_speech_model_info(path: &Path) -> Option<SpeechModelInfo> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);

    let mut magic = [0_u8; 4];
    reader.read_exact(&mut magic).ok()?;
    if magic != GGUF_MAGIC {
        return None;
    }
    let version = read_u32(&mut reader)?;
    if !(2..=3).contains(&version) {
        return None;
    }
    let _tensor_count = read_u64(&mut reader)?;
    let kv_count = read_u64(&mut reader)?;
    if kv_count > MAX_KV_COUNT {
        return None;
    }

    let mut info = SpeechModelInfo::default();
    let mut budget = MAX_TOTAL_STRING_BYTES;
    let mut found = 0_u8;
    for _ in 0..kv_count {
        let key = read_string(&mut reader, &mut budget)?;
        let value_type = read_u32(&mut reader)?;
        match (key.as_str(), value_type) {
            ("general.languages", TYPE_ARRAY) => {
                info.languages = read_string_array(&mut reader, &mut budget)?;
                found += 1;
            }
            ("stt.translation.target_languages", TYPE_ARRAY) => {
                info.translate_targets = read_string_array(&mut reader, &mut budget)?;
                found += 1;
            }
            ("stt.capability.translate", TYPE_BOOL) => {
                info.supports_translate = Some(read_u8(&mut reader)? != 0);
                found += 1;
            }
            ("stt.capability.lang_detect", TYPE_BOOL) => {
                info.supports_lang_detect = Some(read_u8(&mut reader)? != 0);
                found += 1;
            }
            ("stt.translation.pairs", TYPE_ARRAY) => {
                info.translate_pairs = read_string_array(&mut reader, &mut budget)?
                    .into_iter()
                    .filter_map(|pair| {
                        pair.split_once('>')
                            .map(|(from, to)| (from.trim().to_string(), to.trim().to_string()))
                    })
                    .collect();
                found += 1;
            }
            (_, value_type) => skip_value(&mut reader, value_type)?,
        }
        if found == 5 {
            break;
        }
    }
    Some(info)
}

fn read_u8(reader: &mut impl Read) -> Option<u8> {
    let mut buffer = [0_u8; 1];
    reader.read_exact(&mut buffer).ok()?;
    Some(buffer[0])
}

fn read_u32(reader: &mut impl Read) -> Option<u32> {
    let mut buffer = [0_u8; 4];
    reader.read_exact(&mut buffer).ok()?;
    Some(u32::from_le_bytes(buffer))
}

fn read_u64(reader: &mut impl Read) -> Option<u64> {
    let mut buffer = [0_u8; 8];
    reader.read_exact(&mut buffer).ok()?;
    Some(u64::from_le_bytes(buffer))
}

fn read_string(reader: &mut impl Read, budget: &mut u64) -> Option<String> {
    let length = read_u64(reader)?;
    if length > MAX_STRING_LEN || length > *budget {
        return None;
    }
    *budget -= length;
    let mut buffer = vec![0_u8; usize::try_from(length).ok()?];
    reader.read_exact(&mut buffer).ok()?;
    String::from_utf8(buffer).ok()
}

fn read_string_array(reader: &mut impl Read, budget: &mut u64) -> Option<Vec<String>> {
    let element_type = read_u32(reader)?;
    let count = read_u64(reader)?;
    if element_type != TYPE_STRING || count > MAX_ARRAY_LEN {
        return None;
    }
    let mut values = Vec::with_capacity(usize::try_from(count.min(1024)).ok()?);
    for _ in 0..count {
        values.push(read_string(reader, budget)?);
    }
    Some(values)
}

fn skip_bytes(reader: &mut impl Read, count: u64) -> Option<()> {
    let skipped = std::io::copy(&mut reader.take(count), &mut std::io::sink()).ok()?;
    (skipped == count).then_some(())
}

fn scalar_size(value_type: u32) -> Option<u64> {
    match value_type {
        TYPE_UINT8 | TYPE_INT8 | TYPE_BOOL => Some(1),
        TYPE_UINT16 | TYPE_INT16 => Some(2),
        TYPE_UINT32 | TYPE_INT32 | TYPE_FLOAT32 => Some(4),
        TYPE_UINT64 | TYPE_INT64 | TYPE_FLOAT64 => Some(8),
        _ => None,
    }
}

fn skip_value(reader: &mut impl Read, value_type: u32) -> Option<()> {
    if let Some(size) = scalar_size(value_type) {
        return skip_bytes(reader, size);
    }
    match value_type {
        TYPE_STRING => {
            let length = read_u64(reader)?;
            if length > MAX_STRING_LEN {
                return None;
            }
            skip_bytes(reader, length)
        }
        TYPE_ARRAY => {
            let element_type = read_u32(reader)?;
            let count = read_u64(reader)?;
            if count > MAX_ARRAY_LEN {
                return None;
            }
            if let Some(size) = scalar_size(element_type) {
                return skip_bytes(reader, count.checked_mul(size)?);
            }
            match element_type {
                TYPE_STRING => {
                    for _ in 0..count {
                        let length = read_u64(reader)?;
                        if length > MAX_STRING_LEN {
                            return None;
                        }
                        skip_bytes(reader, length)?;
                    }
                    Some(())
                }
                // Nested arrays do not occur in transcribe.cpp GGUFs.
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{SpeechModelInfo, read_speech_model_info};

    fn push_string(out: &mut Vec<u8>, value: &str) {
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }

    fn push_kv_string_array(out: &mut Vec<u8>, key: &str, values: &[&str]) {
        push_string(out, key);
        out.extend_from_slice(&9_u32.to_le_bytes()); // ARRAY
        out.extend_from_slice(&8_u32.to_le_bytes()); // of STRING
        out.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for value in values {
            push_string(out, value);
        }
    }

    fn synthetic_gguf() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3_u32.to_le_bytes()); // version
        out.extend_from_slice(&0_u64.to_le_bytes()); // tensor count
        out.extend_from_slice(&5_u64.to_le_bytes()); // kv count

        // Unrelated scalar key that must be skipped.
        push_string(&mut out, "general.file_type");
        out.extend_from_slice(&4_u32.to_le_bytes()); // UINT32
        out.extend_from_slice(&1_u32.to_le_bytes());

        push_kv_string_array(&mut out, "general.languages", &["no", "nn", "en"]);

        // Unrelated string key.
        push_string(&mut out, "general.name");
        out.extend_from_slice(&8_u32.to_le_bytes()); // STRING
        push_string(&mut out, "NB-Whisper Large");

        push_string(&mut out, "stt.capability.translate");
        out.extend_from_slice(&7_u32.to_le_bytes()); // BOOL
        out.push(1);

        push_kv_string_array(&mut out, "stt.translation.target_languages", &["en"]);
        out
    }

    #[test]
    fn reads_speech_keys_from_synthetic_gguf() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(&synthetic_gguf()).expect("write gguf");
        let info = read_speech_model_info(file.path()).expect("parse");
        assert_eq!(
            info,
            SpeechModelInfo {
                languages: vec!["no".into(), "nn".into(), "en".into()],
                supports_translate: Some(true),
                supports_lang_detect: None,
                translate_targets: vec!["en".into()],
                translate_pairs: Vec::new(),
            }
        );
    }

    #[test]
    fn absent_capability_keys_stay_none() {
        // A header carrying only file_type + languages: capability fields
        // must stay None (family default), not become false.
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3_u32.to_le_bytes());
        out.extend_from_slice(&0_u64.to_le_bytes());
        out.extend_from_slice(&2_u64.to_le_bytes());
        push_string(&mut out, "general.file_type");
        out.extend_from_slice(&4_u32.to_le_bytes()); // UINT32
        out.extend_from_slice(&1_u32.to_le_bytes());
        push_kv_string_array(&mut out, "general.languages", &["no", "nn", "en"]);

        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(&out).expect("write gguf");
        let info = read_speech_model_info(file.path()).expect("parse");
        assert_eq!(info.supports_translate, None);
        assert_eq!(info.supports_lang_detect, None);
        assert_eq!(info.languages.len(), 3);
    }

    #[test]
    fn rejects_headers_exceeding_the_string_budget() {
        // One KV claiming a 9 MiB string exceeds the 8 MiB total budget.
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&3_u32.to_le_bytes());
        out.extend_from_slice(&0_u64.to_le_bytes());
        out.extend_from_slice(&1_u64.to_le_bytes());
        out.extend_from_slice(&(9_u64 * 1024 * 1024).to_le_bytes()); // key length
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(&out).expect("write gguf");
        assert!(read_speech_model_info(file.path()).is_none());
    }

    #[test]
    fn translation_pairs_filter_targets_by_source() {
        let info = SpeechModelInfo {
            translate_targets: vec!["en".into(), "de".into()],
            translate_pairs: vec![
                ("no".into(), "en".into()),
                ("en".into(), "de".into()),
                ("en".into(), "no".into()),
            ],
            ..SpeechModelInfo::default()
        };
        assert_eq!(info.targets_for_source("no"), vec!["en".to_string()]);
        assert_eq!(info.targets_for_source("en"), vec!["de".to_string(), "no".to_string()]);
        assert_eq!(info.targets_for_source("auto").len(), 3);
        assert!(info.targets_for_source("de").is_empty());

        let unrestricted = SpeechModelInfo {
            translate_targets: vec!["en".into()],
            ..SpeechModelInfo::default()
        };
        assert_eq!(unrestricted.targets_for_source("no"), vec!["en".to_string()]);
    }

    #[test]
    fn rejects_non_gguf_files() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        file.write_all(b"definitely not a gguf").expect("write");
        assert!(read_speech_model_info(file.path()).is_none());
    }
}
