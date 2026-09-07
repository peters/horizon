use super::*;

#[test]
fn empty_bundle_has_fixed_versioned_framing_and_independent_digest() {
    let bundle = bundle(vec![], vec![], &[]);
    let encoded = encode(&bundle).expect("encoding");
    let metadata = br#"{"domain":"horizon.repository-overlay","version":1,"repository":"team/repo","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","branch":null,"index":[],"working_tree":[]}"#;
    assert_eq!(metadata.len(), 171);
    let mut expected = b"HZOVLY\0\x01\xab\0\0\0".to_vec();
    expected.extend_from_slice(metadata);
    expected.extend_from_slice(&[0; 4]);
    assert_eq!(encoded.as_ref(), expected);
    assert_eq!(encoded.len(), 187);
    assert_eq!(
        ArtifactDigest::sha256(&encoded).as_str(),
        "4ca86fa8c01c45cbfd3becf3252a96a37770f5ed1c8afced48c00e011f268f53"
    );
    assert_eq!(decode(&encoded).expect("decoding"), bundle);
}

#[test]
fn mixed_layers_links_modes_removals_and_literal_bytes_roundtrip() {
    let staged = b"version https://git-lfs.github.com/spec/v1\nliteral-pointer\n";
    let working = b"\0\xff\x80\n";
    let path = "nested/quoted \" ø.txt";
    let original = bundle(
        vec![
            file(path, staged, false),
            OverlayChange::new("gone".into(), OverlayContent::Remove).expect("remove"),
        ],
        vec![
            file(path, working, true),
            file("untracked", b"", false),
            OverlayChange::new("alias".into(), OverlayContent::Symlink { target: path.into() }).expect("link"),
        ],
        &[working, b"", staged],
    );
    let encoded = encode(&original).expect("encoding");
    let decoded = decode(&encoded).expect("decoding");
    assert_eq!(decoded, original);
    assert_eq!(decoded.manifest_sha256(), original.manifest_sha256());
    assert_eq!(encode(&decoded).expect("reencoding"), encoded);
    assert_eq!(decoded.blob(&ArtifactDigest::sha256(working)), Some(working.as_slice()));
}

#[test]
fn shared_payloads_have_one_canonical_record_despite_references_or_input_order() {
    let first = bundle(
        vec![file("z", b"z", false), file("a", b"a", false)],
        vec![file("z", b"z", true)],
        &[b"z", b"a"],
    );
    let second = bundle(
        vec![file("a", b"a", false), file("z", b"z", false)],
        vec![file("z", b"z", true)],
        &[b"a", b"z"],
    );
    let encoded = encode(&first).expect("encoding");
    assert_eq!(encoded, encode(&second).expect("encoding"));
    let end = metadata_end(&encoded);
    assert_eq!(&encoded[end..end + 4], &2u32.to_le_bytes());
    let decoded = decode(&encoded).expect("decoding");
    assert_eq!(decoded.blobs().len(), 2);
    assert_eq!(decoded.file_bytes(), 2);
    assert_eq!(decoded.plan().content_bytes(), 3);
}

#[test]
fn encoding_and_decoding_own_copies_independent_of_other_buffers() {
    let original = one_file();
    let mut encoded = encode(&original).expect("encoding").into_vec();
    let decoded = decode(&encoded).expect("decoding");
    encoded.fill(0);
    assert_eq!(decoded, original);
    assert_ne!(encode(&original).expect("fresh encoding").as_ref(), encoded);
}

#[test]
fn valid_metadata_changes_need_separate_expected_fingerprint_authorization() {
    let original = one_file();
    let changed = changed_metadata(&original, |metadata| metadata.replace("team/repo", "other/repo"));
    let decoded = decode(&changed).expect("valid unsigned metadata");
    assert_ne!(decoded.manifest_sha256(), original.manifest_sha256());
    assert_eq!(
        decoded.blobs().next().expect("blob").bytes(),
        original.blobs().next().expect("blob").bytes()
    );
}

#[test]
fn encoded_size_budget_admits_the_exact_derived_boundary() {
    assert_eq!(
        encoded_length(
            MAX_ENCODED_METADATA_BYTES,
            MAX_CHANGES,
            super::super::super::MAX_BUNDLE_BYTES
        ),
        Ok(MAX_ENCODED_BUNDLE_BYTES)
    );
    for (metadata, count, payloads) in [
        (MAX_ENCODED_METADATA_BYTES + 1, 0, 0),
        (0, MAX_CHANGES + 1, 0),
        (0, 0, super::super::super::MAX_BUNDLE_BYTES + 1),
        (usize::MAX, usize::MAX, usize::MAX),
    ] {
        assert_eq!(encoded_length(metadata, count, payloads), Err(OverlayCodecError::Limit));
    }
}
