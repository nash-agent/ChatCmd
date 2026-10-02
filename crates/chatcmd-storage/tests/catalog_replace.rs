use chatcmd_core::{
    McpAgentStore, NewMcpAgent, ToolCapability, ToolCatalogStore, ToolDefinition, ToolGroup,
    ToolPreset,
};
use chatcmd_storage::SqliteRepository;
use tempfile::TempDir;

async fn repository(directory: &TempDir) -> SqliteRepository {
    SqliteRepository::open(&directory.path().join("chatcmd.db"), 8)
        .await
        .expect("open repository")
        .0
}

fn tool(id: &str, key: &str) -> ToolDefinition {
    ToolDefinition {
        id: id.to_owned(),
        key: key.to_owned(),
        group_id: "group-workspace".to_owned(),
        title: key.to_owned(),
        description: "test tool".to_owned(),
        input_schema_json: "{}".to_owned(),
        capabilities: vec![ToolCapability::Read],
        enabled: true,
    }
}

#[tokio::test]
async fn catalog_key_collision_migrates_references_to_canonical_tool_id() {
    let directory = TempDir::new().expect("temporary directory");
    let repository = repository(&directory).await;
    let group = ToolGroup {
        id: "group-workspace".to_owned(),
        key: "workspace".to_owned(),
        display_name: "Workspace".to_owned(),
        sort_order: 1,
    };
    let canonical = tool("tool-fs-read", "fs_read_text");
    let conflicting = tool("tool-workspace-read-text", "workspace_read_text");
    let preset = ToolPreset {
        id: "preset-test".to_owned(),
        key: "preset-test".to_owned(),
        name: "Test".to_owned(),
        description: "test preset".to_owned(),
        tool_ids: vec![conflicting.id.clone()],
    };

    repository
        .replace_catalog(
            std::slice::from_ref(&group),
            &[canonical.clone(), conflicting.clone()],
            std::slice::from_ref(&preset),
        )
        .await
        .expect("seed old catalog");

    let agent = repository
        .create_agent(NewMcpAgent {
            id: None,
            name: "Catalog migration agent".to_owned(),
            enabled: true,
        })
        .await
        .expect("create agent")
        .agent;
    repository
        .set_agent_allowed_tools(&agent.id, std::slice::from_ref(&conflicting.id))
        .await
        .expect("assign old tool");

    let renamed = tool(&canonical.id, &conflicting.key);
    repository
        .replace_catalog(
            std::slice::from_ref(&group),
            std::slice::from_ref(&renamed),
            &[],
        )
        .await
        .expect("replace catalog across key collision");

    let tools = repository.list_tools().await.expect("list tools");
    assert!(tools.iter().any(|tool| tool == &renamed));
    assert!(!tools.iter().any(|tool| tool.id == conflicting.id));

    let allowed = repository
        .agent_allowed_tool_ids(&agent.id)
        .await
        .expect("read migrated allowlist");
    assert_eq!(allowed, vec![canonical.id.clone()]);

    let presets = repository.list_presets().await.expect("list presets");
    let migrated = presets
        .iter()
        .find(|candidate| candidate.id == preset.id)
        .expect("preset remains");
    assert_eq!(migrated.tool_ids, vec![canonical.id]);
}
