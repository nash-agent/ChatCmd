use super::*;
use chatcmd_runtime::RuntimeError;
use serde_json::json;

fn view() -> VirtualWorkspaceView {
    VirtualWorkspaceView::new(
        Some(PathBuf::from(r"R:\fixture\project")),
        vec![
            PathBuf::from(r"R:\fixture\project"),
            PathBuf::from(r"R:\fixture\tools\ChatCMD-src"),
            PathBuf::from(r"R:\fixture\home\.kiro"),
        ],
    )
}

#[test]
fn aliases_hide_host_paths_and_keep_project_special() {
    let view = view();
    assert_eq!(view.aliases(), vec!["@project", "@chatcmd-src", "@kiro"]);
    assert_eq!(
        view.project_path(Path::new(r"R:\fixture\project\Source\Foo.cpp")),
        "@project/Source/Foo.cpp"
    );
    assert_eq!(
        view.project_path(Path::new(r"R:\fixture\tools\ChatCMD-src\src\main.rs")),
        "@chatcmd-src/src/main.rs"
    );
}

#[test]
fn alias_arguments_resolve_without_exposing_parent_traversal() {
    let view = view();
    let resolved = view
        .resolve_alias_arguments("fs_read_text", json!({"path":"@chatcmd-src/src/main.rs"}))
        .expect("resolve alias");
    assert!(
        resolved["path"]
            .as_str()
            .expect("path")
            .ends_with(r"ChatCMD-src\src\main.rs")
    );

    let error = view
        .resolve_alias_arguments("fs_read_text", json!({"path":"@project/../secret"}))
        .expect_err("parent traversal must fail");
    assert_eq!(error.code, "invalid_virtual_path");
}

#[test]
fn read_content_is_preserved_while_path_metadata_is_projected() {
    let view = view();
    let value = view.project_tool_output(
        "fs_read_text",
        json!({
            "path": r"R:\fixture\project\docs\note.txt",
            "content": r"literal R:\fixture\project\inside-file must remain exact"
        }),
    );
    assert_eq!(value["path"], "@project/docs/note.txt");
    assert_eq!(
        value["content"],
        r"literal R:\fixture\project\inside-file must remain exact"
    );
}

#[test]
fn command_output_masks_known_roots_inside_stdout() {
    let view = view();
    let value = view.project_tool_output(
        "command_run",
        json!({"stdout":r"built R:\fixture\project\Binaries\Game.exe"}),
    );
    assert_eq!(value["stdout"], r"built @project\Binaries\Game.exe");
}

#[test]
fn execution_output_masks_case_variant_and_unknown_windows_paths() {
    let view = view();
    let value = view.project_tool_output(
        "command_run",
        json!({
            "cwd": r"r:\FIXTURE\TOOLS\chatcmd-src\src",
            "command": {"executable": r"C:\Windows\System32\cmd.exe"},
            "stdout": r"known r:\FIXTURE\PROJECT\Binaries\Game.exe unknown C:\Windows\Temp\x.txt other E:/scratch/out.bin url https://files.example.com/a.png"
        }),
    );

    assert_eq!(value["cwd"], "@chatcmd-src/src");
    assert_eq!(
        value["command"]["executable"],
        "@runtime/Windows/System32/cmd.exe"
    );
    let stdout = value["stdout"].as_str().expect("stdout");
    assert!(stdout.contains(r"known @project\Binaries\Game.exe"));
    assert!(stdout.contains(r"unknown @runtime/Windows\Temp\x.txt"));
    assert!(stdout.contains("other @runtime/scratch/out.bin"));
    assert!(stdout.contains("https://files.example.com/a.png"));
    assert!(!stdout.contains("C:\\"));
    assert!(!stdout.contains("d:\\"));
    assert!(!stdout.contains("E:/"));
}

#[test]
fn execution_session_event_data_masks_verbatim_windows_paths() {
    let view = view();
    let value = view.project_tool_output(
        "shell_read",
        json!({
            "events": [{
                "data": r"PS Microsoft.PowerShell.Core\FileSystem::\\?\R:\FIXTURE\TOOLS\ChatCMD-src> cd C:\Windows\Temp"
            }]
        }),
    );
    let data = value["events"][0]["data"].as_str().expect("event data");
    assert!(data.contains(r"FileSystem::@chatcmd-src"));
    assert!(data.contains(r"cd @runtime/Windows\Temp"));
    assert!(!data.contains(r"\\?\"));
    assert!(!data.contains("C:\\"));
    assert!(!data.contains("D:\\"));
}

#[test]
fn error_messages_mask_known_and_unknown_windows_paths() {
    let view = view();
    let mut error = RuntimeError::new(
        "example",
        r"failed at r:\FIXTURE\PROJECT\Saved\log.txt via C:\Windows\Temp\tool.log",
    );
    view.project_error(&mut error);
    assert!(error.message.contains(r"@project\Saved\log.txt"));
    assert!(error.message.contains(r"@runtime/Windows\Temp\tool.log"));
    assert!(!error.message.contains("C:\\"));
    assert!(!error.message.contains("d:\\"));
}

#[test]
fn file_content_is_not_rewritten_by_generic_runtime_path_masking() {
    let view = view();
    let content = r"literal C:\Windows\System32 and R:\fixture\project\inside-file stay exact";
    let value = view.project_tool_output(
        "fs_read_text",
        json!({"path":r"R:\fixture\project\docs\note.txt","content":content}),
    );
    assert_eq!(value["path"], "@project/docs/note.txt");
    assert_eq!(value["content"], content);
}
