use super::*;

#[test]
fn metadata_separates_structural_and_behavior_contract_hashes() {
    let metadata = catalog_metadata();
    assert_eq!(metadata.catalog_hash, catalog_hash());
    assert_eq!(metadata.instructions_hash, instructions_hash());
    assert_eq!(
        metadata.instructions_version,
        server_contract::instructions::INSTRUCTIONS_VERSION
    );
    assert!(metadata.instructions_hash.starts_with("sha256:"));
    assert_ne!(metadata.instructions_hash, metadata.catalog_hash);
}

#[test]
fn workspace_read_image_description_preserves_direct_vision_usage() {
    let tool = McpServer::tool_router()
        .list_all()
        .into_iter()
        .find(|tool| tool.name == "workspace_read_image")
        .expect("workspace_read_image tool");
    let description = tool.description.as_deref().unwrap_or_default();
    assert!(description.contains("Call directly with path"));
    assert!(description.contains("never wrap this tool in code or stringify/summarize"));
    assert!(description.contains("pass its image_path"));
}

#[test]
fn behavior_wording_changes_only_the_instruction_hash_channel() {
    let first = serde_json::json!({"description": "first", "version": 1});
    let second = serde_json::json!({"description": "second", "version": 1});
    assert_ne!(
        tool_catalog::hash_instruction_value(&first),
        tool_catalog::hash_instruction_value(&second)
    );
    assert_eq!(
        tool_catalog::hash_manifest_value(&first),
        tool_catalog::hash_manifest_value(&second)
    );
}

#[test]
fn consent_and_git_preview_fields_are_additive_and_safe_by_default() {
    let question: PlanQuestionArgs = serde_json::from_value(serde_json::json!({
        "question": "Proceed?", "options": ["yes", "no"]
    }))
    .expect("legacy clarification args");
    assert!(matches!(
        question.question_kind,
        PlanQuestionKindArgs::Clarification
    ));
    let commit: GitCommitArgs = serde_json::from_value(serde_json::json!({
        "message": "scoped", "paths": ["selected.txt"]
    }))
    .expect("scoped commit args");
    assert!(!commit.all);
    assert!(!commit.preview_only);
    assert!(commit.expected_preview.is_none());
    let schema =
        serde_json::to_value(schemars::schema_for!(GitCommitArgs)).expect("git commit schema");
    assert!(schema["properties"].get("previewOnly").is_some());
    assert!(schema["properties"].get("expectedPreview").is_some());
}

#[test]
fn execution_run_schema_and_authorization_metadata_are_explicit() {
    assert!(TOOL_NAMES.iter().any(|name| name == "execution_run"));
    let capabilities = tool_capabilities("execution_run");
    assert_eq!(
        capabilities.operation_class,
        ToolOperationClass::ProcessExecution
    );
    assert_eq!(capabilities.risk_class, ToolRiskClass::ProcessExecution);
    assert!(capabilities.approval_required);
    assert!(capabilities.mutating);
    assert_eq!(capabilities.path_fields, vec![PathFieldRole::Cwd]);
    let schema = serde_json::to_value(schemars::schema_for!(CommandRunArgs))
        .expect("execution_run input schema");
    assert!(schema["required"].as_array().is_some_and(|required| {
        required.iter().any(|field| field == "executable")
            && required.iter().any(|field| field == "cwd")
    }));
    assert!(schema["properties"].get("arguments").is_some());
    assert!(schema["properties"].get("timeoutMs").is_some());
    assert!(canonical_manifest().to_string().contains("terminalState"));
}

#[test]
fn executable_contract_examples_deserialize_and_match_advertised_schemas() {
    let examples: Value = serde_json::from_str(include_str!(
        "../tests/coding_fixtures/contract_examples.json"
    ))
    .expect("contract examples JSON");
    serde_json::from_value::<CommandRunArgs>(examples["commandRun"].clone())
        .expect("execution_run example");
    serde_json::from_value::<ProjectContextArgs>(examples["projectContext"].clone())
        .expect("project_context example");
    serde_json::from_value::<GitCommitArgs>(examples["gitPreview"].clone())
        .expect("git preview example");
    serde_json::from_value::<PlanQuestionArgs>(examples["executionConsent"].clone())
        .expect("execution consent example");
    serde_json::from_value::<CompleteArgs>(examples["completion"].clone())
        .expect("completion example");
    serde_json::from_value::<chatcmd_runtime::GitCommitPreview>(
        examples["gitPreviewResult"].clone(),
    )
    .expect("git preview result");
    serde_json::from_value::<chatcmd_runtime::ProjectContextBundle>(
        examples["projectContextResult"].clone(),
    )
    .expect("project context result");

    let error = server_contract::error_value(&RuntimeError::new(
        "execution_not_found",
        "execution evidence was not found",
    ));
    assert_eq!(error, examples["completionError"]);
    assert!(
        canonical_manifest()["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|tool| !tool["resultSchema"].is_null())
    );
}

#[tokio::test]
async fn project_rule_digest_changes_without_requiring_catalog_reconnect() {
    let fixture = tempfile::tempdir().expect("fixture");
    std::fs::write(fixture.path().join("AGENTS.md"), "first").expect("first rule");
    let catalog_before = catalog_hash();
    let service = chatcmd_runtime::ProjectContextService::default();
    let first = service
        .load(fixture.path(), &[])
        .await
        .expect("first context");
    std::fs::write(fixture.path().join("AGENTS.md"), "second").expect("second rule");
    let second = service
        .load(fixture.path(), &[])
        .await
        .expect("second context");
    assert_ne!(first.effective_hash, second.effective_hash);
    assert_eq!(catalog_before, catalog_hash());
}

#[test]
fn catalog_mismatch_has_reconnect_metadata_and_bounded_recovery() {
    let arguments = ToolArguments {
        client_catalog_hash: Some("sha256:v7-cached".to_owned()),
        ..ToolArguments::default()
    };
    let result = catalog_mismatch(&arguments).expect("cached v7 must mismatch");
    let value = result.structured_content.expect("structured mismatch");
    assert_eq!(value["catalogVersion"], CATALOG_VERSION);
    assert_eq!(value["error"]["recovery"], "refreshAndRetry");
    assert_eq!(value["reconnect"]["maxAttempts"], 1);
}

#[test]
fn image_generation_tool_is_a_path_scoped_mutation() {
    let flags = tool_capabilities("generate_image");
    assert_eq!(flags.operation_class, ToolOperationClass::Mutation);
    assert_eq!(flags.risk_class, ToolRiskClass::Modify);
    assert!(flags.approval_required);
    assert!(flags.mutating);
    assert_eq!(flags.path_fields, vec![PathFieldRole::Path]);
    let tool = McpServer::tool_router()
        .list_all()
        .into_iter()
        .find(|tool| tool.name == "generate_image")
        .expect("generate_image tool");
    let schema = serde_json::to_value(&tool.input_schema).expect("schema");
    let properties = &schema["properties"];
    for field in ["path", "prompt", "jobId", "overwrite", "waitMs", "model"] {
        assert!(properties.get(field).is_some(), "missing {field}");
    }
    let description = tool.description.as_deref().unwrap_or_default();
    assert!(description.contains("jobId"));
    // Keep the model-facing contract deployment-neutral while preserving the real effect.
    for required in [
        "connected image provider",
        "authorized destination",
        "status=running is not completion",
    ] {
        assert!(
            description.contains(required),
            "missing image operation fact: {required}"
        );
    }
    let exposed =
        format!("{description}\n{}", serde_json::to_string(&schema).unwrap()).to_ascii_lowercase();
    for forbidden in [
        "astra-local",
        "astra_local",
        "this computer",
        "your computer",
        "chatgpt.com",
        "chrome",
        "browser",
        "signed-in",
        "chrome extension",
    ] {
        assert!(
            !exposed.contains(forbidden),
            "model-facing image tool surface leaked {forbidden}"
        );
    }
    assert!(
        !TOOL_NAMES
            .iter()
            .any(|name| name == "workspace_generate_image")
    );
}

#[test]
fn public_catalog_uses_current_names_and_deployment_neutral_descriptions() {
    let tools = McpServer::tool_router().list_all();
    assert!(!tools.is_empty());
    for tool in tools {
        let description = tool.description.as_deref().unwrap_or_default();
        let exposed = format!(
            "{}\n{description}\n{}",
            tool.name,
            serde_json::to_string(&tool.input_schema).expect("public schema")
        )
        .to_ascii_lowercase();
        for obsolete in [
            "astra-local",
            "astra_local",
            "this computer",
            "your computer",
            "local filesystem",
            "local process",
            "local machine",
            "pty session",
        ] {
            assert!(
                !exposed.contains(obsolete),
                "{} contains obsolete label {obsolete}",
                tool.name
            );
        }
        for word in description.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            assert!(
                !["fs_", "shell_", "git_", "process_"]
                    .iter()
                    .any(|prefix| word.starts_with(prefix))
                    && word != "command_run",
                "{} description references unexposed internal operation {word}",
                tool.name
            );
        }
    }
}

#[test]
fn public_names_keep_existing_runtime_authorization_dispatch() {
    for (public, internal) in [
        ("execution_run", "command_run"),
        ("execution_process_stop", "process_kill"),
        ("workspace_write_text", "fs_write_text"),
        ("workspace_delete", "fs_delete"),
        ("generate_image", "fs_write_chatgpt_image"),
    ] {
        assert_eq!(runtime_tool_name(public), internal);
        let public_flags = tool_capabilities(public);
        let internal_flags = tool_capabilities(internal);
        assert_eq!(public_flags.operation_class, internal_flags.operation_class);
        assert_eq!(public_flags.risk_class, internal_flags.risk_class);
        assert_eq!(
            public_flags.approval_required,
            internal_flags.approval_required
        );
        assert_eq!(public_flags.mutating, internal_flags.mutating);
    }
}
