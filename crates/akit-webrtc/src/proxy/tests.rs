//! fit's `requestUfrag`, without a network.
#![allow(clippy::unwrap_used)]

use super::*;
use akio_turn::stun::{Class, MessageBuilder, Method, attr, transaction_id};

#[test]
fn ufrag_of_requests() {
    let req = |user: &[u8]| {
        MessageBuilder::new(Method::BINDING, Class::Request, transaction_id())
            .attr(attr::USERNAME, user)
            .build()
    };
    assert_eq!(request_ufrag(&req(b"srv:cli")), Some("srv"));
    assert_eq!(request_ufrag(&req(b"nocolon")), None);
    assert_eq!(request_ufrag(b"hello"), None);
    // A Binding response is not a request.
    let resp = MessageBuilder::new(Method::BINDING, Class::Success, transaction_id())
        .attr(attr::USERNAME, b"a:b")
        .build();
    assert_eq!(request_ufrag(&resp), None);
    // A truncated one.
    let r = req(b"srv:cli");
    assert_eq!(request_ufrag(r.get(..r.len() - 2).unwrap()), None);
}
