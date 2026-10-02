use chatcmd_storage::{CURRENT_SCHEMA_VERSION, SqliteRepository};

#[tokio::test]
async fn schema_26_projects_upgrade_without_enabling_global_access() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project-access.db");
    let old = SqliteRepository::connect(&path, 1).await.unwrap();
    let mut migrations = sqlx::migrate::Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .await
    .unwrap();
    migrations.migrations = migrations
        .migrations
        .into_owned()
        .into_iter()
        .filter(|migration| migration.version <= 26)
        .collect::<Vec<_>>()
        .into();
    migrations.run(old.pool()).await.unwrap();
    sqlx::query("INSERT INTO workspace_projects(id,name,path,canonical_path,created_at_ms,updated_at_ms) VALUES('existing','Existing','/existing/project','/existing/project',1,1)")
        .execute(old.pool()).await.unwrap();
    old.pool().close().await;
    let (upgraded, report) = SqliteRepository::open(&path, 1).await.unwrap();
    assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    let (enabled,approved): (bool,Option<String>) = sqlx::query_as("SELECT allow_all_conversations,global_access_path FROM workspace_projects WHERE id='existing'")
        .fetch_one(upgraded.pool()).await.unwrap();
    assert!(!enabled);
    assert_eq!(approved, None);
    assert!(
        sqlx::query("UPDATE workspace_projects SET allow_all_conversations=2")
            .execute(upgraded.pool())
            .await
            .is_err()
    );
}
