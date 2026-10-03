//! V2 tool observations are fail-closed and never confuse an accepted pass with the user's draft.
#[path = "common/control_plane_mint.rs"]
mod fixture;
use super::*;
use fixture::*;

fn request() -> BtcMintPrepareRequest {
    BtcMintPrepareRequest {
        source_address: SOURCE.into(),
        recipient_address: RECIPIENT.into(),
        usdb_main: Some(format!("0x{}", "1".repeat(40))),
        leader_pass_id: None,
        leader_btc_addr: None,
        prev: vec![],
    }
}
async fn verified(f: &Fixture, r: BtcMintPrepareRequest) -> BtcMintVerifyResponse {
    verify(
        State(f.state.clone()),
        Json(BtcMintVerifyRequest {
            mint: r,
            inscription_id: id('a'),
            expected_source_outpoint: None,
        }),
    )
    .await
    .unwrap()
    .0
}

#[tokio::test]
async fn public_preparation_needs_no_ord_but_requires_zero_and_never_owner() {
    let f = Fixture::new().await;
    let result = prepare(&f.state, &request()).await.unwrap().response;
    assert!(result.eligible, "{:?}", result.blockers);
    assert!(!result.execution_available);
    assert_eq!(result.operation_path.as_deref(), Some("first_opening"));
    f.data.lock().unwrap()["balance"] = json!(1);
    assert!(
        !prepare(&f.state, &request())
            .await
            .unwrap()
            .response
            .eligible
    );
    f.data.lock().unwrap()["balance"] = json!(0);
    f.data.lock().unwrap()["history"] = json!(true);
    assert!(
        !prepare(&f.state, &request())
            .await
            .unwrap()
            .response
            .eligible
    );
    f.data.lock().unwrap()["fail"] = json!("get_addresses_balances");
    let result = prepare(&f.state, &request()).await.unwrap().response;
    assert!(!result.eligible);
    assert!(result.observation.is_null());
}

#[tokio::test]
async fn source_prev_is_explicit_and_remaining_dormant_passes_stay_visible() {
    let f = Fixture::new().await;
    f.data.lock().unwrap()["passes"] = json!([
        {"inscription_id":id('c'),"owner":hash(SOURCE),"state":"dormant"},
        {"inscription_id":id('d'),"owner":hash(SOURCE),"state":"dormant"}]);
    let mut r = request();
    r.prev = vec![id('c')];
    let result = prepare(&f.state, &r).await.unwrap().response;
    assert!(result.eligible, "{:?}", result.blockers);
    assert_eq!(result.operation_path.as_deref(), Some("cross_owner"));
    assert_eq!(result.retained_pass_ids, vec![id('d')]);
    r.prev.push(id('e'));
    assert!(!prepare(&f.state, &r).await.unwrap().response.eligible);
    r.prev.clear();
    r.recipient_address = SOURCE.into();
    f.data.lock().unwrap()["history"] = json!(true);
    assert_eq!(
        prepare(&f.state, &r)
            .await
            .unwrap()
            .response
            .operation_path
            .as_deref(),
        Some("same_owner")
    );
}

#[tokio::test]
async fn preparation_reads_every_page_and_rejects_incomplete_or_stale_evidence() {
    let f = Fixture::new().await;
    f.data.lock().unwrap()["passes"]=json!((0..101).map(|i|json!({"inscription_id":format!("{}i{i}","c".repeat(64)),"owner":hash(SOURCE),"state":"dormant"})).collect::<Vec<_>>());
    assert_eq!(
        prepare(&f.state, &request())
            .await
            .unwrap()
            .response
            .source_passes
            .len(),
        101
    );
    f.data.lock().unwrap()["overrides"]["get_owner_passes_at_height"] =
        json!({"owner":hash(RECIPIENT),"resolved_height":100,"total":1,"items":[]});
    assert!(
        !prepare(&f.state, &request())
            .await
            .unwrap()
            .response
            .eligible
    );
    f.data.lock().unwrap()["overrides"] = json!({"getblockhash":"fork"});
    assert!(
        !prepare(&f.state, &request())
            .await
            .unwrap()
            .response
            .eligible
    );
}

#[tokio::test]
async fn verification_checks_full_configuration_source_and_confirmations() {
    let f = Fixture::new().await;
    assert!(verified(&f, request()).await.verified);
    for (key, bad) in [
        ("owner", json!(hash(SOURCE))),
        ("mint_owner", json!(hash(SOURCE))),
        ("state", json!("dormant")),
        ("mint_version", json!(1)),
        ("usdb_main", json!("0xwrong")),
        ("leader_pass_id", json!(id('e'))),
        ("prev", json!([id('c')])),
        ("inscription_id", json!(id('e'))),
    ] {
        let old = f.data.lock().unwrap()["snapshot"][key].clone();
        f.data.lock().unwrap()["snapshot"][key] = bad;
        assert!(!verified(&f, request()).await.verified, "{key}");
        f.data.lock().unwrap()["snapshot"][key] = old;
    }
    f.data.lock().unwrap()["source"]["source_owner"] = json!(hash(RECIPIENT));
    assert!(!verified(&f, request()).await.verified);
    f.data.lock().unwrap()["source"]["source_owner"] = json!(hash(SOURCE));
    f.data.lock().unwrap()["confirmations"] = json!(6);
    assert!(!verified(&f, request()).await.verified);
    f.data.lock().unwrap()["confirmations"] = json!(-1);
    assert!(!verified(&f, request()).await.verified);
    f.data.lock().unwrap()["fail"] = json!("get_pass_mint_source");
    assert!(!verified(&f, request()).await.verified);
}

#[tokio::test]
async fn verification_binds_actual_source_coin_and_prev_consumption() {
    let f = Fixture::new().await;
    let r = BtcMintVerifyRequest {
        mint: request(),
        inscription_id: id('a'),
        expected_source_outpoint: Some("wrong:0".into()),
    };
    assert!(
        !verify(State(f.state.clone()), Json(r))
            .await
            .unwrap()
            .0
            .verified
    );
    let mut r = request();
    r.prev = vec![id('c')];
    f.data.lock().unwrap()["snapshot"]["prev"] = json!(r.prev);
    assert!(verified(&f, r.clone()).await.verified);
    f.data.lock().unwrap()["prev_state"] = json!("dormant");
    assert!(!verified(&f, r).await.verified);
}

#[tokio::test]
async fn collab_bindings_are_checked_without_inventing_a_beneficiary() {
    let f = Fixture::new().await;
    for binding in ["leader_pass_id", "leader_btc_addr"] {
        let mut r = request();
        r.usdb_main = None;
        if binding == "leader_pass_id" {
            r.leader_pass_id = Some(id('d'));
        } else {
            r.leader_btc_addr = Some(SOURCE.into());
        }
        assert!(prepare(&f.state, &r).await.unwrap().response.eligible);
        f.data.lock().unwrap()["snapshot"]["pass_kind"] = json!("collab");
        f.data.lock().unwrap()["snapshot"]["usdb_main"] = json!("");
        f.data.lock().unwrap()["snapshot"][binding] = json!(if binding == "leader_pass_id" {
            id('d')
        } else {
            SOURCE.into()
        });
        assert!(verified(&f, r.clone()).await.verified);
        f.data.lock().unwrap()["snapshot"][binding] = json!("wrong");
        assert!(!verified(&f, r).await.verified);
        f.data.lock().unwrap()["snapshot"][binding] = Value::Null;
        f.data.lock().unwrap()["snapshot"]["pass_kind"] = json!("standard");
    }
}

#[tokio::test]
async fn development_execute_is_not_authorized_by_prepare_eligibility() {
    let f = Fixture::new().await;
    let r = crate::models::BtcMintExecuteRequest {
        wallet_name: "world-0".into(),
        source_address: SOURCE.into(),
        recipient_address: RECIPIENT.into(),
        usdb_main: request().usdb_main,
        leader_pass_id: None,
        leader_btc_addr: None,
        prev: vec![],
    };
    let error = super::super::post_execute_btc_mint(State(f.state.clone()), Json(r))
        .await
        .unwrap_err();
    assert!(error.1.0.error.contains("enabled development"));
    assert!(
        !f.data.lock().unwrap()["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["method"] == "listunspent")
    );
}

#[tokio::test]
async fn address_bound_collab_can_open_before_its_leader_exists() {
    let f = Fixture::new().await;
    f.data.lock().unwrap()["snapshot"] = Value::Null;
    let mut r = request();
    r.usdb_main = None;
    r.leader_btc_addr = Some(SOURCE.into());
    let result = prepare(&f.state, &r).await.unwrap().response;
    assert!(result.eligible);
    assert!(
        result
            .warnings
            .iter()
            .any(|s| s.contains("no Active standard pass"))
    );
    r.leader_btc_addr = None;
    r.leader_pass_id = Some(id('d'));
    assert!(!prepare(&f.state, &r).await.unwrap().response.eligible);
}
