use super::*;

#[test]
fn project_preparation_rejects_workspace_spoofing_and_unknown_fields() {
    for input in [
        r#"{"type":"WORKSPACE_PROJECT","projectId":7,"workspaceId":2}"#,
        r#"{"type":"LOCAL_DIRECTORY","directory":"/work","command":"anything"}"#,
        r#"{"type":"SHELL","directory":"/work"}"#,
    ] {
        assert!(serde_json::from_str::<PrepareProject>(input).is_err());
    }
    assert!(prepare(
        0,
        PrepareProject::LocalDirectory {
            directory: "/".into()
        }
    )
    .is_err());
}

#[test]
fn local_project_preparation_creates_only_the_requested_new_directory() {
    let root = std::env::temp_dir().join(format!(
        "codex-project-test-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::create_dir(&root).unwrap();
    let parent = root.to_str().unwrap();
    for name in [
        "",
        "..",
        ".",
        "../escape",
        "/absolute",
        "nested/child",
        "C:drive",
        "back\\slash",
    ] {
        assert!(create_directory(parent, name).is_err());
    }
    let result = create_directory(parent, "新项目").unwrap();
    assert!(Path::new(&result.directory).is_dir());
    assert_eq!(
        local_directory(&result.directory).unwrap().directory,
        result.directory
    );
    let file = Path::new(&result.directory).join("keep.txt");
    fs::write(&file, "keep").unwrap();
    assert!(create_directory(parent, "新项目").is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
    assert!(local_directory("relative").is_err());
    fs::remove_dir_all(root).unwrap();
}
