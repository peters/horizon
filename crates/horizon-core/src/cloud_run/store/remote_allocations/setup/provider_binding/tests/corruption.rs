use super::*;

fn corrupt(fixture: &Fixture, sql: &str) {
    let connection = fixture.raw();
    let definitions: Vec<String> = connection.prepare(
        "SELECT sql FROM sqlite_schema WHERE name IN ('remote_provider_bindings_no_update', 'remote_provider_bindings_no_replace') ORDER BY name"
    ).expect("query").query_map([], |row| row.get(0)).expect("definitions")
        .collect::<Result<_, _>>().expect("triggers");
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;
        DROP TRIGGER remote_provider_bindings_no_update; DROP TRIGGER remote_provider_bindings_no_replace;",
        )
        .expect("synthetic corruption admission");
    connection.execute_batch(sql).expect("corrupt row");
    for definition in definitions {
        connection.execute_batch(&definition).expect("restore exact schema");
    }
    connection
        .execute_batch("PRAGMA foreign_keys=ON; PRAGMA ignore_check_constraints=OFF")
        .expect("restore connection guards");
}

#[test]
fn malformed_rows_are_errors_not_absence_or_backfill_and_never_echo_payloads() {
    let cases = [
        "workspace_local_id = 'foreign'",
        "session_id = '22222222-2222-4222-8222-222222222222'",
        "generation = generation + 1",
        "workflow_id = '33333333-3333-4333-8333-333333333333'",
        "job_id = '44444444-4444-4444-8444-444444444444'",
        "version = 2",
        "provider = 'run_pod'",
        "subscription_id = 'invalid-sensitive-value'",
        "subscription_id = subscription_id || char(0)",
        "subscription_id = 'AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA'",
        "subscription_id = replace(hex(zeroblob(65536)), '0', 's')",
        "profile_digest = ''",
        "profile_digest = upper(profile_digest)",
        "profile_digest = profile_digest || char(0)",
        "profile_digest = replace(hex(zeroblob(65536)), '0', 'a')",
    ];
    for assignment in cases {
        let fixture = Fixture::new();
        fixture.record();
        corrupt(&fixture, &format!("UPDATE remote_provider_bindings SET {assignment}"));
        let before = fixture.snapshots();
        let error = fixture
            .store
            .load_remote_cpu_profile_binding(&fixture.saved)
            .expect_err("corrupt binding");
        assert!(!format!("{error:?}").contains("invalid-sensitive-value"));
        assert!(
            fixture
                .store
                .record_remote_cpu_profile_binding(&fixture.saved, &profile())
                .is_err()
        );
        assert_eq!(fixture.count(), 1);
        assert_eq!(fixture.snapshots(), before);
    }
}

#[test]
fn workspace_workflow_and_job_collisions_fail_with_the_exact_schema_restored() {
    for column in ["workflow_id", "job_id"] {
        let fixture = Fixture::new();
        fixture.record();
        let runtime = fixture.saved.workspace().state().runtime.as_ref().expect("runtime");
        let id = if column == "workflow_id" {
            runtime.workflow_id.to_string()
        } else {
            runtime.job_id.to_string()
        };
        // Distinct unique columns can still form a conflicting OR lookup after
        // corruption: one row owns workspace, another owns its original workflow/job.
        corrupt(
            &fixture,
            &format!(
                "UPDATE remote_provider_bindings SET {column} = '33333333-3333-4333-8333-333333333333';
             INSERT INTO remote_provider_bindings SELECT 'foreign', session_id, generation,
             '44444444-4444-4444-8444-444444444444', '55555555-5555-4555-8555-555555555555',
             version, provider, subscription_id, profile_digest FROM remote_provider_bindings;
             UPDATE remote_provider_bindings SET {column} = '{id}' WHERE workspace_local_id = 'foreign';"
            ),
        );
        assert_eq!(fixture.count(), 2);
        assert!(fixture.store.load_remote_cpu_profile_binding(&fixture.saved).is_err());
        assert!(
            fixture
                .store
                .record_remote_cpu_profile_binding(&fixture.saved, &profile())
                .is_err()
        );
    }
}

#[test]
fn legacy_schemas_return_none_without_migration_backfill_or_snapshot_changes() {
    for version in [4, 5, 6] {
        let fixture = Fixture::new();
        let connection = fixture.raw();
        connection
            .execute_batch("DROP TABLE remote_provider_bindings")
            .expect("legacy");
        if version <= 5 {
            connection
                .execute_batch("DROP TABLE remote_network_volume_selections")
                .expect("legacy volume");
        }
        if version == 4 {
            connection
                .execute_batch("DROP TABLE remote_first_pin_intents")
                .expect("legacy pin");
        }
        connection
            .pragma_update(None, "user_version", version)
            .expect("version");
        let before = fixture.snapshots();
        let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
        assert_eq!(
            reader
                .load_remote_cpu_profile_binding(&fixture.saved)
                .expect("legacy absence"),
            None
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            version
        );
        let tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'remote_provider_bindings'",
                [],
                |row| row.get(0),
            )
            .expect("absence");
        assert_eq!(tables, 0);
        assert_eq!(fixture.snapshots(), before);
    }
}

#[test]
fn read_does_not_create_a_missing_database_or_accept_a_non_cpu_binding() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.store.path()).expect("remove owned synthetic database");
    assert!(fixture.store.load_remote_cpu_profile_binding(&fixture.saved).is_err());
    assert!(!fixture.store.path().exists());

    let other = Fixture::with_target(|target| target.provider = CloudProvider::LocalDocker);
    let runtime = other.saved.workspace().state().runtime.as_ref().expect("runtime");
    other
        .raw()
        .execute(
            "INSERT INTO remote_provider_bindings VALUES (?1, ?2, ?3, ?4, ?5, 1, 'azure', ?6, ?7)",
            params![
                "workspace",
                OWNER,
                i64::try_from(runtime.generation).expect("generation"),
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string(),
                SUBSCRIPTION,
                "a".repeat(64)
            ],
        )
        .expect("malformed association fixture");
    assert!(other.store.load_remote_cpu_profile_binding(&other.saved).is_err());
}
