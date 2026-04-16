use claude_agent_sdk::{
    PermissionMode, PermissionUpdate, PermissionUpdateDestination, PermissionUpdateKind,
};

#[test]
fn permission_update_set_mode_serializes_correctly() {
    let upd = PermissionUpdate {
        kind: PermissionUpdateKind::SetMode { mode: PermissionMode::AcceptEdits },
        destination: Some(PermissionUpdateDestination::Session),
    };
    let v = upd.to_value();
    assert_eq!(v["type"], "setMode");
    assert_eq!(v["mode"], "acceptEdits");
    assert_eq!(v["destination"], "session");
}

#[test]
fn permission_update_add_directories_serializes() {
    let upd = PermissionUpdate {
        kind: PermissionUpdateKind::AddDirectories { directories: vec!["/tmp".into()] },
        destination: None,
    };
    let v = upd.to_value();
    assert_eq!(v["type"], "addDirectories");
    assert_eq!(v["directories"][0], "/tmp");
}
