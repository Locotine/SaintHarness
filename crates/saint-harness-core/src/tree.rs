use crate::store::EventStore;
use saint_harness_schema::{InteractionMode, RuntimeMode, SessionEvent};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNodeInfo {
    pub id: String,
    pub parent_msg_id: Option<String>,
    pub seq: i64,
    pub role: String,
    pub preview: String,
    pub is_head: bool,
}

pub fn list_session_tree(
    store: &EventStore,
    session_id: &str,
) -> Result<Vec<TreeNodeInfo>, String> {
    let sess = store
        .reader
        .get_session(session_id)?
        .ok_or_else(|| format!("session `{session_id}` not found"))?;
    let all = store.reader.list_all_messages(session_id)?;
    Ok(all
        .into_iter()
        .map(|row| {
            let preview: String = row.message.plain_text().chars().take(72).collect();
            let is_head = sess.active_branch.as_deref() == Some(row.id.as_str());
            TreeNodeInfo {
                id: row.id,
                parent_msg_id: row.parent_msg_id,
                seq: row.seq,
                role: format!("{:?}", row.message.role).to_ascii_lowercase(),
                preview,
                is_head,
            }
        })
        .collect())
}

pub async fn switch_active_branch(
    store: &EventStore,
    session_id: &str,
    target_msg_id: String,
) -> Result<(), String> {
    store
        .bus
        .publish_durable(
            SessionEvent::SessionActiveBranchChanged {
                session_id: session_id.to_string(),
                active_branch: Some(target_msg_id),
            },
            None,
        )
        .await?;
    Ok(())
}

/// Forks an existing session up through `through_msg_id` (or current `active_branch`) into a new session
/// copying the exact ancestor chain along `parent_msg_id`.
pub async fn fork_session(
    store: &EventStore,
    source_session_id: &str,
    through_msg_id: Option<&str>,
) -> Result<String, String> {
    let source = store
        .reader
        .get_session(source_session_id)?
        .ok_or_else(|| format!("session `{source_session_id}` not found"))?;

    let all = store.reader.list_all_messages(source_session_id)?;
    let by_id: std::collections::HashMap<String, _> =
        all.into_iter().map(|m| (m.id.clone(), m)).collect();
    let head = through_msg_id
        .map(ToString::to_string)
        .or(source.active_branch.clone());

    let mut chain = Vec::new();
    let mut cur = head;
    while let Some(id) = cur {
        if let Some(row) = by_id.get(&id) {
            chain.push(row.clone());
            cur = row.parent_msg_id.clone();
        } else {
            break;
        }
    }
    chain.reverse();

    let new_session_id = format!("sess_{}", Ulid::new());
    store
        .bus
        .publish_durable(
            SessionEvent::SessionCreated {
                session_id: new_session_id.clone(),
                parent_id: None,
                fork_source_id: Some(source.id.clone()),
                title: format!("{} (fork)", source.title),
                agent_id: source.agent_id,
                model_profile: source.model_profile,
                cwd: source.cwd,
                workspace_root: source.workspace_root,
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: InteractionMode::Build,
            },
            None,
        )
        .await?;

    let mut prev_new_id: Option<String> = None;
    for row in chain {
        let mut msg = row.message;
        let new_msg_id = format!("msg_{}", Ulid::new());
        msg.id = new_msg_id.clone();
        msg.parent_msg_id = prev_new_id.clone();
        prev_new_id = Some(new_msg_id);
        store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: new_session_id.clone(),
                    message: msg,
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: None,
                    changed_files: vec![],
                },
                None,
            )
            .await?;
    }

    Ok(new_session_id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{ContentPart, Message, Role};

    #[tokio::test]
    async fn tree_branch_walk_and_fork_keep_sibling_branches_isolated() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EventStore::open(tmp.path().join("tree.sqlite")).unwrap();
        let sid = "s_tree".to_string();
        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: sid.clone(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "tree".into(),
                    agent_id: "build".into(),
                    model_profile: "qwen-3.8-27b".into(),
                    cwd: "/tmp".into(),
                    workspace_root: "/tmp".into(),
                    runtime_mode: RuntimeMode::Auto,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await
            .unwrap();

        for (id, parent, txt) in [
            ("m1", None, "root"),
            ("m2a", Some("m1"), "branch A"),
            ("m2b", Some("m1"), "branch B"),
        ] {
            store
                .bus
                .publish_durable(
                    SessionEvent::MessageAppended {
                        session_id: sid.clone(),
                        message: Message {
                            id: id.into(),
                            parent_msg_id: parent.map(ToString::to_string),
                            role: Role::User,
                            parts: vec![ContentPart::Text {
                                text: txt.into(),
                                provider_meta: None,
                            }],
                            model_provider: None,
                            model_id: None,
                        },
                        finish_reason: None,
                        usage: None,
                        error_json: None,
                        snapshot_start: None,
                        changed_files: vec![],
                    },
                    None,
                )
                .await
                .unwrap();
        }

        // Active head is m2b -> walking branch only includes [m1, m2b], never sibling m2a
        let branch_b = store.reader.walk_active_branch(&sid).unwrap();
        assert_eq!(
            branch_b.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["m1", "m2b"]
        );

        // Switch to m2a -> only [m1, m2a]
        switch_active_branch(&store, &sid, "m2a".into())
            .await
            .unwrap();
        let branch_a = store.reader.walk_active_branch(&sid).unwrap();
        assert_eq!(
            branch_a.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["m1", "m2a"]
        );

        // Fork through m2a
        let forked = fork_session(&store, &sid, Some("m2a")).await.unwrap();
        assert_eq!(store.reader.walk_active_branch(&forked).unwrap().len(), 2);
    }
}
