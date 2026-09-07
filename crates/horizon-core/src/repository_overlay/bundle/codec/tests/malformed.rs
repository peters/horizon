use super::*;

#[test]
fn every_truncated_prefix_and_a_trailing_byte_are_rejected() {
    let encoded = encode(&one_file()).expect("encoding");
    for length in 0..encoded.len() {
        assert!(decode(&encoded[..length]).is_err(), "prefix {length}");
    }
    let mut trailing = encoded.into_vec();
    trailing.push(0);
    assert_eq!(decode(&trailing), Err(OverlayCodecError::NonCanonical));
}

#[test]
fn unknown_magic_and_format_or_metadata_versions_fail() {
    for position in 0..8 {
        let mut encoded = encode(&one_file()).expect("encoding").into_vec();
        encoded[position] ^= 1;
        assert_eq!(decode(&encoded), Err(OverlayCodecError::Unsupported));
    }
    for changed in [
        changed_metadata(&one_file(), |metadata| {
            metadata.replace("\"version\":1", "\"version\":2")
        }),
        changed_metadata(&one_file(), |metadata| {
            metadata.replace("horizon.repository-overlay", "other.domain")
        }),
    ] {
        assert_eq!(decode(&changed), Err(OverlayCodecError::Unsupported));
    }
}

#[test]
fn oversized_metadata_and_count_headers_fail_without_allocating_claimed_sizes() {
    let mut encoded = MAGIC.to_vec();
    encoded.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode(&encoded), Err(OverlayCodecError::Limit));
    let encoded = encode(&one_file()).expect("encoding");
    let metadata = &encoded[12..metadata_end(&encoded)];
    assert_eq!(decode(&frame(metadata, u32::MAX, &[])), Err(OverlayCodecError::Limit));
    assert_eq!(decode(&frame(metadata, 0, &[])), Err(OverlayCodecError::Malformed));
    assert_eq!(decode(&frame(metadata, 2, &[])), Err(OverlayCodecError::Malformed));
}

#[test]
fn invalid_json_duplicate_unknown_missing_and_wrong_variant_fields_fail() {
    let original = one_file();
    let encoded = encode(&original).expect("encoding");
    let metadata = String::from_utf8(encoded[12..metadata_end(&encoded)].to_vec()).expect("metadata");
    for invalid in [
        String::from("[1,2,3]"),
        String::from("not-json"),
        metadata.replacen('{', "{\"extra\":true,", 1),
        metadata.replacen('{', "{\"version\":1,", 1),
        metadata.replace("\"path\":\"selected\"", "\"path\":\"selected\",\"path\":\"selected\""),
        metadata.replace("\"kind\":\"file\"", "\"kind\":\"file\",\"kind\":\"file\""),
        metadata.replace("\"kind\":\"file\"", "\"kind\":\"file\",\"extra\":true"),
        metadata.replace("\"path\":\"selected\",", ""),
        metadata.replace("\"executable\":true", "\"executable\":null"),
        metadata.replace("\"executable\":true", "\"executable\":\"true\""),
        metadata.replace("\"kind\":\"file\"", "\"kind\":\"unknown\""),
        metadata.replace("\"kind\":\"file\"", "\"kind\":\"remove\""),
        metadata.replace("\"kind\":\"file\"", "\"kind\":\"symlink\""),
        metadata.replace("\"kind\":\"file\"", "\"kind\":\"file\",\"target\":\"elsewhere\""),
        metadata.replace("\"bytes\":10", "\"bytes\":-1"),
    ] {
        assert!(decode(&frame(invalid.as_bytes(), 1, &encoded[metadata_end(&encoded) + 4..])).is_err());
    }
}

#[test]
fn semantically_equivalent_but_noncanonical_metadata_is_rejected() {
    for changed in [
        changed_metadata(&one_file(), |metadata| format!(" {metadata}")),
        changed_metadata(&one_file(), |metadata| metadata.replace("\"branch\":null,", "")),
        changed_metadata(&one_file(), |metadata| {
            metadata.replace("\"path\":\"selected\"", "\"path\":\"\\u0073elected\"")
        }),
        changed_metadata(&one_file(), |metadata| {
            metadata.replace("\"kind\":\"file\"", "\"kind\":\"file\",\"target\":null")
        }),
        changed_metadata(&one_file(), |metadata| {
            let hash = ArtifactDigest::sha256(b"literal\0\xff\n");
            metadata.replace(hash.as_str(), &hash.as_str().to_uppercase())
        }),
    ] {
        assert_eq!(decode(&changed), Err(OverlayCodecError::NonCanonical));
    }
}

#[test]
fn source_path_link_and_metadata_limits_are_revalidated() {
    for changed in [
        changed_metadata(&one_file(), |metadata| {
            metadata.replace("team/repo", "https://user:password@example.test/repo")
        }),
        changed_metadata(&one_file(), |metadata| metadata.replace("selected", "../escape")),
        changed_metadata(&one_file(), |metadata| metadata.replace("selected", ".env")),
        changed_metadata(&one_file(), |metadata| metadata.replace("selected", &"a".repeat(4097))),
        changed_metadata(&one_file(), |metadata| {
            metadata.replace("\"branch\":null", "\"branch\":\"../bad\"")
        }),
    ] {
        assert!(matches!(decode(&changed), Err(OverlayCodecError::Plan(_))));
    }
    let link = bundle(
        vec![],
        vec![
            OverlayChange::new(
                "link".into(),
                OverlayContent::Symlink {
                    target: "allowed".into(),
                },
            )
            .expect("link"),
        ],
        &[],
    );
    let changed = changed_metadata(&link, |metadata| metadata.replace("allowed", "../../escape"));
    assert_eq!(
        decode(&changed),
        Err(OverlayCodecError::Plan(OverlayPlanError::InvalidLink))
    );
}

#[test]
fn declared_oversized_files_fail_before_payload_records_are_read() {
    let oversized = changed_metadata(&one_file(), |metadata| {
        metadata.replace("\"bytes\":10", "\"bytes\":67108865")
    });
    let end = metadata_end(&oversized);
    assert_eq!(
        decode(&oversized[..end]),
        Err(OverlayCodecError::Bundle(OverlayBundleError::FileLimit))
    );
}

#[test]
fn forged_record_lengths_hashes_and_corrupted_payloads_fail() {
    let encoded = encode(&one_file()).expect("encoding");
    let record = metadata_end(&encoded) + 4;
    for length in [0u64, 9, 11, u64::MAX] {
        let mut changed = encoded.to_vec();
        changed[record + 64..record + 72].copy_from_slice(&length.to_le_bytes());
        assert!(decode(&changed).is_err());
    }
    for offset in [record, record + 63, record + 72] {
        let mut changed = encoded.to_vec();
        changed[offset] ^= 1;
        assert!(decode(&changed).is_err());
    }
    let mut uppercase = encoded.to_vec();
    uppercase[record..record + 64].make_ascii_uppercase();
    assert!(decode(&uppercase).is_err());
}

#[test]
fn duplicate_and_reordered_blob_records_cannot_bypass_canonical_membership() {
    let original = bundle(
        vec![file("a", b"a", false), file("b", b"b", false)],
        vec![],
        &[b"a", b"b"],
    );
    let encoded = encode(&original).expect("encoding");
    let start = metadata_end(&encoded) + 4;
    let first = &encoded[start..start + 73];
    let second = &encoded[start + 73..];
    for records in [[first, first].concat(), [second, first].concat()] {
        assert_eq!(
            decode(&frame(&encoded[12..start - 4], 2, &records)),
            Err(OverlayCodecError::NonCanonical)
        );
    }
}

#[test]
fn decoder_caps_layer_entries_before_growing_unbounded_collections() {
    let changes = (0..=MAX_CHANGES)
        .map(|index| format!("{{\"path\":\"file-{index:05}\",\"kind\":\"remove\"}}"))
        .collect::<Vec<_>>()
        .join(",");
    let empty = bundle(vec![], vec![], &[]);
    let oversized = changed_metadata(&empty, |metadata| {
        metadata.replace("\"index\":[]", &format!("\"index\":[{changes}]"))
    });
    assert_eq!(decode(&oversized), Err(OverlayCodecError::Malformed));
}

#[test]
fn exact_change_limit_is_accepted_but_the_shared_layer_budget_is_enforced() {
    let changes = (0..MAX_CHANGES)
        .map(|index| format!("{{\"path\":\"file-{index:05}\",\"kind\":\"remove\"}}"))
        .collect::<Vec<_>>()
        .join(",");
    let empty = bundle(vec![], vec![], &[]);
    let exact = changed_metadata(&empty, |metadata| {
        metadata.replace("\"index\":[]", &format!("\"index\":[{changes}]"))
    });
    let decoded = decode(&exact).expect("exact entry boundary");
    assert_eq!(decoded.plan().index().len(), MAX_CHANGES);
    let exceeded = changed_metadata(&decoded, |metadata| {
        metadata.replace(
            "\"working_tree\":[]",
            "\"working_tree\":[{\"path\":\"extra\",\"kind\":\"remove\"}]",
        )
    });
    assert_eq!(
        decode(&exceeded),
        Err(OverlayCodecError::Plan(OverlayPlanError::ChangeLimit))
    );
}

#[test]
fn aggregate_payload_budget_is_checked_before_allocating_or_reading_records() {
    let original = bundle(
        vec![file("a", b"a", false), file("b", b"b", false), file("c", b"c", false)],
        vec![],
        &[b"a", b"b", b"c"],
    );
    let oversized = changed_metadata(&original, |metadata| {
        metadata.replace("\"bytes\":1", "\"bytes\":67108864")
    });
    assert_eq!(
        decode(&oversized[..metadata_end(&oversized)]),
        Err(OverlayCodecError::Bundle(OverlayBundleError::BundleLimit))
    );
}

#[test]
fn malformed_input_diagnostics_are_redacted() {
    let malformed = changed_metadata(&one_file(), |metadata| {
        metadata.replace("team/repo", "private-secret-value")
    });
    let error = decode(&malformed).expect_err("invalid identity");
    let diagnostic = format!("{error} {error:?}");
    for private in ["private-secret-value", "selected", "literal", "team/repo"] {
        assert!(!diagnostic.contains(private));
    }
}
