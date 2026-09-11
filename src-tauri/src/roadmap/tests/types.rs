    use super::*;

    /// in Rust and never touch serde, but the frontend's patches arrive as
    /// bytes that must stay different values.
    #[test]
    fn patch_null_clears_value_sets_absent_keeps() {
        let p: ItemPatch =
            serde_json::from_str(r#"{"area": null, "workflow_def_id": "wf-1"}"#).unwrap();
        assert_eq!(p.area, Some(None), "an explicit null means 'clear'");
        assert_eq!(
            p.workflow_def_id,
            Some(Some("wf-1".into())),
            "a value means 'set'"
        );
        assert_eq!(p.run_id, None, "an absent key means 'leave alone'");
        assert_eq!(p.title, None);
    }

    #[test]
    fn only_pre_work_statuses_are_rulable() {
        for status in [ItemStatus::Proposed, ItemStatus::Open, ItemStatus::Queued] {
            assert!(status.is_rulable(), "{status:?}");
        }
        for status in [
            ItemStatus::Active,
            ItemStatus::InReview,
            ItemStatus::Done,
            ItemStatus::Rejected,
        ] {
            assert!(!status.is_rulable(), "{status:?}");
        }
    }

    #[test]
    fn a_wire_patch_can_never_reassign_the_agent() {
        for json in [r#"{"agent_id": "a-1"}"#, r#"{"agent_id": null}"#] {
            let p: ItemPatch = serde_json::from_str(json).unwrap();
            assert_eq!(p.agent_id, None, "{json}");
        }
    }
