//! Pure Discord Gateway MESSAGE_CREATE decoding for a future authenticated bot worker.
//! No socket, credentials, reconnect, delivery, or voice transport is implemented here.
//! The caller must authenticate the Gateway, bound decompression, and pass candidates
//! through the existing host chat intake for deduplication, expiry and target authority.
use crate::extension_protocol::ChatMessage;
use serde::Deserialize;

pub const MAX_GATEWAY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct ChannelFilter {
    guild_id: String,
    channel_id: String,
}

fn snowflake(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

#[derive(Deserialize)]
struct Envelope {
    op: u8,
    t: Option<String>,
    d: serde_json::Value,
}
#[derive(Deserialize)]
struct Message {
    id: String,
    guild_id: Option<String>,
    channel_id: String,
    content: String,
    #[serde(rename = "type")]
    kind: u8,
    author: Author,
    member: Option<Member>,
    webhook_id: Option<String>,
}
#[derive(Deserialize)]
struct Author {
    id: String,
    username: String,
    global_name: Option<String>,
    #[serde(default)]
    bot: bool,
}
#[derive(Deserialize)]
struct Member {
    nick: Option<String>,
}

impl ChannelFilter {
    /// Exactly one guild channel; DMs and threads with different channel IDs are excluded.
    pub fn new(guild_id: String, channel_id: String) -> Result<Self, &'static str> {
        if !snowflake(&guild_id) || !snowflake(&channel_id) {
            return Err("Discord server and channel IDs must be positive numeric snowflakes");
        }
        Ok(Self {
            guild_id,
            channel_id,
        })
    }

    /// None means an irrelevant event, filtered source, system/bot message, or
    /// content-less message. Errors contain fixed diagnostics, never user text.
    /// Message content intent/permissions are a transport setup responsibility.
    pub fn decode(&self, packet: &[u8]) -> Result<Option<ChatMessage>, &'static str> {
        if packet.len() > MAX_GATEWAY_BYTES {
            return Err("Discord Gateway packet exceeds the input limit");
        }
        let packet: Envelope =
            serde_json::from_slice(packet).map_err(|_| "Malformed Discord Gateway envelope")?;
        if packet.op != 0 || packet.t.as_deref() != Some("MESSAGE_CREATE") {
            return Ok(None);
        }
        let message: Message =
            serde_json::from_value(packet.d).map_err(|_| "Malformed Discord message event")?;
        if message.guild_id.as_deref() != Some(self.guild_id.as_str())
            || message.channel_id != self.channel_id
            || message.author.bot
            || message.webhook_id.is_some()
            || !matches!(message.kind, 0 | 19)
            || message.content.trim().is_empty()
        {
            return Ok(None);
        }
        if !snowflake(&message.id) || !snowflake(&message.author.id) {
            return Err("Malformed Discord message or author identity");
        }
        let author_name = message
            .member
            .and_then(|member| member.nick)
            .or(message.author.global_name)
            .unwrap_or(message.author.username);
        let candidate = ChatMessage {
            source_id: format!(
                "discord:{}:{}:{}",
                self.guild_id, self.channel_id, message.id
            ),
            author_id: message.author.id,
            author_name,
            text: message.content,
        };
        candidate
            .validate()
            .map_err(|_| "Discord message exceeds host chat bounds")?;
        Ok(Some(candidate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> Value {
        json!({"op":0,"t":"MESSAGE_CREATE","s":42,"d":{
            "id":"333","guild_id":"111","channel_id":"222","type":0,
            "content":"  Hello\n世界 👋\t ",
            "author":{"id":"444","username":"user","global_name":"Global","bot":false},
            "member":{"nick":"Server name"},"attachments":[]
        }})
    }
    fn decode(value: &Value) -> Result<Option<ChatMessage>, &'static str> {
        ChannelFilter::new("111".into(), "222".into())
            .unwrap()
            .decode(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn preserves_originals_and_qualifies_source_without_inventing_text() {
        let value = fixture();
        let message = decode(&value).unwrap().unwrap();
        assert_eq!(message.source_id, "discord:111:222:333");
        assert_eq!(message.author_id, "444");
        assert_eq!(message.author_name, "Server name");
        assert_eq!(message.text, value["d"]["content"].as_str().unwrap());
    }
    #[test]
    fn display_name_fallbacks_and_replies_are_supported() {
        let mut value = fixture();
        value["d"]["member"] = Value::Null;
        value["d"]["type"] = json!(19);
        assert_eq!(decode(&value).unwrap().unwrap().author_name, "Global");
        value["d"]["author"]["global_name"] = Value::Null;
        assert_eq!(decode(&value).unwrap().unwrap().author_name, "user");
    }
    #[test]
    fn ignores_unrelated_events_without_parsing_message_payload() {
        for (op, event) in [
            (10, Value::Null),
            (0, json!("MESSAGE_UPDATE")),
            (1, json!("MESSAGE_CREATE")),
        ] {
            assert!(
                decode(&json!({"op":op,"t":event,"d":null}))
                    .unwrap()
                    .is_none()
            );
        }
    }
    #[test]
    fn filters_wrong_channels_dms_bots_webhooks_and_system_messages() {
        for (path, replacement) in [
            (vec!["guild_id"], Value::Null),
            (vec!["guild_id"], json!("999")),
            (vec!["channel_id"], json!("999")),
            (vec!["author", "bot"], json!(true)),
            (vec!["webhook_id"], json!("555")),
            (vec!["type"], json!(7)),
            (vec!["content"], json!(" \n\t")),
        ] {
            let mut value = fixture();
            let mut field = &mut value["d"];
            for key in path {
                field = &mut field[key];
            }
            *field = replacement;
            assert!(decode(&value).unwrap().is_none());
        }
    }
    #[test]
    fn validates_configuration_and_message_identities() {
        for id in ["", "0", "01", "-1", "1x", "18446744073709551616"] {
            assert!(ChannelFilter::new(id.into(), "222".into()).is_err());
            assert!(ChannelFilter::new("111".into(), id.into()).is_err());
            for path in [vec!["id"], vec!["author", "id"]] {
                let mut value = fixture();
                let mut field = &mut value["d"];
                for key in path {
                    field = &mut field[key];
                }
                *field = json!(id);
                assert!(decode(&value).is_err());
            }
        }
        assert!(ChannelFilter::new(u64::MAX.to_string(), u64::MAX.to_string()).is_ok());
    }
    #[test]
    fn rejects_oversized_or_control_text_without_truncation() {
        for text in [
            "x".repeat(2049),
            "🦀".repeat(513),
            "bad\0text".into(),
            "bad\u{1b}text".into(),
        ] {
            let mut value = fixture();
            value["d"]["content"] = json!(text);
            assert_eq!(
                decode(&value),
                Err("Discord message exceeds host chat bounds")
            );
        }
        let mut value = fixture();
        value["d"]["content"] = json!("x".repeat(2048));
        assert_eq!(decode(&value).unwrap().unwrap().text.len(), 2048);
    }
    #[test]
    fn rejects_bad_names_and_malformed_payloads_with_fixed_errors() {
        for name in ["", "x\nname", &"x".repeat(129)] {
            let mut value = fixture();
            value["d"]["member"]["nick"] = json!(name);
            assert_eq!(
                decode(&value),
                Err("Discord message exceeds host chat bounds")
            );
        }
        for value in [json!({}), json!({"op":0,"t":"MESSAGE_CREATE","d":{}})] {
            assert!(decode(&value).is_err());
        }
        let filter = ChannelFilter::new("111".into(), "222".into()).unwrap();
        assert!(filter.decode(b"not json").is_err());
        assert!(filter.decode(&vec![b' '; MAX_GATEWAY_BYTES + 1]).is_err());
    }
}
