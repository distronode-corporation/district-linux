//! The endpoint table's own invariants: every row unique and well formed, and the
//! rules about bodies, workspaces and retries that hold across the whole table.

use std::collections::{BTreeMap, BTreeSet};

use district_api::{
    ALL_ENDPOINTS, Auth, BodyKind, EXCLUDED, Endpoint, HttpMethod, LINUX_ONLY, PathMatch,
    RetryPolicy, WorkspaceIn, normalize_template,
};

#[test]
fn every_method_and_path_is_unique() {
    let mut seen: BTreeMap<(HttpMethod, String), Vec<&str>> = BTreeMap::new();
    for spec in ALL_ENDPOINTS {
        seen.entry((spec.method, normalize_template(spec.path_template)))
            .or_default()
            .push(spec.id.name());
    }
    let duplicates: Vec<_> = seen.iter().filter(|(_, ids)| ids.len() > 1).collect();
    assert!(
        duplicates.is_empty(),
        "one method and path, several endpoints: {duplicates:?}"
    );
}

#[test]
fn ids_are_unique_and_each_indexes_its_own_row() {
    let mut names = BTreeSet::new();
    for (index, spec) in ALL_ENDPOINTS.iter().enumerate() {
        assert_eq!(
            spec.id as usize,
            index,
            "{} is out of order",
            spec.id.name()
        );
        assert_eq!(
            spec.id.spec(),
            spec,
            "{} does not look itself up",
            spec.id.name()
        );
        assert!(
            names.insert(spec.id.name()),
            "{} appears twice",
            spec.id.name()
        );
    }
    assert_eq!(names.len(), ALL_ENDPOINTS.len());
}

#[test]
fn every_endpoint_sends_the_access_token() {
    for spec in ALL_ENDPOINTS {
        assert_eq!(spec.auth, Auth::Bearer, "{}", spec.id.name());
    }
}

#[test]
fn paths_are_well_formed() {
    for spec in ALL_ENDPOINTS {
        let name = spec.id.name();
        let path = spec.path_template;
        assert!(path.starts_with("/api/"), "{name}: {path}");
        assert!(!path.ends_with('/'), "{name}: {path}");
        for segment in path[1..].split('/') {
            assert!(!segment.is_empty(), "{name}: empty segment in {path}");
            if let Some(inner) = segment.strip_prefix('{') {
                let param = inner
                    .strip_suffix('}')
                    .unwrap_or_else(|| panic!("{name}: {path}"));
                assert!(
                    !param.is_empty() && param.chars().all(|c| c.is_ascii_alphanumeric()),
                    "{name}: placeholder {segment} in {path}"
                );
            } else {
                assert!(
                    segment.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                    "{name}: segment {segment} in {path}"
                );
            }
        }
    }
}

/// Only reads are repeated after a refused token, plus the two endpoints that
/// mint a short-lived token and store nothing. Everything with a side effect or a
/// cost is sent at most once.
#[test]
fn reads_may_repeat_and_writes_may_not() {
    let token_mints = [Endpoint::CallRoomToken, Endpoint::TelemetryToken];
    for spec in ALL_ENDPOINTS {
        let expected = if spec.method == HttpMethod::Get || token_mints.contains(&spec.id) {
            RetryPolicy::OnceAfterRefresh
        } else {
            RetryPolicy::Never
        };
        assert_eq!(spec.retry, expected, "{}", spec.id.name());
    }
    for mint in token_mints {
        assert_eq!(mint.spec().method, HttpMethod::Post);
    }
}

/// Named one by one as well, so that a change to the rule above cannot quietly
/// make any of these repeatable.
#[test]
fn calls_with_side_effects_or_cost_never_repeat() {
    let never = [
        Endpoint::CallDial,
        Endpoint::CallAnswer,
        Endpoint::CallHangUp,
        Endpoint::MessageSend,
        Endpoint::MessageMediaUpload,
        Endpoint::MessageDraftGenerate,
        Endpoint::Hq,
        Endpoint::PersonaPreviewToken,
        Endpoint::SchedulingEnable,
        Endpoint::SchedulingHandOff,
        Endpoint::ContactCreate,
        Endpoint::ContactDelete,
        Endpoint::ContactEnrich,
        Endpoint::KnowledgeDocumentCreate,
        Endpoint::KnowledgeDocumentDelete,
        Endpoint::MemberAdd,
        Endpoint::MemberRemove,
        Endpoint::MessageDraftDelete,
        Endpoint::DeskTicketCreate,
        Endpoint::DeskLogoUpload,
        Endpoint::DeskLogoDelete,
        Endpoint::SupportRequestCreate,
        Endpoint::NativeDeviceRevoke,
        Endpoint::NativeRevokeAll,
        Endpoint::PushRegister,
    ];
    for endpoint in never {
        assert_eq!(
            endpoint.spec().retry,
            RetryPolicy::Never,
            "{}",
            endpoint.name()
        );
    }
}

#[test]
fn bodies_fit_their_methods() {
    for spec in ALL_ENDPOINTS {
        let name = spec.id.name();
        match spec.method {
            HttpMethod::Get | HttpMethod::Delete => {
                assert_eq!(spec.body, BodyKind::Empty, "{name}");
            }
            HttpMethod::Put | HttpMethod::Patch => assert_eq!(spec.body, BodyKind::Json, "{name}"),
            HttpMethod::Post => {}
        }
        if spec.body == BodyKind::Multipart {
            assert_eq!(spec.method, HttpMethod::Post, "{name}");
        }
        match spec.workspace_scoped {
            Some(WorkspaceIn::Body) => assert_eq!(spec.body, BodyKind::Json, "{name}"),
            Some(WorkspaceIn::Form) => assert_eq!(spec.body, BodyKind::Multipart, "{name}"),
            Some(WorkspaceIn::Query) | None => {}
        }
    }
}

#[test]
fn reads_and_deletes_name_their_workspace_in_the_query() {
    for spec in ALL_ENDPOINTS {
        if matches!(spec.method, HttpMethod::Get | HttpMethod::Delete)
            && spec.workspace_scoped.is_some()
        {
            assert_eq!(
                spec.workspace_scoped,
                Some(WorkspaceIn::Query),
                "{}",
                spec.id.name()
            );
        }
    }
}

/// The endpoints that name no workspace, listed so that a new one is a decision.
#[test]
fn only_account_and_device_endpoints_are_unscoped() {
    let unscoped: BTreeSet<_> = ALL_ENDPOINTS
        .iter()
        .filter(|spec| spec.workspace_scoped.is_none())
        .map(|spec| spec.id)
        .collect();
    let expected = BTreeSet::from([
        Endpoint::AuthMe,
        Endpoint::NativeDevices,
        Endpoint::NativeDeviceRevoke,
        Endpoint::NativeRevokeAll,
        Endpoint::StripeBilling,
        Endpoint::WorkspaceList,
        Endpoint::CallRoomToken,
        Endpoint::PushRegister,
        Endpoint::PushUnregister,
    ]);
    assert_eq!(unscoped, expected);
}

#[test]
fn no_endpoint_in_the_table_is_excluded() {
    for spec in ALL_ENDPOINTS {
        for exclusion in EXCLUDED {
            assert!(
                !exclusion.covers(spec.method, spec.path_template),
                "{} is covered by the exclusion {:?}",
                spec.id.name(),
                exclusion.name
            );
        }
    }
}

#[test]
fn every_exclusion_says_why() {
    let mut names = BTreeSet::new();
    for exclusion in EXCLUDED {
        assert!(
            names.insert(exclusion.name),
            "{} is listed twice",
            exclusion.name
        );
        assert!(!exclusion.reason.is_empty(), "{}", exclusion.name);
        assert!(
            !exclusion.reason.contains('\n'),
            "{}: one line",
            exclusion.name
        );
        assert!(
            exclusion.path.as_str().starts_with("/api/"),
            "{}",
            exclusion.name
        );
    }
}

#[test]
fn linux_only_additions_are_in_the_table_and_say_why() {
    for addition in LINUX_ONLY {
        assert!(
            ALL_ENDPOINTS
                .iter()
                .any(|spec| spec.id == addition.endpoint)
        );
        assert!(!addition.reason.is_empty());
    }
}

#[test]
fn a_subtree_covers_its_root_and_everything_below_it_only() {
    let reports = PathMatch::Subtree("/api/reports");
    assert!(reports.matches("/api/reports"));
    assert!(reports.matches("/api/reports/daily"));
    assert!(reports.matches("/api/reports/{id}/pdf"));
    assert!(!reports.matches("/api/reportsx"));
    assert!(!reports.matches("/api/report"));
    assert_eq!(reports.as_str(), "/api/reports");
}

#[test]
fn an_exact_match_ignores_placeholder_names() {
    let exact = PathMatch::Exact("/api/things/{thingId}/part");
    assert!(exact.matches("/api/things/{id}/part"));
    assert!(!exact.matches("/api/things/{id}/part/more"));
    assert!(!exact.matches("/api/things/literal/part"));
    assert_eq!(exact.as_str(), "/api/things/{thingId}/part");
}

#[test]
fn an_exclusion_with_a_method_covers_only_that_method() {
    let token = EXCLUDED
        .iter()
        .find(|e| e.method == Some(HttpMethod::Post))
        .expect("at least one exclusion names a method");
    let path = token.path.as_str();
    assert!(token.covers(HttpMethod::Post, path));
    assert!(!token.covers(HttpMethod::Get, path));

    let any_method = EXCLUDED.iter().find(|e| e.method.is_none()).unwrap();
    for method in [HttpMethod::Get, HttpMethod::Post, HttpMethod::Delete] {
        assert!(any_method.covers(method, any_method.path.as_str()));
    }
}

#[test]
fn templates_normalize_their_placeholders() {
    assert_eq!(normalize_template("/api/a/{x}/b/{yId}"), "/api/a/{}/b/{}");
    assert_eq!(normalize_template("/api/a/b"), "/api/a/b");
    assert_eq!(normalize_template("/api/{half"), "/api/{half");
}

#[test]
fn methods_are_spelled_as_on_the_wire() {
    let spelled: Vec<_> = [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
    ]
    .iter()
    .map(|m| m.as_str())
    .collect();
    assert_eq!(spelled, ["GET", "POST", "PUT", "PATCH", "DELETE"]);
}
