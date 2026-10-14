use serde_json::json;

use super::*;

#[test]
fn commands_carry_their_session() {
    let browser: Value = serde_json::from_str(&encode(
        1,
        "Target.createTarget",
        json!({ "url": "about:blank" }),
        None,
    ))
    .unwrap();
    assert_eq!(
        browser,
        json!({ "id": 1, "method": "Target.createTarget", "params": { "url": "about:blank" } })
    );
    let page: Value =
        serde_json::from_str(&encode(2, "Page.enable", json!({}), Some("S1"))).unwrap();
    assert_eq!(page["sessionId"], "S1");
}

#[test]
fn replies_and_events() {
    assert_eq!(
        parse(r#"{"id":3,"result":{"targetId":"T"}}"#),
        Some(Parsed::Reply {
            id: 3,
            result: Ok(json!({ "targetId": "T" }))
        })
    );
    assert_eq!(
        parse(r#"{"id":4,"error":{"code":-32601,"message":"'Page.nope' wasn't found"}}"#),
        Some(Parsed::Reply {
            id: 4,
            result: Err("'Page.nope' wasn't found".to_owned())
        })
    );
    assert_eq!(
        parse(r#"{"method":"Page.screencastFrame","params":{"sessionId":7},"sessionId":"S1"}"#),
        Some(Parsed::Event(CdpEvent {
            method: "Page.screencastFrame".to_owned(),
            params: json!({ "sessionId": 7 }),
            session: Some("S1".to_owned()),
        }))
    );
    assert_eq!(
        parse(r#"{"method":"Target.targetDestroyed","params":{"targetId":"T"}}"#).map(|parsed| {
            match parsed {
                Parsed::Event(event) => event.session,
                Parsed::Reply { .. } => Some("reply".to_owned()),
            }
        }),
        Some(None)
    );
    assert_eq!(parse("[]"), None);
    assert_eq!(parse("not json"), None);
}
