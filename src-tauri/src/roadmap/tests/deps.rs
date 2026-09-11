    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> Graph {
        edges
            .iter()
            .map(|(code, deps)| {
                (
                    (*code).to_string(),
                    deps.iter().map(|d| (*d).to_string()).collect(),
                )
            })
            .collect()
    }

    fn list(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| (*c).to_string()).collect()
    }

    fn rejects(codes: &[&str]) -> BTreeSet<String> {
        codes.iter().map(|c| (*c).to_string()).collect()
    }

    fn no_rejects() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn an_item_with_no_deps_is_always_fine() {
        let g = graph(&[("MCA-100", &[]), ("MCA-101", &[])]);
        assert_eq!(validate_edit(&g, &no_rejects(), "MCA-100", &[]), Ok(()));
        assert_eq!(validate_new(&g, &no_rejects(), &[]), Ok(()));
        assert_eq!(find_cycle(&g, "MCA-100"), None);
    }

    #[test]
    fn a_chain_however_long_is_fine() {
        let g = graph(&[
            ("MCA-100", &[]),
            ("MCA-101", &["MCA-100"]),
            ("MCA-102", &["MCA-101"]),
            ("MCA-103", &[]),
        ]);
        assert_eq!(
            validate_edit(&g, &no_rejects(), "MCA-103", &list(&["MCA-102"])),
            Ok(())
        );
        assert_eq!(
            validate_edit(&g, &no_rejects(), "MCA-103", &list(&["MCA-102", "MCA-100"])),
            Ok(())
        );
        assert_eq!(find_cycle(&g, "MCA-102"), None);
    }

    #[test]
    fn a_direct_cycle_is_refused_and_spelled_out() {
        let g = graph(&[("MCA-101", &[]), ("MCA-104", &["MCA-101"])]);
        let err = validate_edit(&g, &no_rejects(), "MCA-101", &list(&["MCA-104"])).unwrap_err();
        assert!(
            err.contains("MCA-101 → MCA-104 → MCA-101"),
            "the refusal must name the loop: {err}"
        );
        assert!(err.contains("loop"), "{err}");
    }

    #[test]
    fn a_transitive_cycle_is_refused_with_the_whole_path() {
        let g = graph(&[
            ("MCA-101", &["MCA-102"]),
            ("MCA-102", &["MCA-103"]),
            ("MCA-103", &[]),
        ]);
        let err = validate_edit(&g, &no_rejects(), "MCA-103", &list(&["MCA-101"])).unwrap_err();
        assert!(
            err.contains("MCA-103 → MCA-101 → MCA-102 → MCA-103"),
            "{err}"
        );
    }

    #[test]
    fn depending_on_a_loop_you_are_not_in_is_refused_too() {
        let g = graph(&[
            ("MCA-101", &["MCA-102"]),
            ("MCA-102", &["MCA-101"]),
            ("MCA-200", &[]),
        ]);
        let err = validate_edit(&g, &no_rejects(), "MCA-200", &list(&["MCA-101"])).unwrap_err();
        assert!(
            err.contains("MCA-200 would wait on a dependency loop"),
            "{err}"
        );
        assert!(err.contains("MCA-101 → MCA-102 → MCA-101"), "{err}");
    }

    #[test]
    fn an_item_cannot_depend_on_itself() {
        let g = graph(&[("MCA-100", &[])]);
        let err = validate_edit(&g, &no_rejects(), "MCA-100", &list(&["MCA-100"])).unwrap_err();
        assert!(err.contains("depend on itself"), "{err}");
    }

    #[test]
    fn an_unknown_code_is_refused_and_the_board_is_listed() {
        let g = graph(&[("MCA-100", &[]), ("MCA-101", &[])]);
        let err = validate_edit(&g, &no_rejects(), "MCA-100", &list(&["MCA-999"])).unwrap_err();
        assert!(err.contains("MCA-999"), "{err}");
        assert!(
            err.contains("MCA-100, MCA-101"),
            "the codes it could name: {err}"
        );
        assert!(validate_new(&g, &no_rejects(), &list(&["MCA-999"])).is_err());
        let err = validate_new(&Graph::new(), &no_rejects(), &list(&["MCA-999"])).unwrap_err();
        assert!(err.contains("no items to depend on yet"), "{err}");
    }

    #[test]
    fn a_long_board_lists_only_the_first_codes() {
        let codes: Vec<String> = (0..20).map(|n| format!("MCA-1{n:02}")).collect();
        let g: Graph = codes.iter().map(|c| (c.clone(), Vec::new())).collect();
        let err = validate_edit(&g, &no_rejects(), "MCA-100", &list(&["nope"])).unwrap_err();
        assert!(err.contains("and 8 more"), "{err}");
    }

    #[test]
    fn a_dep_on_a_rejected_item_is_refused_with_the_reopen_path_named() {
        let g = graph(&[("MCA-100", &[]), ("MCA-101", &[])]);
        let dead = rejects(&["MCA-100"]);

        for err in [
            validate_new(&g, &dead, &list(&["MCA-100"])).unwrap_err(),
            validate_edit(&g, &dead, "MCA-101", &list(&["MCA-100"])).unwrap_err(),
            validate_batch(&g, &dead, &[list(&["MCA-100"])])
                .unwrap_err()
                .message,
        ] {
            assert!(err.contains("MCA-100 was rejected"), "{err}");
            assert!(err.contains("reopen"), "{err}");
        }

        assert_eq!(validate_new(&g, &dead, &list(&["MCA-101"])), Ok(()));
        let batch = vec![Vec::new(), list(&["MCA-100"])];
        assert_eq!(validate_batch(&g, &dead, &batch).unwrap_err().at, 1);
    }

    #[test]
    fn a_batch_may_order_itself_with_hash_references() {
        let g = graph(&[("MCA-100", &[])]);
        let batch = vec![
            list(&["#3"]),
            list(&["#1"]),
            list(&["MCA-100", "#4"]),
            Vec::new(),
        ];
        assert_eq!(validate_batch(&g, &no_rejects(), &batch), Ok(()));
        assert_eq!(batch_index("#3", 4), Some(2));
        assert_eq!(batch_index("MCA-100", 4), None);
        assert_eq!(batch_index("#9", 4), None);
    }

    #[test]
    fn a_batch_internal_cycle_is_refused() {
        let batch = vec![list(&["#2"]), list(&["#3"]), list(&["#1"])];
        let refusal = validate_batch(&Graph::new(), &no_rejects(), &batch).unwrap_err();
        assert_eq!(refusal.at, 0);
        assert!(refusal.message.contains("#1 → #2 → #3 → #1"), "{refusal:?}");
    }

    #[test]
    fn a_batch_item_that_waits_on_a_board_loop_is_refused() {
        // that can never be built.
        let g = graph(&[("MCA-101", &["MCA-102"]), ("MCA-102", &["MCA-101"])]);
        let batch = vec![Vec::new(), list(&["MCA-101"])];
        let refusal = validate_batch(&g, &no_rejects(), &batch).unwrap_err();
        assert_eq!(refusal.at, 1);
        assert!(
            refusal.message.contains("MCA-101 → MCA-102 → MCA-101"),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_batch_reference_must_be_in_range_and_not_itself() {
        let batch = vec![Vec::new(), Vec::new()];
        for (deps, at, needle) in [
            (list(&["#3"]), 0, "not an item in this batch"),
            (list(&["#0"]), 0, "not an item in this batch"),
            (list(&["#two"]), 0, "not an item in this batch"),
            (list(&["#1"]), 0, "depend on itself"),
        ] {
            let mut batch = batch.clone();
            batch[at] = deps;
            let refusal = validate_batch(&Graph::new(), &no_rejects(), &batch).unwrap_err();
            assert_eq!(refusal.at, at);
            assert!(refusal.message.contains(needle), "{refusal:?}");
        }
    }

    #[test]
    fn a_batch_dep_on_an_unknown_code_names_the_batch_syntax_too() {
        let g = graph(&[("MCA-100", &[])]);
        let batch = vec![list(&["MCA-999"])];
        let refusal = validate_batch(&g, &no_rejects(), &batch).unwrap_err();
        assert_eq!(refusal.at, 0);
        assert!(refusal.message.contains("MCA-999"), "{refusal:?}");
        assert!(refusal.message.contains("#n"), "{refusal:?}");
    }

    #[test]
    fn a_deleted_dependency_is_a_leaf_not_a_loop() {
        let g = graph(&[("MCA-100", &["MCA-099"])]);
        assert_eq!(find_cycle(&g, "MCA-100"), None);
    }

    #[test]
    fn the_graph_comes_straight_off_the_board() {
        let g = graph_of(&[]);
        assert!(g.is_empty());
    }
