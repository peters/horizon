//! Agents' access requests as the worker lists them, and the person's decisions.
use super::*;

#[test]
fn requests_keep_only_well_formed_entries() {
    let listed = requests::parse_list(
        "{\"requests\":[\
         {\"id\":\"r1\",\"repository\":\"acme/design-system\",\"access\":\"push\",\"reason\":\"Shared fix\",\"session\":\"panel-2\",\"agent\":\"claude\",\"created_at\":1},\
         {\"id\":\"r 2\",\"repository\":\"acme/x\",\"access\":\"push\",\"reason\":\"x\"},\
         {\"id\":\"r3\",\"repository\":\"acme/x\",\"access\":\"admin\",\"reason\":\"x\"},\
         {\"id\":\"r4\",\"repository\":\"acme/x\",\"access\":\"read\",\"reason\":\"line\\nbreak\"}]}",
    );
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].repository, "acme/design-system");
    assert_eq!(listed[0].agent, "claude");
    assert!(requests::parse_list("not json").is_empty());
    for misleading in ["\\u202e", "\\u2066", "\\u200f"] {
        let reordered = requests::parse_list(&format!(
            "{{\"requests\":[{{\"id\":\"r5\",\"repository\":\"acme/x\",\"access\":\"read\",\
             \"reason\":\"fix {misleading}tsurt\",\"agent\":\"claude\"}},\
             {{\"id\":\"r6\",\"repository\":\"acme/x\",\"access\":\"read\",\"reason\":\"x\",\
             \"agent\":\"cl{misleading}aude\"}}]}}"
        ));
        assert!(
            reordered.is_empty(),
            "bidirectional formatting is refused: {misleading}"
        );
    }
}

#[test]
fn a_refused_decision_is_explained() {
    assert_eq!(
        requests::parse_decision(
            "{\"ok\":true,\"id\":\"r1\",\"decision\":\"allow-cloud\",\"repository\":\"acme/x\",\"access\":\"push\",\"status\":\"allowed\"}"
        ),
        None
    );
    assert!(
        requests::parse_decision(
            "{\"ok\":false,\"id\":\"r1\",\"error\":\"not_installed\",\"message\":\"The GitHub App is not installed\"}"
        )
        .unwrap()
        .contains("not installed")
    );
    assert!(
        requests::parse_decision("{\"ok\":false,\"error\":\"token_expired\"}")
            .unwrap()
            .contains("Connect GitHub again")
    );
    assert_eq!(
        requests::parse_decision("{\"ok\":false,\"error\":\"Weird Text\"}").unwrap(),
        "The worker refused the decision."
    );
    assert!(requests::parse_decision("").is_some());
}
