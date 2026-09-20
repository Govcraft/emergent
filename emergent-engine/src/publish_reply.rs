//! Whether the broker should answer a publish it has just handled.
//!
//! A client that publishes with `publish_ack` waits for the broker's
//! `PublishAck`, so the broker has to send one. A client that publishes
//! fire and forget waits for nothing, and acton-reactive still leaves the
//! broker holding an envelope that looks repliable.
//!
//! The difference is in the reply address, and acton sets it in two places.
//! `ActorHandle::try_send_boxed` (`common/actor_handle.rs:956-966`), the fire
//! and forget path, builds the envelope with `create_envelope(Some(self
//! .reply_address()))`, so the reply address is the broker's own.
//! `try_send_boxed_with_reply_to` (`common/actor_handle.rs:984-1005`), the
//! `publish_ack` path, is handed the listener's per-request proxy address
//! instead, whose Ern root is `ipc_proxy_<correlation_id>`
//! (`common/ipc/listener.rs:1795-1800`).
//!
//! The actor loop turns both into the handler's reply envelope the same way,
//! `OutboundEnvelope::new_with_recipient(envelope.recipient, envelope.reply_to)`
//! (`actor/managed_actor/idle.rs:172-176`), so in the handler the sender is
//! always the broker and the recipient is whoever asked. `OutboundEnvelope
//! ::reply` (`message/outbound_envelope.rs:185-209`) never checks: it spawns a
//! task and delivers. Replying to a fire and forget publish therefore costs a
//! spawned task and a slot in the broker's own bounded inbox, for a message no
//! handler accepts, competing with real publishes.
//!
//! An actor's Ern root carries a ULID (`message_broker_01m2y2dvb1ez...`), so
//! the broker's own name is read off the reply envelope's return address
//! rather than written out anywhere.
//!
//! Measured on 20 fire and forget publishes against engine 0.10.10: 20
//! `emergent::PublishAck` replies, every one logged with sender and recipient
//! equal to the broker's own Ern.
//!
//! [`should_reply`] is the whole decision, and it is pure.

/// Whether a reply is worth sending, given who it would go to (pure).
///
/// `self_name` is the broker's own actor name and `reply_recipient` the name on
/// the reply envelope's recipient address. They match when acton addressed the
/// reply back at the broker, which is what a fire and forget publish leaves
/// behind. An envelope with no recipient falls back to its return address,
/// which in a handler is the broker as well, so that is self addressed too.
#[must_use]
pub fn should_reply(self_name: &str, reply_recipient: Option<&str>) -> bool {
    reply_recipient.is_some_and(|recipient| recipient != self_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The broker's Ern root as it appears in acton's logs, ULID suffix and
    /// all, so the comparison is tested on the strings it really sees.
    const BROKER: &str = "message_broker_01m2y2dvb1ez1r3bfdb2w3r7a4";

    #[test]
    fn a_reply_is_sent_only_to_somebody_other_than_the_broker() {
        let cases = [
            (
                "a publish_ack client waits behind its own proxy",
                Some("ipc_proxy_01K5Q0000000000000000000"),
                true,
            ),
            (
                "fire and forget leaves the broker addressed to itself",
                Some(BROKER),
                false,
            ),
            (
                "an envelope with no recipient replies to its own sender",
                None,
                false,
            ),
            (
                "a second broker incarnation is a different actor",
                Some("message_broker_01m2y2dvb1ez1r3bfdb2w3r7a5"),
                true,
            ),
        ];
        for (case, recipient, expected) in cases {
            assert_eq!(should_reply(BROKER, recipient), expected, "{case}");
        }
    }
}
